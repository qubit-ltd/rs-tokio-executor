# Qubit Tokio Executor 用户手册

[English](user_guide.md) | [README](../README.zh_CN.md) | [API 文档](https://docs.rs/qubit-tokio-executor)

本文适用于 `qubit-tokio-executor` 0.12，面向已经运行 Tokio 的 Rust 应用开发者。它说明如何在处理可能阻塞的同步任务或异步 future 时，复用 Qubit 的任务结果与服务生命周期抽象。

## 概念模型

创建执行器或服务时，需要传入 `tokio::runtime::Handle`。对象会保存这个 runtime 句柄；即使调用方身处另一个 runtime 或普通线程，只要被绑定的 runtime 仍在运行，后续提交仍会使用它。

| 工作类型 | 服务 | Tokio 原语 | 结果句柄 |
| --- | --- | --- | --- |
| 可能阻塞 OS 线程的同步任务 | `TokioExecutorService` | `spawn_blocking` | 共享 `TaskHandle` 或 `TokioBlockingTaskHandle` |
| 非阻塞异步任务 | `TokioIoExecutorService` | `tokio::spawn` | `TokioTaskHandle` |

`shutdown` 会关闭任务准入，但让已经接受的任务继续完成；`stop` 除了关闭准入，还会请求取消。提交成功只代表服务接收了工作，任务是否成功必须以等待返回句柄的结果为准。

## 贯穿场景

假设 HTTP 应用需要解析一份计算量较大的文档，同时仍要及时处理异步请求。目标是拿到解析结果，并在退出时等待已接收任务完成，而不是直接丢弃它们。解析可能占用 OS 线程，因此应使用阻塞任务服务。

## 安装与最小配置

```toml
[dependencies]
qubit-tokio-executor = "0.12"
qubit-executor = "0.9"
tokio = { version = "1.53", features = ["macros", "rt-multi-thread", "time"] }
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

需要在饱和后重试时，必须在**第一次提交尝试之前**调用 `capacity_changes()` 订阅。收到 `SubmissionError::Saturated` 后等待 `changed()` 再试；其他错误应直接返回。通知只是提示，不预留槽位、不保证公平，generation 数值也不代表可用容量。提交方法按值消费任务，即使拒绝也不会把 callable 或 future 还给调用方。重试时可用 `Arc<Mutex<Option<C>>>` 保留 callable：包装闭包真正运行时取出它，释放锁后再调用。不要假设任意 future 可以克隆或安全重复执行。

等待操作本身被取消时，只会丢弃当前包装任务。若应用还需要拿回原始任务，应把所有权槽放在重试流程之外。已接受的包装任务从槽中取出原任务后，原任务就已消费，不能再次提交。遇到 `Shutdown` 或其他错误应立即结束重试，让调用方决定如何处理仍留在槽中的任务。

[Callable 重试程序](../examples/capacity_retry_callable.rs)与 [future 重试程序](../examples/capacity_retry_future.rs)也分别作为独立文件保留。两者都先订阅变化，再用 channel 控制一个占满容量 1 服务的任务；首次提交必须得到 `Saturated`，释放 blocker 后再等通知并重试。future 示例将 `Pin<Box<F>>` 放入 `Arc<Mutex<Option<_>>>`，首次 poll 取出 future 后立即释放互斥锁，再 await。程序会断言结果为 42、任务只执行一次，然后关闭服务并等待终止。

Callable 重试完整程序：

```rust
use std::io;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

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

    // Keep the original FnOnce in this slot. A saturated submission consumes
    // its wrapper, but cannot consume the callable held by the slot.
    let calls = Arc::new(AtomicUsize::new(0));
    let callable_calls = Arc::clone(&calls);
    let callable = Arc::new(Mutex::new(Some(move || {
        callable_calls.fetch_add(1, Ordering::SeqCst);
        Ok::<usize, io::Error>(40 + 2)
    })));
    let mut changes = service.capacity_changes();
    let mut blocker_released = false;
    let mut saturated_attempts = 0;
    let result = loop {
        let callable = Arc::clone(&callable);
        let attempt = service.submit_callable(move || {
            let callable = callable
                .lock()
                .expect("callable mutex poisoned")
                .take()
                .expect("accepted wrapper runs once");
            callable()
        });

        match attempt {
            Ok(handle) => {
                assert_eq!(saturated_attempts, 1, "first attempt must be saturated");
                break handle.await?;
            }
            Err(SubmissionError::Saturated) => {
                saturated_attempts += 1;
                if !blocker_released {
                    release_tx.send(())?;
                    blocker_released = true;
                }
                // Notifications are hints. Always retry admission after one.
                changes.changed().await?;
            }
            Err(SubmissionError::Shutdown) => {
                return Err("service shut down before callable was accepted".into());
            }
            Err(error) => return Err(error.into()),
        }
    };

    assert_eq!(result, 42);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    blocker.await?;
    assert!(callable.lock().expect("callable mutex poisoned").is_none());
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
use std::sync::Mutex;

use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioIoExecutorService;

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

    // This future is deliberately neither Clone nor Unpin. Saturated attempts
    // drop only their wrapper and leave the future in this ownership slot.
    let calls = Arc::new(AtomicUsize::new(0));
    let future_calls = Arc::clone(&calls);
    let original: Pin<Box<dyn Future<Output = Result<usize, io::Error>> + Send>> =
        Box::pin(async move {
            future_calls.fetch_add(1, Ordering::SeqCst);
            Ok(40 + 2)
        });
    let future = Arc::new(Mutex::new(Some(original)));
    let mut release_tx = Some(release_tx);
    let mut changes = service.capacity_changes();
    let mut blocker_released = false;
    let mut saturated_attempts = 0;
    let result = loop {
        let future = Arc::clone(&future);
        let attempt = service.spawn(async move {
            let mut original = future
                .lock()
                .expect("future mutex poisoned")
                .take()
                .expect("accepted wrapper is polled once");
            // Never hold the synchronous mutex across an await point.
            (&mut original).await
        });

        match attempt {
            Ok(handle) => {
                assert_eq!(saturated_attempts, 1, "first attempt must be saturated");
                break handle.await?;
            }
            Err(SubmissionError::Saturated) => {
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
            Err(SubmissionError::Shutdown) => {
                return Err("service shut down before future was accepted".into());
            }
            Err(error) => return Err(error.into()),
        }
    };

    assert_eq!(result, 42);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    blocker.await?;
    assert!(future.lock().expect("future mutex poisoned").is_none());
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

阻塞句柄的 `IntoFuture` 会消费句柄，因此 `handle.await` 一旦开始，不能像 IO 句柄那样再复用它。若要取消尚在队列中的任务，应先调用 `handle.cancel()`，再消费句柄等待结果；也可使用服务级终止等待。丢弃 `TokioBlockingTaskHandle` 或它的等待 future 不会停止闭包。显式取消与服务 `stop` 只能尝试取消尚未开始的任务；已经运行的阻塞闭包会继续执行到返回。两类句柄一旦丢弃，调用方也就放弃了通过该句柄观察最终结果的途径。

## 错误与诊断

`SubmissionError::Shutdown` 表示服务不再接收任务；`SubmissionError::Saturated` 表示未完成任务容量已满。多个调用方协作时，可通过 `is_running`、`is_not_running` 或 `lifecycle` 检查服务状态。两个服务接受任务后，任务本身仍可能返回业务错误；异步任务还可能在最终结果中报告取消或 panic。

`stop` 会返回 `StopReport`。阻塞服务的 `cancelled` 只统计闭包开始前真正取消成功的排队任务；已经开始的阻塞闭包会继续运行，直至自行返回。IO 服务的 `cancelled` 则统计已经向 Tokio abort handle 发出请求的异步任务数。

## 排障

| 现象 | 检查项 | 处理方式 |
| --- | --- | --- |
| 提交被拒绝 | `Shutdown` 表示已关闭，`Saturated` 表示容量已满 | 容量已满时，先订阅容量变化，等待通知后重试。 |
| 异步任务拖慢其他任务 | IO future 中可能执行了阻塞操作 | 将阻塞操作移交给 `TokioExecutorService`。 |
| 阻塞服务迟迟不终止 | 某个阻塞闭包可能已经开始运行 | 等待闭包返回；Tokio 无法强制中止它。 |
| 取消后结果并不确定 | 正常完成与取消可能竞争 | 等待返回的句柄，以最终结果为准。 |

## 限制与最佳实践

在需要的任务全部完成前，请保持 runtime 句柄对应的 runtime 存活。不要把提交成功当作任务成功；应根据工作是否阻塞选择执行域，并在希望已接收任务完成时使用优雅关闭 `shutdown`。`TokioExecutor` 适合策略级的阻塞执行；需要服务生命周期与任务计数时，使用两个 service 类型。

## 延伸阅读

- [README](../README.zh_CN.md)
- [English user guide](user_guide.md)
- [API 文档](https://docs.rs/qubit-tokio-executor)
