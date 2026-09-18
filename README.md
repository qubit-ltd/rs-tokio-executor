# Qubit Tokio Executor

[![Rust CI](https://github.com/qubit-ltd/rs-tokio-executor/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/rs-tokio-executor/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/endpoint?url=https://qubit-ltd.github.io/rs-tokio-executor/coverage-badge.json)](https://qubit-ltd.github.io/rs-tokio-executor/coverage/)
[![Crates.io](https://img.shields.io/crates/v/qubit-tokio-executor.svg?color=blue)](https://crates.io/crates/qubit-tokio-executor)
[![Rust](https://img.shields.io/badge/rust-1.94+-blue.svg?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![中文文档](https://img.shields.io/badge/文档-中文版-blue.svg)](README.zh_CN.md)

Qubit Tokio Executor gives Rust applications already built on Tokio a
service-oriented way to submit blocking callables and non-blocking futures,
then observe their results and control their lifecycle through the shared Qubit
executor abstractions.

## Installation

```toml
[dependencies]
qubit-tokio-executor = "0.9"
tokio = { version = "1.53", features = ["macros", "rt-multi-thread", "time"] }
```

The crate does not create or own a Tokio runtime. Construct a service with the
application's `tokio::runtime::Handle`, and keep that runtime alive while work
may run.

## Quick Start

An API service can keep CPU-heavy parsing off Tokio worker threads while it
continues handling async requests. Submit the parsing closure to
`TokioExecutorService`, await its shared task handle, then gracefully close the
service:

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

Use `TokioIoExecutorService::spawn` for non-blocking futures. The [English user
guide](doc/user_guide.md) and [Chinese user guide](doc/user_guide.zh_CN.md)
explain both workflows, cancellation, termination, and troubleshooting.

## Why This Project Exists

Tokio provides scheduling primitives, while Qubit applications may also need a
common `ExecutorService` lifecycle and task-result model across execution
domains. This crate adapts those abstractions to Tokio instead of requiring each
caller to reproduce task admission, shutdown, result handling, and accounting.

## What It Provides

- `TokioExecutor` for strategy-level synchronous callable execution through
  Tokio's blocking pool.
- `TokioExecutorService` (and `TokioBlockingExecutorService`) for managed
  blocking `Runnable` and `Callable` work submitted with `spawn_blocking`.
- `TokioIoExecutorService` for `Future<Output = Result<R, E>>` work submitted
  with `tokio::spawn`.
- `TokioBlockingTaskHandle` for tracked blocking work that can be cancelled
  before its closure starts, and `TokioTaskHandle` for async results and
  best-effort abort requests.

Choose the blocking service for synchronous work that can occupy an OS thread;
choose the IO service for non-blocking futures. Do not run long blocking work
inside an IO future. `shutdown` rejects new tasks and lets accepted work finish;
`stop` also requests cancellation. Tokio cannot forcibly stop a blocking closure
after it starts, and an async cancellation request can race with completion.

## Learn More

- [User guide (English)](doc/user_guide.md)
- [用户手册（中文）](doc/user_guide.zh_CN.md)
- [API documentation](https://docs.rs/qubit-tokio-executor)
- [中文 README](README.zh_CN.md)

## Testing

```bash
# Run tests with the default feature set
cargo test

# Run tests with all declared features
cargo test --all-features

# Project CI checks
./ci-check.sh

# Check code coverage
./coverage.sh
```

## License

Copyright (c) 2025 - 2026. Haixing Hu. All rights reserved.

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for the
full license text.

## Contributing

Contributions are welcome. Please follow the Rust API guidelines, keep public
API documentation and tests current, and run `./align-ci.sh` to format code and
`./ci-check.sh` to satisfy CI requirements before submitting a pull request.

## Author

**Haixing Hu** - *Qubit Co. Ltd.*

Repository: [https://github.com/qubit-ltd/rs-tokio-executor](https://github.com/qubit-ltd/rs-tokio-executor)
