# Qubit Tokio Executor User Guide

[中文](user_guide.zh_CN.md) | [README](../README.md) | [API documentation](https://docs.rs/qubit-tokio-executor)

Applies to `qubit-tokio-executor` 0.12. This guide is for Rust application
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
by shutdown and an explicit wait for service termination so accepted work can
finish rather than being silently dropped. The blocking service is the
appropriate boundary because the parser can occupy an OS thread.

## Installation and Minimal Configuration

```toml
[dependencies]
qubit-tokio-executor = "0.12"
qubit-executor = "0.9"
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
different finite limit. Completion releases capacity when the service accounting
finishes the task. For a queued blocking task, successful
cancellation immediately returns logical capacity, but neither cancellation
nor service termination guarantees Tokio has destroyed the queued closure and
its captures. A started blocking closure cannot be aborted and keeps its slot
until it returns. For IO tasks, capacity returns after Tokio processes abort and
drops the guarded future. The limit bounds logically unfinished tasks, not
captured resources that may remain in Tokio's blocking queue. Shutdown is checked before capacity, so submissions after
shutdown return `SubmissionError::Shutdown` even while all slots are occupied.

Use `service.stats()` to diagnose pressure: the blocking service reports
`task_capacity`, `queued`, and `running`; the IO service reports
`task_capacity` and `accepted_unfinished`, including futures Tokio has not yet
polled. Both include `lifecycle`. These values are a snapshot for diagnosis,
not a reservation or guarantee that the next submission will be accepted.

For retry after saturation, subscribe with `capacity_changes()` **before the
first submission attempt**. After `SubmissionError::Saturated`, wait for
`changed()` and retry; return other errors. The notification is only a hint:
it reserves no slot, promises no fairness, and its generation is not a count of
available capacity. Submission consumes its task by value even when rejected,
so the original callable or future is not returned. A retry must retain the
callable in an ownership slot such as `Arc<Mutex<Option<C>>>`; a wrapper closure
takes it when run, releases the mutex, and only then calls it. Do not assume an
arbitrary future can be cloned or safely run twice. Cancelling the wait before
acceptance drops the current wrapper. Keep the ownership slot outside the
cancelled retry operation if the application must recover the original task.
After an accepted wrapper takes the task from the slot, that task has been
consumed and cannot be retried. `Shutdown` and other errors end the loop
immediately so the caller can decide what to do with any task still in its
slot.

The following complete programs are also kept as standalone files: [callable retry](../examples/capacity_retry_callable.rs) and [future retry](../examples/capacity_retry_future.rs). Each subscribes before its first attempt, occupies a capacity-one service with a channel-controlled task, requires the first attempt to be `Saturated`, and retries after release. The future version stores `Pin<Box<F>>` in `Arc<Mutex<Option<_>>>`, takes it on first poll, drops the lock, and then awaits it. Both assert result 42 and one execution before shutdown and termination.

Callable retry program:

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

Future retry program:

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

To time out while waiting on an IO task without losing the handle, borrow it
with `timeout`:

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

`timeout` stops waiting; because it borrows `&mut handle`, the task handle is
still available for an explicit cancellation request and final-result check.
Dropping a `TokioTaskHandle` detaches its Tokio task rather than cancelling it.
Dropping a wait future does not request cancellation either. `shutdown` only
closes admission and lets accepted work finish; `stop` also requests task
cancellation, which may race with completion. The awaited result is authoritative.

The blocking handle differs: its `IntoFuture` implementation consumes the
handle, so `handle.await` cannot be wrapped in a timeout and then reused. Call
`handle.cancel()` before consuming it if queued cancellation is desired, or use
service-level termination waits. Dropping `TokioBlockingTaskHandle` or its wait
future does not stop a closure; explicit cancellation and service `stop` can
cancel queued work only. Started blocking closures always run until they
return. Dropping either kind of handle also means the caller gives up that
result-observation endpoint.

## Errors and Diagnostics

`SubmissionError::Shutdown` means a service no longer accepts work;
`SubmissionError::Saturated` means its unfinished-task capacity is full. Check
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
| Submission is rejected | The service may be shut down (`Shutdown`) or at capacity (`Saturated`) | For saturation, subscribe before submitting, wait for a change, then retry. |
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
