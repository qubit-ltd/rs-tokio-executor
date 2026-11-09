# Qubit Tokio Executor User Guide

[中文](user_guide.zh_CN.md) | [README](../README.md) | [API documentation](https://docs.rs/qubit-tokio-executor)

Applies to `qubit-tokio-executor` 0.12. This guide is for Rust application
authors who already run Tokio and want Qubit's task-result and service-lifecycle
abstractions for blocking callables or asynchronous futures.

## Conceptual Model

The crate binds each executor or service to a `tokio::runtime::Handle` supplied
at construction. Submission can use that captured runtime even when the caller
is executing in another runtime or a plain thread, as long as the bound runtime
remains alive. Submission location and runtime progress are separate: the
handle selects where work is submitted, but it does not start or drive that
runtime.

| Work type | Service | Tokio primitive | Result handle |
| --- | --- | --- | --- |
| Synchronous work that may block an OS thread | `TokioExecutorService` | `spawn_blocking` | shared `TaskHandle` or `TokioBlockingTaskHandle` |
| Non-blocking asynchronous work | `TokioIoExecutorService` | `tokio::spawn` | `TokioTaskHandle` |

`shutdown` closes admission and lets accepted work finish. `stop` closes
admission and requests cancellation. A successful submission only means that
the service accepted the work; await its returned handle to learn its result.

## Runtime Lifetime and Driver Progress

Keep the runtime that owns the supplied handle alive until its accepted work
has finished. Async work on that runtime must be driven so Tokio can poll it
and process cancellation. A multi-thread runtime drives its workers while it
is running. A current-thread runtime advances async work only while its owner
is inside `Runtime::block_on`; merely holding its `Handle` does not make
progress. Synchronous callables submitted through `TokioExecutor` and
`TokioExecutorService` run on Tokio's blocking pool and do not need
`block_on` merely to execute. If such a callable waits on async work tied to
the bound runtime, that async work still needs the runtime to be driven.

For example, awaiting a `TokioTaskHandle` inside another runtime polls that
handle but does not drive the bound current-thread runtime. Drive the bound
runtime with its own `block_on` (or keep the application running inside that
runtime) until the task and service finish. `stop` requests aborts, but Tokio
must still run to drop cancelled IO futures and update IO service accounting.
Likewise, awaiting `await_termination()` from a different runtime does not
advance a bound current-thread runtime. Do not synchronously wait on a task
from the thread that must drive that task; use async waiting while the runtime
continues to run.

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
qubit-executor = "0.9.1"
tokio = { version = "1.54", features = ["macros", "rt-multi-thread", "time"] }
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

`TokioBlockingTaskHandle::cancellation_handle` is available in
`qubit-tokio-executor` 0.12.0 and later. Save this endpoint before consuming
the task handle into its future when cancellation may be needed during the
wait. The endpoint and its clones share the task's cancellation path; dropping
an endpoint does not cancel the task. Tokio still cannot interrupt a blocking
closure after it starts. Releases before 0.12.0 do not expose this endpoint;
call `handle.cancel()` before awaiting the handle in those versions.

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
first submission attempt**. Use the inherent `try_submit`,
`try_submit_callable`, `try_submit_tracked_callable`, or `try_spawn` method. A
rejection contains a `RejectedTask<T>`; call `into_parts()` to recover the
reason and the same unrun task. Keep that task in a local `Option` or variable,
wait for `changed()` only after `SubmissionError::Saturated`, then retry with
the recovered task. The notification is only a hint: it reserves no slot,
promises no fairness, and its generation is not a count of available capacity.
Return or otherwise handle `Shutdown` and other errors instead of retrying
them. If the retry future itself is cancelled while waiting, its local task is
dropped with that future; keep ownership outside the cancellable operation if
the application must retain it across cancellation. `RejectedTask` formats
only the rejection reason with `Debug`, so task captures are not printed and
the task type need not implement `Debug`.

The following complete programs are also kept as standalone files: [callable retry](../examples/capacity_retry_callable.rs) and [future retry](../examples/capacity_retry_future.rs). Each subscribes before its first attempt, occupies a capacity-one service with a channel-controlled task, requires the first attempt to be `Saturated`, recovers the original task through `RejectedTask::into_parts()`, and retries after release. Both assert result 42 and one execution before shutdown and termination.

Callable retry program:

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

Future retry program:

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
handle, so `handle.await` cannot be wrapped in a timeout and then reused. In
version 0.12.0 and later, retain a cancellation endpoint before consuming it
if queued cancellation may be needed while waiting; in earlier versions, call
`handle.cancel()` before consuming it. Service-level termination waits are
also available. Dropping `TokioBlockingTaskHandle` or its wait
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

Submission errors describe service admission, not Tokio runtime liveness.
`TokioExecutor::call` does not return `SubmissionError::Shutdown` when its
bound runtime has closed. A blocking task may still be accepted and its handle
may report `Dropped` if Tokio discards it before its runner starts. The
services reject new work after their own `shutdown` or `stop`; a runtime that
has closed independently does not change that admission decision. For an IO
task accepted after runtime shutdown, the handle may report `Cancelled` if
Tokio discards the task. Shutdown races can produce different final outcomes;
use the returned handle as the authority.

| Situation | Submission result | Observation |
| --- | --- | --- |
| The service has shut down | `Shutdown` | The task is not accepted. |
| The service is running but at capacity | `Saturated` | The task is not accepted; it can be retained and retried. |
| The runtime is closed and a blocking task is submitted | Accepted (`Ok(handle)`) | In the covered case where Tokio drops it before the runner starts, the handle reports `Dropped`. |
| The runtime is closed and an IO task is submitted | Accepted (`Ok(handle)`) | In the covered case where Tokio destroys the task, the handle reports `Cancelled`. |

These runtime-shutdown rows describe covered submission scenarios, not the
only possible outcome of every shutdown race. A blocking task that has started
can continue until it returns. The final handle result is authoritative.

The `try_*` methods return `RejectedTask<T>` when admission fails. Inspect
`reason()` and use `into_parts()` if the task should be retained or retried.
The older `submit_*` methods keep their existing `SubmissionError` result and
consume rejected tasks.

`stop` returns a `StopReport`. For the blocking service, `cancelled` counts only
queued tasks successfully cancelled before their closure started; active blocking
closures remain running until they return. For the IO service, `cancelled` counts
async tasks whose Tokio abort handle was signalled.

## Troubleshooting

| Symptom | Check | Resolution |
| --- | --- | --- |
| Submission is rejected | The service may be shut down (`Shutdown`) or at capacity (`Saturated`) | For saturation, subscribe before submitting, wait for a change, then retry. |
| A task is accepted after the bound runtime has shut down | Submission checks service admission, not runtime liveness | Inspect the returned handle; a blocking task may be `Dropped` and an IO task may be `Cancelled` if Tokio discards it before execution. |
| An accepted IO task does not run | The bound current-thread runtime may be idle | Drive that runtime with its owner’s `Runtime::block_on`; awaiting from another runtime is not a driver. |
| The service remains active after `stop` | Tokio may not have processed task aborts yet | Keep the bound runtime driven while awaiting service termination. |
| Async work stalls other tasks | A blocking operation may be inside an IO future | Move that operation to `TokioExecutorService`. |
| Blocking service does not terminate | A blocking closure may already be running | Let the closure return; Tokio cannot forcibly abort it. |
| Cancellation did not determine the final result | Completion and cancellation can race | Await the returned handle. |

## Limitations and Best Practices

Keep the bound runtime alive until all desired work is complete, and keep it
driven while async work or async cancellation cleanup needs progress. Pure
synchronous callables run on Tokio's blocking pool without `block_on`, though
any async work they await still needs a driver. Do not block a runtime driver
thread waiting synchronously for tasks that need it to run. Do not treat
acceptance as task success. Choose the execution domain from the work's
blocking behavior, and use graceful `shutdown` when accepted tasks should
finish.
`TokioExecutor` is available for strategy-level blocking execution, but use the
service types when you need lifecycle and task accounting.
Do not wait for a service to terminate from a task that is still counted by that
same service. This applies to both blocking and IO services: even after shutdown
is requested, termination requires that task itself to finish. A tracked task
may request shutdown, but termination should be awaited by a coordinator outside
the service's tracked tasks.

## Further Reading

- [README](../README.md)
- [中文用户手册](user_guide.zh_CN.md)
- [API documentation](https://docs.rs/qubit-tokio-executor)
- [Concurrency protocol](concurrency-design.md)
