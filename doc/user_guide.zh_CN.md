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

需要在饱和后重试时，必须在**第一次提交尝试之前**调用 `capacity_changes()` 订阅。收到 `SubmissionError::Saturated` 后等待 `changed()` 再试；其他错误应直接返回。通知只是提示，不预留槽位、不保证公平，generation 数值也不代表可用容量。提交方法按值消费任务，即使拒绝也不会把 callable 或 future 还给调用方。重试时可用 `Arc<Mutex<Option<C>>>` 保留 callable：包装闭包真正运行时取出它，释放锁后再调用。不要假设任意 future 可以克隆或安全重复执行。被取消的等待若尚未提交，会丢弃包装任务；提交成功后则按句柄契约处理。

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
