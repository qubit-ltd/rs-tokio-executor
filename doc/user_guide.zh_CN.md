# Qubit Tokio Executor 用户手册

[English](user_guide.md) | [README](../README.zh_CN.md) | [API 文档](https://docs.rs/qubit-tokio-executor)

本文适用于 `qubit-tokio-executor` 0.9，面向已经运行 Tokio 的 Rust 应用开发者。它说明如何在处理可能阻塞的同步任务或异步 future 时，复用 Qubit 的任务结果与服务生命周期抽象。

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
qubit-tokio-executor = "0.9"
tokio = { version = "1.53", features = ["macros", "rt-multi-thread", "time"] }
```

本 crate 不会替你启动 runtime。请在已有 Tokio 应用中构造服务，或者传入应用自己持有的 runtime 句柄。

## 核心工作流

阻塞服务的方法来自 `ExecutorService` trait，因此先导入该 trait。提交 callable，等待结果，然后进行优雅关闭并等待服务终止：

```rust
use std::io;

use qubit_tokio_executor::{ExecutorService, TokioExecutorService};

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

## 错误与诊断

`SubmissionError::Shutdown` 表示服务不再接收任务。多个调用方协作时，可通过 `is_running`、`is_not_running` 或 `lifecycle` 检查服务状态。两个服务接受任务后，任务本身仍可能返回业务错误；异步任务还可能在最终结果中报告取消或 panic。

`stop` 会返回 `StopReport`。阻塞服务的 `cancelled` 只统计闭包开始前真正取消成功的排队任务；已经开始的阻塞闭包会继续运行，直至自行返回。IO 服务的 `cancelled` 则统计已经向 Tokio abort handle 发出请求的异步任务数。

## 排障

| 现象 | 检查项 | 处理方式 |
| --- | --- | --- |
| 提交被拒绝 | 服务可能已经 `shutdown` 或 `stop` | 在服务处于运行状态时提交。 |
| 异步任务拖慢其他任务 | IO future 中可能执行了阻塞操作 | 将阻塞操作移交给 `TokioExecutorService`。 |
| 阻塞服务迟迟不终止 | 某个阻塞闭包可能已经开始运行 | 等待闭包返回；Tokio 无法强制中止它。 |
| 取消后结果并不确定 | 正常完成与取消可能竞争 | 等待返回的句柄，以最终结果为准。 |

## 限制与最佳实践

在需要的任务全部完成前，请保持 runtime 句柄对应的 runtime 存活。不要把提交成功当作任务成功；应根据工作是否阻塞选择执行域，并在希望已接收任务完成时使用优雅关闭 `shutdown`。`TokioExecutor` 适合策略级的阻塞执行；需要服务生命周期与任务计数时，使用两个 service 类型。

## 延伸阅读

- [README](../README.zh_CN.md)
- [English user guide](user_guide.md)
- [API 文档](https://docs.rs/qubit-tokio-executor)
