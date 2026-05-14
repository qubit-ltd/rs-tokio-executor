# Qubit Tokio Executor

[![Rust CI](https://github.com/qubit-ltd/rs-tokio-executor/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/rs-tokio-executor/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/endpoint?url=https://qubit-ltd.github.io/rs-tokio-executor/coverage-badge.json)](https://qubit-ltd.github.io/rs-tokio-executor/coverage/)
[![Crates.io](https://img.shields.io/crates/v/qubit-tokio-executor.svg?color=blue)](https://crates.io/crates/qubit-tokio-executor)
[![Rust](https://img.shields.io/badge/rust-1.94+-blue.svg?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![English Documentation](https://img.shields.io/badge/docs-English-blue.svg)](README.md)

面向 Rust 的 Tokio-backed executor service。

## 概览

Qubit Tokio Executor 将 Qubit executor 抽象适配到 Tokio。它为通过 `tokio::task::spawn_blocking` 提交的 blocking 函数提供 executor-service 语义，也为通过 `tokio::spawn` 提交的 async future 提供独立服务。

本 crate 不创建也不持有 Tokio runtime。所有会 spawn Tokio 工作的方法，都必须在应用已经配置好的 Tokio runtime 内调用。

## 功能

- 提供 `TokioExecutor`，用于策略级 Tokio blocking 执行。
- 提供 `TokioExecutorService`，用于基于 `spawn_blocking` 的托管 blocking 工作。
- 提供 `TokioBlockingExecutorService` 别名，用于明确表达 Tokio blocking 执行域。
- 提供 `TokioIoExecutorService`，用于基于 `tokio::spawn` 的 async `Future` 工作。
- 提供 `TokioBlockingTaskHandle`，用于 tracked blocking 任务的开始前取消。
- 提供 `TokioTaskHandle`，用于 async IO 任务的 Tokio abort-based 取消。
- 再导出共享的 `ExecutorService`、`SubmissionError`、`StopReport` 与 `CancelResult`，便于使用方导入。

## Runtime 要求

本 crate 假设 Tokio runtime 已经存在。应用需要在 `Cargo.toml` 中启用所需 Tokio runtime feature：

```toml
[dependencies]
qubit-tokio-executor = "0.4.0"
tokio = { version = "1.52", features = ["macros", "rt-multi-thread", "time"] }
```

如果某个方法内部使用 `tokio::spawn` 或 `tokio::task::spawn_blocking`，则调用它时必须已经进入 Tokio runtime。没有 runtime 时调用会返回 `SubmissionError::WorkerSpawnFailed`。

## Blocking 与 IO 任务

同步函数可能阻塞 OS 线程时，使用 `TokioExecutorService` 或 `TokioBlockingExecutorService`。这些任务会走 Tokio blocking pool，不应被用来提交 async IO future。

非阻塞 future 使用 `TokioIoExecutorService`。这些任务运行在 Tokio async scheduler 上，不应在 future 内执行长时间 blocking 操作。

## 关闭与取消

`submit` 或 `spawn` 成功只表示服务接受了任务。Blocking callable 提交通过共享的 `TaskHandle` 报告结果；tracked blocking 提交返回 `TokioBlockingTaskHandle`，它把共享 tracked-task 状态与 Tokio abort handle 结合起来，用于处理 queued blocking 任务。Async IO 提交使用 `TokioTaskHandle`，因为它直接包装 Tokio `JoinHandle`。

`shutdown` 拒绝新任务，并允许已接受任务完成。`stop` 拒绝新任务，并请求取消或 abort 已跟踪的 Tokio 任务。Async IO 任务取消会发送 best-effort Tokio abort 请求；`CancelResult::Cancelled` 只表示请求已发出，最终结果以 await 返回的 `TokioTaskHandle` 为准。通过 Tokio 提交的 blocking 任务只能在 blocking 闭包开始前取消。Queued tracked blocking 任务取消成功后会立即从 service 计数中移除；已经运行的 blocking 代码不能被 Rust 强制停止，服务终止会等待这些代码返回。

`TokioExecutorService` 同时提供阻塞式 `wait_termination` 和异步 `await_termination` service-level 等待。`TokioIoExecutorService` 有意不提供 service-level 异步等待；调用方需要观察 async 任务完成时，应 await `spawn` 返回的 task handle。

## 快速开始

### Tokio blocking 工作

```rust
use std::io;

use qubit_tokio_executor::{ExecutorService, TokioExecutorService};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = TokioExecutorService::new();
    let handle = service.submit_callable(|| Ok::<usize, io::Error>(40 + 2))?;
    assert_eq!(handle.await?, 42);
    service.shutdown();
    service.await_termination().await;

    Ok(())
}
```

### Async IO future

```rust
use std::io;

use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = TokioIoExecutorService::new();
    let handle = service.spawn(async { Ok::<usize, io::Error>(6 * 7) })?;
    assert_eq!(handle.await?, 42);
    service.shutdown();
    assert!(service.is_terminated());

    Ok(())
}
```

## 如何选择 Executor

如果应用已经基于 Tokio，并且需要与 Tokio 调度集成的执行服务，使用 `qubit-tokio-executor`。需要不绑定 runtime 的 OS 线程执行时，使用 `qubit-thread-pool`。CPU 密集型 Rayon 工作使用 `qubit-rayon-executor`。

应用层需要统一装配 blocking、CPU、Tokio blocking 与 async IO 域时，使用 `qubit-execution-services`。

## 测试

快速在本地跑一遍：

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

若要与持续集成（CI）保持一致，请在仓库根目录依次执行：`./align-ci.sh` 将本地工具链与配置对齐到 CI 规则，再执行 `./ci-check.sh` 复现流水线中的检查。需要查看或生成测试覆盖率时，使用 `./coverage.sh`。

## 参与贡献

欢迎通过 Issue 与 Pull Request 参与本仓库。建议：

- 报告缺陷、讨论设计或较大能力扩展时，可先开 Issue 对齐方向再投入实现。
- 单次 PR 尽量聚焦单一主题，便于代码审查与合并历史。
- 提交 PR 前请先运行 `./align-ci.sh`，再运行 `./ci-check.sh`，确保本地与 CI 使用同一套规则且能通过流水线等价检查。
- 若修改运行期行为，请补充或更新相应测试；若影响对外 API 或用户可见行为，请同步更新本文档或相关 rustdoc。
- 如果修改 runtime、关闭或取消行为，请在适用时同时覆盖 blocking 服务和 async IO 服务。

向本仓库贡献内容即表示您同意以 [Apache License, Version 2.0](LICENSE)（与本项目相同）授权您的贡献。

## 许可证与版权

Copyright (c) 2026. Haixing Hu.

本软件依据 [Apache License, Version 2.0](LICENSE) 授权；完整许可文本见仓库根目录的 `LICENSE` 文件。

## 作者与维护

**Haixing Hu** — Qubit Co. Ltd.

| | |
| --- | --- |
| **源码仓库** | [github.com/qubit-ltd/rs-tokio-executor](https://github.com/qubit-ltd/rs-tokio-executor) |
| **API 文档** | [docs.rs/qubit-tokio-executor](https://docs.rs/qubit-tokio-executor) |
| **Crate 发布** | [crates.io/crates/qubit-tokio-executor](https://crates.io/crates/qubit-tokio-executor) |
