# Qubit Tokio Executor

[![CircleCI](https://circleci.com/gh/qubit-ltd/rs-tokio-executor.svg?style=shield)](https://circleci.com/gh/qubit-ltd/rs-tokio-executor)
[![Coverage Status](https://coveralls.io/repos/github/qubit-ltd/rs-tokio-executor/badge.svg?branch=main)](https://coveralls.io/github/qubit-ltd/rs-tokio-executor?branch=main)
[![Crates.io](https://img.shields.io/crates/v/qubit-tokio-executor.svg?color=blue)](https://crates.io/crates/qubit-tokio-executor)
[![Rust](https://img.shields.io/badge/rust-1.94+-blue.svg?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![中文文档](https://img.shields.io/badge/文档-中文版-blue.svg)](README.zh_CN.md)

Tokio-backed executor services for Rust.

## Overview

Qubit Tokio Executor adapts the Qubit executor abstractions to Tokio. It provides
executor-service semantics for blocking functions submitted through
`tokio::task::spawn_blocking`, and a separate service for async futures submitted
through `tokio::spawn`.

The crate does not create or own a Tokio runtime. Calls that spawn Tokio work
must be made from inside an existing Tokio runtime configured by the
application.

## Features

- `TokioExecutor` for strategy-level Tokio blocking execution.
- `TokioExecutorService` for managed blocking work backed by `spawn_blocking`.
- `TokioBlockingExecutorService` alias for naming the Tokio blocking domain explicitly.
- `TokioIoExecutorService` for async `Future` work backed by `tokio::spawn`.
- `TokioTaskHandle` for awaiting, cancellation, completion checks, and task-result reporting.
- `TokioExecution` as the execution carrier used by Tokio-backed executor APIs.
- Shared `ExecutorService`, `RejectedExecution`, and `ShutdownReport` re-exports for convenient imports.

## Runtime Requirement

This crate assumes a Tokio runtime already exists. In applications, enable the
Tokio runtime features you need in `Cargo.toml`:

```toml
[dependencies]
qubit-tokio-executor = "0.1.0"
tokio = { version = "1.52", features = ["macros", "rt-multi-thread", "time"] }
```

If a method internally uses `tokio::spawn` or `tokio::task::spawn_blocking`, it
must be called while a Tokio runtime is entered. Calling it without a runtime is
a Tokio usage error.

## Blocking vs IO Tasks

Use `TokioExecutorService` or `TokioBlockingExecutorService` for synchronous
functions that may block an OS thread. These tasks run through Tokio's blocking
pool and should not be used for async IO futures.

Use `TokioIoExecutorService` for non-blocking futures. These tasks run on
Tokio's async scheduler and should not perform long blocking operations inside
the future body.

## Shutdown and Cancellation

A successful `submit` or `spawn` means only that the service accepted the task.
The task result is reported through `TokioTaskHandle`.

`shutdown` rejects new tasks and lets accepted tasks finish. `shutdown_now`
rejects new tasks and requests cancellation or abort for tracked Tokio tasks.
Async IO tasks are aborted through Tokio abort handles; blocking tasks submitted
through Tokio can be marked cancelled from the service side, but already running
blocking code cannot be forcibly stopped by Rust.

## Quick Start

### Tokio blocking work

```rust
use std::io;

use qubit_tokio_executor::{ExecutorService, TokioExecutorService};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let service = TokioExecutorService::new();
let handle = service.submit_callable(|| Ok::<usize, io::Error>(40 + 2))?;
assert_eq!(handle.await?, 42);
service.shutdown();
service.await_termination().await;
# Ok(())
# }
```

### Async IO futures

```rust
use std::io;

use qubit_tokio_executor::TokioIoExecutorService;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let service = TokioIoExecutorService::new();
let handle = service.spawn(async { Ok::<usize, io::Error>(6 * 7) })?;
assert_eq!(handle.await?, 42);
service.shutdown();
service.await_termination().await;
# Ok(())
# }
```

## Choosing an Executor

Use `qubit-tokio-executor` when your application is already Tokio-based and you
need execution services that integrate with Tokio scheduling. Use
`qubit-thread-pool` for runtime-independent OS-thread execution, and use
`qubit-rayon-executor` for CPU-bound Rayon work.

For application-level wiring across blocking, CPU-bound, Tokio blocking, and
async IO domains, use `qubit-execution-services`.

## Testing

A minimal local run:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

To mirror what continuous integration enforces, run the repository scripts from
the project root: `./align-ci.sh` brings local tooling and configuration in line
with CI, then `./ci-check.sh` runs the same checks the pipeline uses. For test
coverage, use `./coverage.sh` to generate or open reports.

## Contributing

Issues and pull requests are welcome.

- Open an issue for bug reports, design questions, or larger feature proposals when it helps align on direction.
- Keep pull requests scoped to one behavior change, fix, or documentation update when practical.
- Before submitting, run `./align-ci.sh` and then `./ci-check.sh` so your branch matches CI rules and passes the same checks as the pipeline.
- Add or update tests when you change runtime behavior, and update this README or public rustdoc when user-visible API behavior changes.
- If you change runtime, shutdown, or cancellation behavior, cover both blocking and async IO services when applicable.

By contributing, you agree to license your contributions under the [Apache License, Version 2.0](LICENSE), the same license as this project.

## License

Copyright © 2026 Haixing Hu, Qubit Co. Ltd.

This project is licensed under the [Apache License, Version 2.0](LICENSE). See the `LICENSE` file in the repository for the full text.

## Author

**Haixing Hu** — Qubit Co. Ltd.

| | |
| --- | --- |
| **Repository** | [github.com/qubit-ltd/rs-tokio-executor](https://github.com/qubit-ltd/rs-tokio-executor) |
| **Documentation** | [docs.rs/qubit-tokio-executor](https://docs.rs/qubit-tokio-executor) |
| **Crate** | [crates.io/crates/qubit-tokio-executor](https://crates.io/crates/qubit-tokio-executor) |
