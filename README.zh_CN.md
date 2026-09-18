# Qubit Tokio Executor

[![Rust CI](https://github.com/qubit-ltd/rs-tokio-executor/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/rs-tokio-executor/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/endpoint?url=https://qubit-ltd.github.io/rs-tokio-executor/coverage-badge.json)](https://qubit-ltd.github.io/rs-tokio-executor/coverage/)
[![Crates.io](https://img.shields.io/crates/v/qubit-tokio-executor.svg?color=blue)](https://crates.io/crates/qubit-tokio-executor)
[![Rust](https://img.shields.io/badge/rust-1.94+-blue.svg?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![English Document](https://img.shields.io/badge/Document-English-blue.svg)](README.md)

Qubit Tokio Executor 为已经采用 Tokio 的 Rust 应用提供统一的执行服务：既能提交可能阻塞的同步任务，也能调度非阻塞 future，并沿用 Qubit 的任务结果和生命周期管理方式。

## 安装

```toml
[dependencies]
qubit-tokio-executor = "0.9"
tokio = { version = "1.53", features = ["macros", "rt-multi-thread", "time"] }
```

本 crate 不负责创建或持有 Tokio runtime。请用应用自身的
`tokio::runtime::Handle` 构造服务，并让 runtime 持续运行到所有已提交任务结束。

## 快速开始

例如，Web 服务需要解析一份计算量较大的输入，但又不能占用 Tokio 的工作线程。将解析闭包交给 `TokioExecutorService`，等待共享任务句柄返回结果；应用退出时再停止接收新任务：

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

若工作本身就是异步 IO，则使用 `TokioIoExecutorService::spawn`，让 future 继续由 Tokio 异步调度器执行。完整的阻塞任务、异步任务、取消、终止和排障说明，请参阅[英文用户手册](doc/user_guide.md)或[中文用户手册](doc/user_guide.zh_CN.md)。

## 为什么需要这个项目

Tokio 提供了任务调度原语，但 Qubit 应用往往还需要在多个执行域中复用一致的 `ExecutorService` 生命周期和任务结果模型。这个 crate 将这些抽象适配到 Tokio，避免业务代码反复围绕 Tokio 句柄实现任务准入、关闭、结果传递与计数管理。

## 核心能力

- `TokioExecutor`：通过 Tokio blocking pool 执行同步 callable 的策略级执行器。
- `TokioExecutorService` 及别名 `TokioBlockingExecutorService`：通过
  `spawn_blocking` 管理 `Runnable` 和 `Callable` 类型的阻塞任务。
- `TokioIoExecutorService`：通过 `tokio::spawn` 调度
  `Future<Output = Result<R, E>>`。
- `TokioBlockingTaskHandle`：跟踪阻塞任务，并可在闭包开始前请求取消；
  `TokioTaskHandle`：等待异步任务结果，并支持尽力而为的 abort 请求。

同步任务可能占用 OS 线程时应选择阻塞服务；非阻塞 future 则使用 IO 服务。不要在 IO future 中执行长时间阻塞操作。`shutdown` 会拒绝后续提交并等待已接受任务完成；`stop` 还会请求取消。阻塞闭包一旦开始执行，Tokio 无法强制终止；异步任务的取消请求也可能与正常完成竞争。

## 延伸阅读

- [用户手册（英文）](doc/user_guide.md)
- [用户手册（中文）](doc/user_guide.zh_CN.md)
- [API 文档](https://docs.rs/qubit-tokio-executor)
- [English README](README.md)

## 测试

```bash
# 使用默认 feature 集运行测试
cargo test

# 使用项目声明的全部 feature 运行测试
cargo test --all-features

# 运行项目 CI 检查
./ci-check.sh

# 检查代码覆盖率
./coverage.sh
```

## 许可证

Copyright (c) 2025 - 2026. Haixing Hu. All rights reserved.

本项目基于 Apache License 2.0 授权。完整许可证文本请参阅
[LICENSE](LICENSE)。

## 贡献

欢迎贡献。请遵循 Rust API 指南，及时更新公共 API 文档与测试，并在提交
Pull Request 前运行 `./align-ci.sh`格式化代码，运行`./ci-check.sh`对齐CI要求。

## 作者

**Haixing Hu** - *Qubit Co. Ltd.*

仓库地址：[https://github.com/qubit-ltd/rs-tokio-executor](https://github.com/qubit-ltd/rs-tokio-executor)
