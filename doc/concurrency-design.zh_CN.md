# 并发协议

本文说明本仓库中 `qubit-tokio-executor` 0.12.0 的实现，是面向维护者的指南，不承诺 Tokio 按特定顺序调度任务。源码和测试链接均相对于本 crate。

## 1. 执行域

`TokioExecutor` 是用于阻塞 callable 的策略级适配器。`TokioExecutorService` 通过 `Handle::spawn_blocking` 提交同步 callable，并跟踪排队和运行中的工作。其容量限制的是尚未完成的服务任务，不是 Tokio 阻塞线程池的总容量。服务逻辑上取消排队闭包后，Tokio 仍可能持有该闭包及其捕获项；阻塞闭包一旦开始，Tokio 无法强制中断。

`TokioIoExecutorService` 接收 future，并通过 `Handle::spawn` 调度。future 由绑定的 runtime 轮询；活动任务计数包含已接受但尚未轮询的工作。取消 IO 任务会请求 Tokio 中止；服务计数在 Tokio 丢弃受保护的 future 时释放。因此，两个 service 的取消完成时点不同。

实现位置：[`tokio_executor_service.rs`](../src/tokio_executor_service.rs)、[`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs)、[`tokio_guarded_io_future.rs`](../src/internal/tokio_guarded_io_future.rs)。

## 2. 准入流程

两个 service 都用 submission mutex 将准入与 shutdown/stop 串行化。在锁内先检查生命周期，再保留容量。阻塞任务会一直持锁到接收任务的 Tokio 闭包已 spawn 且 abort handle 已注册。只有释放此锁后，才通过 oneshot 交付已接受的 callable 和 guard。IO 任务会在准入锁内创建 Tokio 任务并注册 abort handle，同时把 future 保留在 handoff 值中；释放锁后由 `deliver` 发送 future。

准入被拒时，`try_*` API 返回包含原始任务和原因的 `RejectedTask`。兼容的 `submit`/`spawn` API 会消费被拒任务；需要恢复所有权时应使用 `try_*`。Shutdown 拒绝优先于容量已满。

实现位置：阻塞准入与交付见 [`tokio_executor_service.rs`](../src/tokio_executor_service.rs)；IO 注册与交接见 [`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs) 和 [`tokio_io_spawn_handoff.rs`](../src/internal/tokio_io_spawn_handoff.rs)。

## 3. 状态机

服务生命周期与任务结果完成是两种独立观察。服务从 `Running` 进入 `ShuttingDown` 或 `Stopping`；当服务不再运行且服务侧任务计数为空时，推导出 `Terminated`。Shutdown 允许已接受任务继续完成。Stop 请求中止任务，但受各执行域的取消限制影响。

阻塞服务计数中，每个已接受任务沿 `Queued → Running → Finished` 转换；若取消先于启动，则沿 `Queued → Finished` 转换。原子状态转换确保排队任务不会被重复结束。IO 服务则持有已接受 future 的计数，直到 guard 析构。这些计数状态不决定结果句柄的最终结果：句柄依据各自完成路径报告任务结果、取消、panic 或 runtime 丢弃。接受任务本身不等于任务成功。

实现位置：[`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs)、[`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs)、[`tokio_service_task_tracker.rs`](../src/tokio_service_task_guard/internal/tokio_service_task_tracker.rs)。

## 4. 锁与回调

submission lock 覆盖生命周期/容量准入决策和已注册 Tokio 任务的初始化。它会在向 Tokio 任务交付用户 payload，以及发布可能唤醒等待者的服务生命周期通知之前释放。abort-handle map 锁覆盖插入、移除和用于避免陈旧注册的完成状态检查。移除的条目会在释放 map 锁后析构：排队取消闭包可能持有最后一个 task slot 引用，其析构会发布 `Dropped`，并唤醒可能重入 `stop` 的结果 Waker。`stop` 在此锁内取走 map 内容，随后释放锁，再调用 abort 和排队取消 hook。阻塞任务计数 monitor 保护队列/运行计数及其等待谓词；写锁区间只覆盖计数转换。

Waker 可以重入 service 方法。因此通知在准入锁之外发布。payload 也在准入锁外交付，因为发送失败或取消可能析构用户持有的捕获项并执行其析构函数。IO 字段顺序确保 future 先析构，再由 task guard 更新计数。

实现位置：[`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs)、[`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs)、[`tokio_io_spawn_handoff.rs`](../src/internal/tokio_io_spawn_handoff.rs)、[`tokio_guarded_io_future.rs`](../src/internal/tokio_guarded_io_future.rs)。

## 5. 注册与移除

每个任务的 marker 用于协调任务完成与 abort-handle 注册。如果完成先于注册取得 map 锁而标记 marker，注册会看到已完成状态，不插入条目；如果先完成注册，随后任务完成会移除该条目。因此 map 不会有意保留已完成任务。阻塞排队取消和 task guard 析构共享原子 tracker，只有获胜的状态转换才能递减相应的排队/运行计数。

实现位置：[`tokio_task_registration.rs`](../src/tokio_task_registration.rs)、[`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs)、[`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs)、[`tokio_service_task_tracker.rs`](../src/tokio_service_task_guard/internal/tokio_service_task_tracker.rs)。

## 6. 生命周期与资源释放

IO 工作的 `TokioGuardedIoFuture` 将用户 future 字段声明在 guard 之前。Rust 按字段声明顺序析构，因此用户 future 状态先销毁，随后 guard 才递减活动计数并通知等待者。尚未交付的 handoff 也将 future 保留在 guard 之前。Tokio 处理 abort 时，future 的析构可以释放容量并允许服务终止。

若阻塞任务仍在队列中时被取消，服务计数可在排队状态转换获胜后释放逻辑容量。Tokio 可能仍持有该阻塞闭包及其捕获项，直到处理 abort。已经开始的阻塞闭包不会被强制中断；它会继续占用服务计数槽位，直到返回并由 guard 完成计数。

实现位置：[`tokio_guarded_io_future.rs`](../src/internal/tokio_guarded_io_future.rs)、[`tokio_io_spawn_handoff.rs`](../src/internal/tokio_io_spawn_handoff.rs)、[`tokio_service_task_tracker.rs`](../src/tokio_service_task_guard/internal/tokio_service_task_tracker.rs)。

## 7. 通知与观察

`capacity_changes()` 是基于 watch 的提示，表示容量或生命周期发生了变化。它不会预留槽位，也不保证公平；调用方必须重新尝试准入。应在首次尝试前订阅，以便观察到拒绝与等待之间发生的变化。stats 是快照：阻塞任务计数和生命周期可能在相邻时刻采样，IO 各字段则分别采样。任务最终结果应以结果句柄为准。

异步终止等待会先注册/启用通知，再检查终止谓词，避免两者之间发生的状态转换被漏掉。阻塞终止等待使用任务计数 monitor 谓词。通知可能运行任意已注册 Waker，因此通知发布必须在准入锁之外。

实现位置：[`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs)、[`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs)。参见 [`reentrant_waker_tests.rs`](../tests/reentrant_waker_tests.rs)、[`tokio_executor_service_submit_tests.rs`](../tests/tokio_executor_service_submit_tests.rs) 和 [`tokio_io_executor_service_spawn_tests.rs`](../tests/tokio_io_executor_service_spawn_tests.rs)。

## 8. Runtime 约束

Tokio `Handle` 用于选择 runtime；它不拥有也不驱动 runtime。应让绑定的 runtime 保持存活，直到已接受任务结束。异步任务及异步取消清理需要 runtime 获得推进；current-thread runtime 只有在所有者驱动它时才会轮询这些任务，例如调用 `Runtime::block_on`。通过 `TokioExecutor` 和 `TokioExecutorService` 提交的同步 callable 在 Tokio blocking pool 上运行，单纯执行它们不需要调用 `block_on`。如果阻塞 callable 等待绑定 runtime 上的异步任务，该异步任务仍需要 runtime 获得驱动。关闭 runtime 与更改 service 准入状态是两回事：runtime 关闭后 service 仍可能接受任务，此时返回句柄决定最终观察到的结果。不能把 runtime 关闭视作 service shutdown。

实现与测试：[`current_thread_driving_tests.rs`](../tests/current_thread_driving_tests.rs)、[`runtime_shutdown_tests.rs`](../tests/runtime_shutdown_tests.rs)。

## 不变量 → 实现 → 断言该不变量的测试

| 不变量 | 实现 | 断言该不变量的测试 |
| --- | --- | --- |
| IO 准入已接受但尚未交付的任务，在 handoff payload 析构前仍计入容量。 | [`tokio_io_spawn_handoff.rs`](../src/internal/tokio_io_spawn_handoff.rs)、[`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs) | [`tokio_io_executor_service_handoff_tests.rs::test_stop_after_registration_keeps_capacity_until_handoff_payload_drops`](../src/tests/tokio_io_executor_service_handoff_tests.rs) |
| 阻塞任务的排队取消最多释放一次计数；service stop 不会取消已开始的阻塞任务。 | [`tokio_service_task_tracker.rs`](../src/tokio_service_task_guard/internal/tokio_service_task_tracker.rs)、[`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs) | [`tokio_executor_service_submit_tests.rs::test_tokio_executor_service_capacity_reuses_slot_after_queued_cancel`](../tests/tokio_executor_service_submit_tests.rs)；[`task_handle_drop_tests.rs::test_stopping_blocking_service_does_not_cancel_started_closure`](../tests/task_handle_drop_tests.rs) |
| 容量通知和终止 Waker 可以重入而不会因准入锁死锁。 | [`tokio_executor_service.rs`](../src/tokio_executor_service.rs)、[`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs)、两个 service state 文件 | [`reentrant_waker_tests.rs::test_reentrant_waker_watchdog_matrix`](../tests/reentrant_waker_tests.rs)、[`reentrant_waker_tests.rs::test_queued_blocking_stop_watchdog_matrix`](../tests/reentrant_waker_tests.rs) |
| 移除 blocking abort 注册项时，在释放 abort map 锁后再析构 task slot 持有者，使结果 Waker 可重入 abort 处理。 | [`tokio_executor_service_state.rs::remove_abort_handle`](../src/tokio_executor_service_state.rs) | [`abort_handle_drop_tests.rs::test_remove_abort_handle_drops_slot_outside_abort_map_lock`](../src/tests/abort_handle_drop_tests.rs) |
| 绑定的 current-thread runtime 必须被驱动，已接受的 IO future 才会被轮询；纯阻塞 callable 可在不调用 `block_on` 时运行。 | [`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs) 中 Tokio handle 的 spawn；[`tokio_executor.rs`](../src/tokio_executor.rs) 与 [`tokio_executor_service.rs`](../src/tokio_executor_service.rs) 中的 `spawn_blocking` | [`current_thread_driving_tests.rs`](../tests/current_thread_driving_tests.rs) |
| runtime 关闭与 service 准入相互独立；关闭 runtime 后的失败/交接清理会释放已接受计数。 | [`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs)、[`tokio_executor_service.rs`](../src/tokio_executor_service.rs) 中的 service state 与 handoff | [`runtime_shutdown_tests.rs::test_io_submission_after_runtime_shutdown_does_not_deadlock_on_drop`](../tests/runtime_shutdown_tests.rs)、[`runtime_shutdown_tests.rs::test_services_accept_submission_after_bound_runtime_shutdown_without_panicking`](../tests/runtime_shutdown_tests.rs) |
| 丢弃结果句柄不会取消已接受任务，也不会释放其服务容量。 | Tokio task handle 与 service guard | [`task_handle_drop_tests.rs::test_dropping_blocking_handle_does_not_release_capacity_or_cancel_work`](../tests/task_handle_drop_tests.rs)、[`task_handle_drop_tests.rs::test_dropping_io_handle_detaches_future_until_it_completes`](../tests/task_handle_drop_tests.rs) |
| 仍有已接受任务时，终止等待保持等待；最后一个任务完成后才结束。 | 两个 service state 文件中的阻塞 monitor 和 IO notify/谓词循环 | [`tokio_executor_service_shutdown_tests.rs::test_tokio_executor_service_await_termination_waits_for_tasks`](../tests/tokio_executor_service_shutdown_tests.rs)、[`tokio_io_executor_service_shutdown_tests.rs::test_tokio_io_executor_service_await_termination_waits_for_task_completion`](../tests/tokio_io_executor_service_shutdown_tests.rs) |
