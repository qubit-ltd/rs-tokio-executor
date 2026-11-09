# Concurrency Protocol

This document describes the implementation in this repository for `qubit-tokio-executor` 0.12.0. It is a maintainer guide to the current source, not a promise that Tokio will schedule work in a particular order. Source and test links are relative to this crate.

## 1. Execution domains

`TokioExecutor` is a strategy-level adapter for blocking callables. `TokioExecutorService` submits synchronous callables with `Handle::spawn_blocking` and tracks queued and running work. Its configured capacity bounds unfinished service tasks, not Tokio's total blocking pool. A queued closure may still retain captures after the service has logically cancelled it; once a blocking closure starts, Tokio cannot forcibly interrupt it.

`TokioIoExecutorService` accepts futures and schedules them with `Handle::spawn`. The future is polled by the bound runtime, and its active-task count includes accepted work not yet polled. Cancelling an IO task asks Tokio to abort it; service accounting is released when Tokio drops the guarded future. Thus the two services have different cancellation completion points.

Implementation: [`tokio_executor_service.rs`](../src/tokio_executor_service.rs), [`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs), [`tokio_guarded_io_future.rs`](../src/internal/tokio_guarded_io_future.rs).

## 2. Admission protocol

Both services serialize admission against shutdown/stop with a submission mutex. Under that lock they check lifecycle first, then reserve capacity. For blocking work, the lock remains held through spawning a receiver task and registering its abort handle. The accepted callable and guard are delivered through a oneshot only after this lock is released. For IO work, the future is retained in a handoff value while a Tokio task and abort registration are created under admission; `deliver` sends it after the lock is released.

If admission is rejected, `try_*` APIs return a `RejectedTask` containing the original task and reason. The compatibility `submit`/`spawn` APIs consume rejected input, so callers needing ownership recovery must use `try_*`. Shutdown rejection takes precedence over saturation.

Implementation: blocking admission and delivery in [`tokio_executor_service.rs`](../src/tokio_executor_service.rs); IO registration and handoff in [`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs) and [`tokio_io_spawn_handoff.rs`](../src/internal/tokio_io_spawn_handoff.rs).

## 3. State machines

Service lifecycle and task-result completion are separate observations. A service moves from `Running` to `ShuttingDown` or `Stopping`; `Terminated` is derived once it is no longer running and its service-side task count is empty. Shutdown allows accepted work to finish. Stop requests aborts, subject to each execution domain's cancellation limits.

For blocking service accounting, each accepted task follows `Queued → Running → Finished`, or `Queued → Finished` if cancellation wins before start. Atomic transitions ensure the queued slot is not finished twice. For IO service accounting, an accepted-future count is held until its guard drops. These accounting states do not determine the result handle's final outcome: the handle reports the task result, cancellation, panic, or runtime drop according to its own completion path. Acceptance itself is not success.

Implementation: [`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs), [`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs), [`tokio_service_task_tracker.rs`](../src/tokio_service_task_guard/internal/tokio_service_task_tracker.rs).

## 4. Locks and callbacks

The submission lock covers the lifecycle/capacity admission decision and setup of a registered Tokio task. It is released before handing user payloads to the Tokio task and before service lifecycle notifications that can wake waiters. The abort-handle map lock covers insertion/removal and the completion check used to avoid stale registration. Removed entries are dropped after releasing the map lock: a queued-cancellation closure can own the final task-slot reference, whose destructor publishes `Dropped` and wakes a result Waker that may reenter `stop`. `stop` takes the map contents under that lock, then invokes abort and queued-cancellation hooks after releasing it. The blocking task-count monitor protects queue/running counters and its wait predicate; its write section is limited to counter transitions.

Wakers may reenter service methods. Notifications therefore occur outside submission admission. Payloads are also handed off outside admission because a failed send or cancellation can drop user-owned captures and run their destructors. IO field order ensures a future is dropped before its task guard updates accounting.

Implementation: [`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs), [`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs), [`tokio_io_spawn_handoff.rs`](../src/internal/tokio_io_spawn_handoff.rs), [`tokio_guarded_io_future.rs`](../src/internal/tokio_guarded_io_future.rs).

## 5. Registration and removal

A per-task marker coordinates task completion with abort-handle registration. If completion marks the marker before registration obtains the map lock, registration sees the finished marker and does not insert. If registration inserts first, completion later removes that entry. The map therefore does not intentionally retain completed tasks. Blocking queued cancellation and task-guard drop share an atomic tracker, so only the transition that wins can decrement the corresponding queued/running accounting.

Implementation: [`tokio_task_registration.rs`](../src/tokio_task_registration.rs), [`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs), [`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs), [`tokio_service_task_tracker.rs`](../src/tokio_service_task_guard/internal/tokio_service_task_tracker.rs).

## 6. Lifecycle and resource release

For IO work, `TokioGuardedIoFuture` declares the user future before the guard. Rust drops fields in declaration order, so user future state is destroyed before the guard decrements active accounting and notifies waiters. A pre-delivery handoff similarly retains the future before its guard. When Tokio processes an abort, that destruction can release capacity and permit termination.

For a blocking task cancelled while still queued, service accounting can release its logical capacity when the queued transition wins. Tokio may still hold the blocking closure and its captures until it processes the aborted task. A started blocking closure is not forcibly interruptible; it retains its service slot until it returns and its guard finishes.

Implementation: [`tokio_guarded_io_future.rs`](../src/internal/tokio_guarded_io_future.rs), [`tokio_io_spawn_handoff.rs`](../src/internal/tokio_io_spawn_handoff.rs), [`tokio_service_task_tracker.rs`](../src/tokio_service_task_guard/internal/tokio_service_task_tracker.rs).

## 7. Notifications and observation

`capacity_changes()` is a watch-based hint that capacity or lifecycle changed. It does not reserve a slot or promise fairness; callers must retry admission. Subscribe before the first attempt so a change between rejection and waiting is observable. Stats are snapshots: blocking counts and lifecycle may be sampled at adjacent instants, and IO fields are independently sampled. Use the task result handle for the final task outcome.

Async termination waiting registers/enables its notification before checking the termination predicate, preventing a transition in between from being lost. Blocking termination waits use the task-count monitor predicate. Notifications can cause arbitrary registered Wakers to run, which is why publication is outside admission locks.

Implementation: [`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs), [`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs). See [`reentrant_waker_tests.rs`](../tests/reentrant_waker_tests.rs), [`tokio_executor_service_submit_tests.rs`](../tests/tokio_executor_service_submit_tests.rs), and [`tokio_io_executor_service_spawn_tests.rs`](../tests/tokio_io_executor_service_spawn_tests.rs).

## 8. Runtime constraints

A Tokio `Handle` selects a runtime; it neither owns nor drives it. Keep the bound runtime alive and let it run until accepted work and cancellation cleanup have progressed. A current-thread runtime advances only while its owner drives it, for example with `Runtime::block_on`. Closing a runtime is distinct from changing service admission state: a service may still accept work after its runtime has shut down, and the returned handle then determines the observed outcome. Do not infer service shutdown from runtime shutdown.

Implementation and tests: [`current_thread_driving_tests.rs`](../tests/current_thread_driving_tests.rs), [`runtime_shutdown_tests.rs`](../tests/runtime_shutdown_tests.rs).

## Invariant → implementation → asserting test

| Invariant | Implementation | Test that asserts it |
| --- | --- | --- |
| IO admission accepted before delivery remains counted until its undelivered future payload is dropped. | [`tokio_io_spawn_handoff.rs`](../src/internal/tokio_io_spawn_handoff.rs), [`tokio_io_executor_service_state.rs`](../src/tokio_io_executor_service_state.rs) | [`tokio_io_executor_service_handoff_tests.rs::test_stop_after_registration_keeps_capacity_until_handoff_payload_drops`](../src/tests/tokio_io_executor_service_handoff_tests.rs) |
| Blocking queued cancellation can release accounting once; a started blocking task is not cancelled by service stop. | [`tokio_service_task_tracker.rs`](../src/tokio_service_task_guard/internal/tokio_service_task_tracker.rs), [`tokio_executor_service_state.rs`](../src/tokio_executor_service_state.rs) | [`tokio_executor_service_submit_tests.rs::test_tokio_executor_service_capacity_reuses_slot_after_queued_cancel`](../tests/tokio_executor_service_submit_tests.rs); [`task_handle_drop_tests.rs::test_stopping_blocking_service_does_not_cancel_started_closure`](../tests/task_handle_drop_tests.rs) |
| Capacity notifications and termination Wakers may reenter without deadlocking on admission. | [`tokio_executor_service.rs`](../src/tokio_executor_service.rs), [`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs), both service state files | [`reentrant_waker_tests.rs::test_reentrant_waker_watchdog_matrix`](../tests/reentrant_waker_tests.rs), [`reentrant_waker_tests.rs::test_queued_blocking_stop_watchdog_matrix`](../tests/reentrant_waker_tests.rs) |
| Removing a blocking abort registration drops its task-slot owner outside the abort-map lock, so a result Waker can reenter abort handling. | [`tokio_executor_service_state.rs::remove_abort_handle`](../src/tokio_executor_service_state.rs) | [`abort_handle_drop_tests.rs::test_remove_abort_handle_drops_slot_outside_abort_map_lock`](../src/tests/abort_handle_drop_tests.rs) |
| A bound current-thread runtime must be driven for an accepted IO future to be polled. | Tokio handle spawn in [`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs) | [`current_thread_driving_tests.rs::test_bound_current_thread_runtime_drives_submitted_future_child`](../tests/current_thread_driving_tests.rs) |
| Runtime shutdown and service admission are distinct; failed/closed-runtime handoff cleanup releases accepted accounting. | service state and handoff in [`tokio_io_executor_service.rs`](../src/tokio_io_executor_service.rs), [`tokio_executor_service.rs`](../src/tokio_executor_service.rs) | [`runtime_shutdown_tests.rs::test_io_submission_after_runtime_shutdown_does_not_deadlock_on_drop`](../tests/runtime_shutdown_tests.rs), [`runtime_shutdown_tests.rs::test_services_accept_submission_after_bound_runtime_shutdown_without_panicking`](../tests/runtime_shutdown_tests.rs) |
| Dropping a result handle does not cancel accepted work or release its service capacity. | Tokio task handles and service guards | [`task_handle_drop_tests.rs::test_dropping_blocking_handle_does_not_release_capacity_or_cancel_work`](../tests/task_handle_drop_tests.rs), [`task_handle_drop_tests.rs::test_dropping_io_handle_detaches_future_until_it_completes`](../tests/task_handle_drop_tests.rs) |
| Termination waiters remain pending while accepted tasks remain, then complete after the last task finishes. | blocking monitor and IO notify/predicate loop in service state files | [`tokio_executor_service_shutdown_tests.rs::test_tokio_executor_service_await_termination_waits_for_tasks`](../tests/tokio_executor_service_shutdown_tests.rs), [`tokio_io_executor_service_shutdown_tests.rs::test_tokio_io_executor_service_await_termination_waits_for_task_completion`](../tests/tokio_io_executor_service_shutdown_tests.rs) |
