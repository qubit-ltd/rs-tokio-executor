# Qubit Tokio Executor User Guide

[中文](user_guide.zh_CN.md) | [README](../README.md) | [API documentation](https://docs.rs/qubit-tokio-executor)

Applies to `qubit-tokio-executor` 0.10. This guide is for Rust application
authors who already run Tokio and want Qubit's task-result and service-lifecycle
abstractions for blocking callables or asynchronous futures.

## Conceptual Model

The crate binds each executor or service to a `tokio::runtime::Handle` supplied
at construction. Submission can use that captured runtime even when the caller
is executing in another runtime or a plain thread, as long as the bound runtime
remains alive.

| Work type | Service | Tokio primitive | Result handle |
| --- | --- | --- | --- |
| Synchronous work that may block an OS thread | `TokioExecutorService` | `spawn_blocking` | shared `TaskHandle` or `TokioBlockingTaskHandle` |
| Non-blocking asynchronous work | `TokioIoExecutorService` | `tokio::spawn` | `TokioTaskHandle` |

`shutdown` closes admission and lets accepted work finish. `stop` closes
admission and requests cancellation. A successful submission only means that
the service accepted the work; await its returned handle to learn its result.

## Scenario

Suppose an HTTP application must parse a CPU-heavy document while continuing to
serve asynchronous requests. Its success criterion is a parsed value, followed
by a clean shutdown that waits for accepted work rather than silently dropping
it. The blocking service is the appropriate boundary because the parser can
occupy an OS thread.

## Installation and Minimal Configuration

```toml
[dependencies]
qubit-tokio-executor = "0.10"
qubit-executor = "0.8"
tokio = { version = "1.53", features = ["macros", "rt-multi-thread", "time"] }
```

This crate does not start a runtime. Construct the service inside an existing
Tokio application, or pass a handle obtained from the runtime the application
owns.

## Core Workflow

Import `ExecutorService` to call the blocking-service trait methods. Submit the
callable, await its result, then request graceful shutdown and await service
termination:

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

The `Result` from `submit_callable` reports whether the service accepted the
work; the result produced by awaiting the handle reports whether the callable
itself succeeded.

## Advanced Usage

For non-blocking async work, submit a future to `TokioIoExecutorService`:

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

Use `submit_tracked_callable` when queued blocking work must be individually
cancelled. `TokioBlockingTaskHandle::cancel` can cancel only before the blocking
closure starts. `TokioTaskHandle::cancel` sends Tokio an abort request; await
the handle to observe whether cancellation or completion won the race.

Each service accepts at most 1024 unfinished tasks by default. The blocking
service counts queued and running tasks; the IO service also counts accepted
futures that Tokio has not polled yet. At capacity, submission returns
`SubmissionError::Saturated`. Use `TokioExecutorService::with_task_capacity` or
`TokioIoExecutorService::with_task_capacity` with a `NonZeroUsize` to choose a
different finite limit. Completion and cancellation release capacity when the
underlying task is dropped. A blocking closure that has started keeps its slot
until it returns. Shutdown is checked before capacity, so submissions after
shutdown return `SubmissionError::Shutdown` even while all slots are occupied.

## Errors and Diagnostics

`SubmissionError::Shutdown` means a service no longer accepts work. Check
`is_running`, `is_not_running`, or `lifecycle` when coordinating callers. An
accepted task can still return its own error, and an async task can report a
cancellation or panic through its final task result.

`stop` returns a `StopReport`. For the blocking service, `cancelled` counts only
queued tasks successfully cancelled before their closure started; active blocking
closures remain running until they return. For the IO service, `cancelled` counts
async tasks whose Tokio abort handle was signalled.

## Troubleshooting

| Symptom | Check | Resolution |
| --- | --- | --- |
| Submission is rejected | The service may have been shut down or stopped | Submit while the service is running. |
| Async work stalls other tasks | A blocking operation may be inside an IO future | Move that operation to `TokioExecutorService`. |
| Blocking service does not terminate | A blocking closure may already be running | Let the closure return; Tokio cannot forcibly abort it. |
| Cancellation did not determine the final result | Completion and cancellation can race | Await the returned handle. |

## Limitations and Best Practices

Keep the bound runtime alive until all desired work is complete. Do not treat
acceptance as task success. Choose the execution domain from the work's blocking
behavior, and use graceful `shutdown` when accepted tasks should finish.
`TokioExecutor` is available for strategy-level blocking execution, but use the
service types when you need lifecycle and task accounting.

## Further Reading

- [README](../README.md)
- [中文用户手册](user_guide.zh_CN.md)
- [API documentation](https://docs.rs/qubit-tokio-executor)
