# Qubit Tokio Executor 用户手册

[English](user_guide.md) | [README](../README.zh_CN.md) | [API 文档](https://docs.rs/qubit-tokio-executor)

本文适用于 `qubit-tokio-executor` 0.12，面向已经运行 Tokio 的 Rust 应用开发者。它说明如何在处理可能阻塞的同步任务或异步 future 时，复用 Qubit 的任务结果与服务生命周期抽象。

## 概念模型

创建执行器或服务时，需要传入 `tokio::runtime::Handle`。对象会保存这个 runtime 句柄；即使调用方身处另一个 runtime 或普通线程，只要被绑定的 runtime 仍在运行，后续提交仍会使用它。提交到哪个 runtime 与该 runtime 是否正在推进是两回事：`Handle` 只用于定位 runtime，不会启动或驱动它。

| 工作类型 | 服务 | Tokio 原语 | 结果句柄 |
| --- | --- | --- | --- |
| 可能阻塞 OS 线程的同步任务 | `TokioExecutorService` | `spawn_blocking` | 共享 `TaskHandle` 或 `TokioBlockingTaskHandle` |
| 非阻塞异步任务 | `TokioIoExecutorService` | `tokio::spawn` | `TokioTaskHandle` |

`shutdown` 会关闭任务准入，但让已经接受的任务继续完成；`stop` 除了关闭准入，还会请求取消。提交成功只代表服务接收了工作，任务是否成功必须以等待返回句柄的结果为准。

## Runtime 存活与驱动推进

在已接收的任务结束前，既要让创建 `Handle` 的 runtime 保持存活，也要让 Tokio 有机会调度任务和处理取消。多线程 runtime 运行期间会持续驱动 worker；current-thread runtime 则只会在所属线程执行 `Runtime::block_on` 时推进。单独持有 `Handle` 不会让任务运行。

例如，在另一个 runtime 中 await `TokioTaskHandle`，只会等待句柄结果，不会替绑定的 current-thread runtime 执行任务。应由绑定 runtime 的所有者调用 `block_on`，或让应用继续运行在该 runtime 中，直到任务和服务结束。`stop` 发出 abort 请求后，也需要 runtime 继续运行，Tokio 才能销毁被取消的 future 并更新服务计数。在其他 runtime 中等待 `await_termination()` 同样不会推进绑定的 current-thread runtime。不要在负责驱动任务的线程上同步等待该任务；应采用异步等待，同时让 runtime 继续调度。

## 贯穿场景

假设 HTTP 应用需要解析一份计算量较大的文档，同时仍要及时处理异步请求。目标是拿到解析结果，并在退出时等待已接收任务完成，而不是直接丢弃它们。解析可能占用 OS 线程，因此应使用阻塞任务服务。

## 安装与最小配置

```toml
[dependencies]
qubit-tokio-executor = "0.12"
qubit-executor = "0.9.1"
tokio = { version = "1.54", features = ["macros", "rt-multi-thread", "time"] }
```

本 crate 不会替你启动 runtime。请在已有 Tokio 应用中构造服务，或者传入应用自己持有的 runtime 句柄。

## 核心工作流

阻塞服务的方法来自 `ExecutorService` trait，因此先导入该 trait。提交 callable，等待结果，然后进行优雅关闭并等待服务终止：

```rust
use std::io;

use qubit_executor::service::ExecutorService;
use qubit_tokio_executor::TokioExecutorService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let handle = service.submit_callable(|| Ok::<usize, io::Error>(40 + 2))?;
    assert_eq!(handle.await?, 42);
    service.shutdown();
    service.await_termination().await;
    Ok(())
}
```

`submit_callable` 返回的 `Result` 表示提交是否成功；等待任务句柄得到的结果才表示 callable 本身是否成功。

## 进阶用法

若工作本身是非阻塞异步 IO，请交给 `TokioIoExecutorService`：

```rust
use std::io;

use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let handle = service.spawn(async { Ok::<usize, io::Error>(6 * 7) })?;
    assert_eq!(handle.await?, 42);
    service.shutdown();
    service.await_termination().await;
    Ok(())
}
```

需要单独取消排队中的阻塞任务时，使用 `submit_tracked_callable`。`TokioBlockingTaskHandle::cancel` 只有在阻塞闭包尚未开始时才能取消任务。异步任务的 `TokioTaskHandle::cancel` 会向 Tokio 发出 abort 请求；完成与取消可能竞争，最终仍应等待句柄确认结果。

`TokioBlockingTaskHandle::cancellation_handle` 从
`qubit-tokio-executor` 0.12.0 版本开始提供。在把阻塞句柄消费为 future
之前保存此端点，等待过程中就可以通过它请求取消。端点及其克隆共享同一取消路径；丢弃端点不会取消任务。阻塞闭包开始后仍无法被 Tokio 强制中断。0.12.0 之前的版本没有此端点；这些版本应在等待句柄前调用 `handle.cancel()`。

两个服务默认各自最多接收 1024 个尚未完成的任务。阻塞服务统计排队和
运行中的任务；IO 服务还统计 Tokio 已接收但尚未 poll 的 future。达到容量
后提交返回 `SubmissionError::Saturated`。可使用
`TokioExecutorService::with_task_capacity` 或
`TokioIoExecutorService::with_task_capacity` 并传入 `NonZeroUsize` 设置其他
有限容量。任务完成或服务记账中的任务取消后会归还逻辑容量。排队中的阻塞任务成功取消后，容量会立即归还；但取消结果或服务终止都不保证 Tokio 已销毁排队闭包及其 captures。已经开始运行的阻塞闭包不能被 abort，会一直占用容量直到返回。IO 任务在 Tokio 处理 abort 并销毁受保护的 future 后归还容量。因此容量限制的是逻辑上未完成的任务数量，不是可能仍留在 Tokio 队列中的捕获资源上限。服务先检查关闭状态，再检查容量；即使容量已满，关闭后的提交也返回 `SubmissionError::Shutdown`。

可通过 `service.stats()` 排查容量压力：阻塞服务提供 `task_capacity`、
`queued` 和 `running`；IO 服务提供 `task_capacity` 与
`accepted_unfinished`（包含 Tokio 尚未 poll 的 future）。两者都包含
`lifecycle`。这些数据只是某一时刻的诊断快照，不会预留容量，也不保证
下一次提交一定成功。

需要在饱和后重试时，先调用 `capacity_changes()` 订阅，再进行第一次提交。使用 `try_submit`、`try_submit_callable`、`try_submit_tracked_callable` 或 `try_spawn`；拒绝时会得到 `RejectedTask<T>`。调用 `into_parts()` 可取回拒绝原因和原任务，任务尚未执行、poll 或析构。只有原因是 `SubmissionError::Saturated` 时才等待 `changed()` 并重新提交；通知只是提示，不预留槽位、不保证公平，generation 也不是可用容量计数。遇到 `Shutdown` 或其他错误，应结束重试并由调用方决定如何处置任务。

如果等待变化的重试 future 被取消，其中的局部任务也会随之析构；需要跨取消保留任务时，应把所有权放在可取消操作之外。`RejectedTask` 的 `Debug` 只展示拒绝原因，不打印闭包捕获，也不要求任务实现 `Debug`。

[Callable 重试程序](../examples/capacity_retry_callable.rs)和 [future 重试程序](../examples/capacity_retry_future.rs)均先订阅容量变化，用 channel 控制一个占满容量为 1 的服务任务，确认第一次提交得到 `Saturated`，再通过 `RejectedTask::into_parts()` 恢复任务并重试。两段程序都会断言结果为 42、任务只执行一次，然后关闭服务并等待终止。

Callable 重试完整程序：

```rust
use std::io;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use qubit_executor::service::ExecutorService;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioExecutorService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = TokioExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity is nonzero"),
    );

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let started_tx = Mutex::new(Some(started_tx));
    let blocker = service.submit_callable(move || {
        started_tx
            .lock()
            .expect("started sender mutex poisoned")
            .take()
            .expect("blocker starts once")
            .send(())
            .expect("main is still waiting");
        release_rx.recv().expect("main releases blocker");
        Ok::<(), io::Error>(())
    })?;
    started_rx.await?;

    // This non-Clone callable stays in the caller's hands until admission
    // succeeds. Saturation returns it without running or dropping it.
    let calls = Arc::new(AtomicUsize::new(0));
    let callable_calls = Arc::clone(&calls);
    let mut callable = Some(move || {
        callable_calls.fetch_add(1, Ordering::SeqCst);
        Ok::<usize, io::Error>(40 + 2)
    });
    let mut changes = service.capacity_changes();
    let mut blocker_released = false;
    let mut saturated_attempts = 0;
    let result = loop {
        let task = callable.take().expect("rejected callable is restored");
        match service.try_submit_callable(task) {
            Ok(handle) => {
                assert_eq!(saturated_attempts, 1, "first attempt must be saturated");
                break handle.await?;
            }
            Err(rejected) => {
                let (reason, task) = rejected.into_parts();
                callable = Some(task);
                match reason {
                    SubmissionError::Saturated => {
                        saturated_attempts += 1;
                        if !blocker_released {
                            release_tx.send(())?;
                            blocker_released = true;
                        }
                        changes.changed().await?;
                    }
                    SubmissionError::Shutdown => {
                        return Err("service shut down before callable was accepted".into());
                    }
                    error => return Err(error.into()),
                }
            }
        }
    };

    assert_eq!(result, 42);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    blocker.await?;
    assert!(callable.is_none());
    service.shutdown();
    service.await_termination().await;
    Ok(())
}
```

Future 重试完整程序：

```rust
use std::future::Future;
use std::io;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioIoExecutorService;

type RetryFuture = Pin<Box<dyn Future<Output = Result<usize, io::Error>> + Send>>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity is nonzero"),
    );

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let blocker = service.spawn(async move {
        started_tx.send(()).expect("main is still waiting");
        release_rx.await.expect("main releases blocker");
        Ok::<(), io::Error>(())
    })?;
    started_rx.await?;

    // Keep the original pinned, non-Clone future until admission succeeds.
    let calls = Arc::new(AtomicUsize::new(0));
    let future_calls = Arc::clone(&calls);
    let mut future: Option<RetryFuture> = Some(Box::pin(async move {
            future_calls.fetch_add(1, Ordering::SeqCst);
            Ok(40 + 2)
        }));
    let mut release_tx = Some(release_tx);
    let mut changes = service.capacity_changes();
    let mut blocker_released = false;
    let mut saturated_attempts = 0;
    let result = loop {
        let task = future.take().expect("rejected future is restored");
        match service.try_spawn(task) {
            Ok(handle) => {
                assert_eq!(saturated_attempts, 1, "first attempt must be saturated");
                break handle.await?;
            }
            Err(rejected) => {
                let (reason, task) = rejected.into_parts();
                future = Some(task);
                match reason {
                    SubmissionError::Saturated => {
                        saturated_attempts += 1;
                        if !blocker_released {
                            release_tx
                                .take()
                                .expect("release sender is available")
                                .send(())
                                .map_err(|_| "blocker stopped before release")?;
                            blocker_released = true;
                        }
                        changes.changed().await?;
                    }
                    SubmissionError::Shutdown => {
                        return Err("service shut down before future was accepted".into());
                    }
                    error => return Err(error.into()),
                }
            }
        }
    };

    assert_eq!(result, 42);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    blocker.await?;
    assert!(future.is_none());
    service.shutdown();
    service.await_termination().await;
    Ok(())
}
```

IO 句柄可以借用来等待超时，因此超时后还能主动取消并检查最终结果：

```rust
use std::io;
use std::time::Duration;

use qubit_executor::CancelResult;
use qubit_executor::TaskExecutionError;
use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let mut handle = service.spawn(std::future::pending::<Result<usize, io::Error>>())?;
    assert!(tokio::time::timeout(Duration::from_millis(1), &mut handle).await.is_err());
    assert_eq!(handle.cancel(), CancelResult::Cancelled);
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    service.shutdown();
    service.await_termination().await;
    Ok(())
}
```

`timeout` 只结束这次等待；因为传入的是 `&mut handle`，句柄仍在手中，可以继续请求取消并确认最终结果。直接丢弃 `TokioTaskHandle` 会让 Tokio 任务脱离当前观察者，不会取消任务；丢弃等待 future 也不会发出取消请求。`shutdown` 只关闭准入并让已接收任务继续执行；`stop` 还会请求取消，仍可能与正常完成竞争。最终应以等待句柄所得结果为准。

阻塞句柄的 `IntoFuture` 会消费句柄，因此 `handle.await` 一旦开始，不能像 IO 句柄那样再复用它。0.12.0 及之后的版本可以在消费句柄前保存取消端点，以便等待期间请求取消；更早版本应在消费句柄前调用 `handle.cancel()`。也可使用服务级终止等待。丢弃 `TokioBlockingTaskHandle` 或它的等待 future 不会停止闭包。显式取消与服务 `stop` 只能尝试取消尚未开始的任务；已经运行的阻塞闭包会继续执行到返回。两类句柄一旦丢弃，调用方也就放弃了通过该句柄观察最终结果的途径。

## 错误与诊断

`SubmissionError::Shutdown` 表示服务不再接收任务；`SubmissionError::Saturated` 表示未完成任务容量已满。多个调用方协作时，可通过 `is_running`、`is_not_running` 或 `lifecycle` 检查服务状态。两个服务接受任务后，任务本身仍可能返回业务错误；异步任务还可能在最终结果中报告取消或 panic。

提交错误描述的是服务准入状态，不是 Tokio runtime 是否存活。`TokioExecutor::call` 不会因绑定的 runtime 已关闭而返回 `SubmissionError::Shutdown`。阻塞任务仍可能被接受；若 Tokio 在 runner 启动前销毁任务，句柄会报告 `Dropped`。服务自身调用 `shutdown` 或 `stop` 后会拒绝新任务；若 runtime 独立关闭，服务的准入判断不会因此改变。runtime 关闭后接受的 IO 任务若被 Tokio 销毁，句柄可能报告 `Cancelled`。关闭竞态可能产生不同的最终结果，应以返回句柄观察到的结果为准。

| 情况 | 提交结果 | 观察结果 |
| --- | --- | --- |
| 服务已关闭 | `Shutdown` | 任务未被接受。 |
| 服务仍在运行但容量已满 | `Saturated` | 任务未被接受；可保留任务并重试。 |
| runtime 已关闭，提交阻塞任务 | 已接受（`Ok(handle)`） | 在 Tokio 于 runner 启动前销毁任务的已覆盖场景中，句柄报告 `Dropped`。 |
| runtime 已关闭，提交 IO 任务 | 已接受（`Ok(handle)`） | 在 Tokio 销毁任务的已覆盖场景中，句柄报告 `Cancelled`。 |

runtime 关闭的表格场景来自已有回归测试，并不代表每种关闭竞态都只有这一种结果。已经启动的阻塞任务可以继续执行直至返回；应以句柄的最终结果为准。

使用 `try_*` 方法时，准入失败会返回 `RejectedTask<T>`。可先通过 `reason()` 查看拒绝原因，再用 `into_parts()` 取回任务并决定是否重试。旧的 `submit_*` 方法仍返回 `SubmissionError`，并保持拒绝时消费任务的行为。

`stop` 会返回 `StopReport`。阻塞服务的 `cancelled` 只统计闭包开始前真正取消成功的排队任务；已经开始的阻塞闭包会继续运行，直至自行返回。IO 服务的 `cancelled` 则统计已经向 Tokio abort handle 发出请求的异步任务数。

## 排障

| 现象 | 检查项 | 处理方式 |
| --- | --- | --- |
| 提交被拒绝 | `Shutdown` 表示已关闭，`Saturated` 表示容量已满 | 首次提交前订阅容量变化；提交返回 `Saturated` 后等待通知再重试。若收到拒绝后才创建订阅，先立即重试，仍为 `Saturated` 时再等待；遇到 `Shutdown` 则结束重试。 |
| 绑定的 runtime 关闭后仍接受了任务 | 提交检查服务准入，不探测 runtime 是否存活 | 检查返回句柄；若 Tokio 在执行前销毁任务，阻塞任务可能为 `Dropped`，IO 任务可能为 `Cancelled`。 |
| 已接受的 IO 任务没有运行 | 绑定的 current-thread runtime 可能没有被驱动 | 由 runtime 所有者调用 `Runtime::block_on`；在另一个 runtime 中 await 任务句柄不会驱动它。 |
| `stop` 后服务一直未终止 | Tokio 可能还没有处理任务的 abort 请求 | 保持绑定 runtime 有机会运行，并在该 runtime 上等待服务终止。 |
| 异步任务拖慢其他任务 | IO future 中可能执行了阻塞操作 | 将阻塞操作移交给 `TokioExecutorService`。 |
| 阻塞服务迟迟不终止 | 某个阻塞闭包可能已经开始运行 | 等待闭包返回；Tokio 无法强制中止它。 |
| 取消后结果并不确定 | 正常完成与取消可能竞争 | 等待返回的句柄，以最终结果为准。 |

## 限制与最佳实践

在需要的任务全部完成前，请让绑定的 runtime 保持存活并持续获得运行机会。不要在负责驱动任务的线程上同步等待这些任务，否则该线程无法继续调度它们。提交成功不代表任务成功；应根据工作是否阻塞选择执行域，并在希望已接收任务完成时使用优雅关闭 `shutdown`。`TokioExecutor` 适合策略级的阻塞执行；需要服务生命周期与任务计数时，使用两个 service 类型。

## 延伸阅读

- [README](../README.zh_CN.md)
- [English user guide](user_guide.md)
- [API 文档](https://docs.rs/qubit-tokio-executor)
- [并发协议](concurrency-design.zh_CN.md)
