// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::Future;
use std::future::IntoFuture;
use std::io;
use std::io::Read;
use std::num::NonZeroUsize;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::task::Wake;
use std::task::Waker;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use qubit_executor::service::ExecutorService;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioExecutorService;
use qubit_tokio_executor::TokioIoExecutorService;

const CHILD_MODE: &str = "QUBIT_REENTRANT_WAKER_CHILD";
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSED_RUNTIME_CHILD: &str = "QUBIT_CLOSED_RUNTIME_CAPACITY_CHILD";

struct ReentrantWake {
    invoked: Arc<AtomicBool>,
    reenter: Box<dyn Fn() + Send + Sync>,
}

impl Wake for ReentrantWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if !self.invoked.swap(true, Ordering::SeqCst) {
            (self.reenter)();
        }
    }
}

#[derive(Clone, Copy)]
enum CloseAction {
    Shutdown,
    Stop,
}

#[derive(Clone, Copy)]
enum Reentry {
    Submit,
    Close(CloseAction),
}

#[test]
fn test_reentrant_waker_child() {
    if std::env::var_os(CHILD_MODE).is_none() {
        return;
    }
    let kind = std::env::var("QUBIT_REENTRANT_WAKER_KIND").expect("case kind");
    let path = std::env::var("QUBIT_REENTRANT_WAKER_PATH").expect("wait path");
    let outer = parse_close(
        std::env::var("QUBIT_REENTRANT_WAKER_OUTER")
            .expect("outer action")
            .as_str(),
    );
    let reentry = match std::env::var("QUBIT_REENTRANT_WAKER_REENTRY")
        .expect("reentry action")
        .as_str()
    {
        "submit" => Reentry::Submit,
        value => Reentry::Close(parse_close(value)),
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime.block_on(async {
        let invoked = Arc::new(AtomicBool::new(false));
        match kind.as_str() {
            "blocking" if path == "capacity" => run_blocking_case(&runtime, outer, reentry, Arc::clone(&invoked)).await,
            "io" if path == "capacity" => run_io_case(&runtime, outer, reentry, Arc::clone(&invoked)).await,
            "blocking" if path == "termination" => {
                run_blocking_termination_case(&runtime, outer, reentry, Arc::clone(&invoked)).await
            }
            "io" if path == "termination" => {
                run_io_termination_case(&runtime, outer, reentry, Arc::clone(&invoked)).await
            }
            _ => panic!("unknown service kind: {kind}"),
        }
        assert!(invoked.load(Ordering::SeqCst), "registered Waker should run");
    });
}

fn parse_close(value: &str) -> CloseAction {
    match value {
        "shutdown" => CloseAction::Shutdown,
        "stop" => CloseAction::Stop,
        _ => panic!("unknown close action: {value}"),
    }
}

async fn run_blocking_case(
    runtime: &tokio::runtime::Runtime,
    outer: CloseAction,
    reentry: Reentry,
    invoked: Arc<AtomicBool>,
) {
    let service = TokioExecutorService::with_task_capacity(runtime.handle().clone(), NonZeroUsize::new(1).unwrap());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let blocker = service
        .submit_callable(move || {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
            Ok::<(), io::Error>(())
        })
        .expect("blocking task should be accepted");
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("blocking task should start");
    let mut changes = service.capacity_changes();
    let reentrant_service = service.clone();
    let waker = Waker::from(Arc::new(ReentrantWake {
        invoked,
        reenter: Box::new(move || match reentry {
            Reentry::Submit => {
                let result = reentrant_service.submit_callable(|| Ok::<(), io::Error>(()));
                assert!(matches!(result, Err(SubmissionError::Shutdown)));
            }
            Reentry::Close(action) => close_blocking(&reentrant_service, action),
        }),
    }));
    close_after_poll(&mut changes, &waker, || close_blocking(&service, outer));
    release_tx.send(()).expect("blocking task should still be waiting");
    blocker.into_future().await.expect("blocking task should finish");
}

async fn run_io_case(
    runtime: &tokio::runtime::Runtime,
    outer: CloseAction,
    reentry: Reentry,
    invoked: Arc<AtomicBool>,
) {
    let service = TokioIoExecutorService::with_task_capacity(runtime.handle().clone(), NonZeroUsize::new(1).unwrap());
    let blocker = service
        .spawn(std::future::pending::<Result<(), io::Error>>())
        .expect("IO task should be accepted");
    let mut changes = service.capacity_changes();
    let reentrant_service = service.clone();
    let waker = Waker::from(Arc::new(ReentrantWake {
        invoked,
        reenter: Box::new(move || match reentry {
            Reentry::Submit => {
                let result = reentrant_service.spawn(async { Ok::<(), io::Error>(()) });
                assert!(matches!(result, Err(SubmissionError::Shutdown)));
            }
            Reentry::Close(action) => close_io(&reentrant_service, action),
        }),
    }));
    close_after_poll(&mut changes, &waker, || close_io(&service, outer));
    drop(blocker);
}

async fn run_blocking_termination_case(
    runtime: &tokio::runtime::Runtime,
    outer: CloseAction,
    reentry: Reentry,
    invoked: Arc<AtomicBool>,
) {
    let service = TokioExecutorService::with_task_capacity(runtime.handle().clone(), NonZeroUsize::new(1).unwrap());
    let reentrant_service = service.clone();
    let waker = Waker::from(Arc::new(ReentrantWake {
        invoked,
        reenter: Box::new(move || match reentry {
            Reentry::Submit => {
                let result = reentrant_service.submit_callable(|| Ok::<(), io::Error>(()));
                assert!(matches!(result, Err(SubmissionError::Shutdown)));
            }
            Reentry::Close(action) => close_blocking(&reentrant_service, action),
        }),
    }));
    let mut terminated = service.await_termination();
    poll_termination_then_close(terminated.as_mut(), &waker, || close_blocking(&service, outer));
}

async fn run_io_termination_case(
    runtime: &tokio::runtime::Runtime,
    outer: CloseAction,
    reentry: Reentry,
    invoked: Arc<AtomicBool>,
) {
    let service = TokioIoExecutorService::with_task_capacity(runtime.handle().clone(), NonZeroUsize::new(1).unwrap());
    let reentrant_service = service.clone();
    let waker = Waker::from(Arc::new(ReentrantWake {
        invoked,
        reenter: Box::new(move || match reentry {
            Reentry::Submit => {
                let result = reentrant_service.spawn(async { Ok::<(), io::Error>(()) });
                assert!(matches!(result, Err(SubmissionError::Shutdown)));
            }
            Reentry::Close(action) => close_io(&reentrant_service, action),
        }),
    }));
    let mut terminated = Box::pin(service.await_termination());
    poll_termination_then_close(terminated.as_mut(), &waker, || close_io(&service, outer));
}

fn close_after_poll<F>(changes: &mut tokio::sync::watch::Receiver<u64>, waker: &Waker, close: F)
where
    F: FnOnce(),
{
    let mut changed = Box::pin(changes.changed());
    let mut context = Context::from_waker(waker);
    assert!(matches!(changed.as_mut().poll(&mut context), Poll::Pending));
    close();
}

fn poll_termination_then_close<Fut, F>(terminated: std::pin::Pin<&mut Fut>, waker: &Waker, close: F)
where
    Fut: Future<Output = ()> + ?Sized,
    F: FnOnce(),
{
    let mut context = Context::from_waker(waker);
    assert!(matches!(terminated.poll(&mut context), Poll::Pending));
    close();
}

fn close_blocking(service: &TokioExecutorService, action: CloseAction) {
    match action {
        CloseAction::Shutdown => service.shutdown(),
        CloseAction::Stop => {
            let _ = service.stop();
        }
    }
}

fn close_io(service: &TokioIoExecutorService, action: CloseAction) {
    match action {
        CloseAction::Shutdown => service.shutdown(),
        CloseAction::Stop => {
            let _ = service.stop();
        }
    }
}

#[test]
fn test_reentrant_waker_watchdog_matrix() {
    if std::env::var_os(CHILD_MODE).is_some() {
        return;
    }
    for kind in ["blocking", "io"] {
        for path in ["capacity", "termination"] {
            for outer in ["shutdown", "stop"] {
                for reentry in ["submit", "shutdown", "stop"] {
                    let mut child = Command::new(std::env::current_exe().expect("test executable path"))
                        .args(["--exact", "test_reentrant_waker_child", "--nocapture"])
                        .env(CHILD_MODE, "1")
                        .env("QUBIT_REENTRANT_WAKER_KIND", kind)
                        .env("QUBIT_REENTRANT_WAKER_PATH", path)
                        .env("QUBIT_REENTRANT_WAKER_OUTER", outer)
                        .env("QUBIT_REENTRANT_WAKER_REENTRY", reentry)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .expect("child test should start");
                    let deadline = Instant::now() + WATCHDOG_TIMEOUT;
                    loop {
                        if let Some(status) = child.try_wait().expect("child status should be readable") {
                            assert!(
                                status.success(),
                                "{kind}/{path}/{outer}/{reentry} child exited with {status}"
                            );
                            break;
                        }
                        if Instant::now() >= deadline {
                            child.kill().expect("timed-out child should be killed");
                            let status = child.wait().expect("timed-out child should be reaped");
                            panic!(
                                "{kind}/{path}/{outer}/{reentry} Waker reentry timed out; child was killed and reaped ({status})"
                            );
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        }
    }
}

#[test]
fn test_closed_runtime_capacity_waker_child() {
    if std::env::var_os(CLOSED_RUNTIME_CHILD).is_none() {
        return;
    }

    let submission = std::env::var("QUBIT_CLOSED_RUNTIME_SUBMISSION").expect("submission kind");
    let reentry = std::env::var("QUBIT_CLOSED_RUNTIME_REENTRY").expect("reentry action");
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let service = TokioExecutorService::new(runtime.handle().clone());
    drop(runtime);

    let mut changes = service.capacity_changes();
    let invoked = Arc::new(AtomicBool::new(false));
    let reentrant_service = service.clone();
    let waker = Waker::from(Arc::new(ReentrantWake {
        invoked: Arc::clone(&invoked),
        reenter: Box::new(move || match reentry.as_str() {
            "submit" => {
                let handle = reentrant_service
                    .try_submit_callable(|| Ok::<(), io::Error>(()))
                    .expect("reentrant submission should remain admissible");
                assert!(matches!(handle.get(), Err(qubit_executor::TaskExecutionError::Dropped)));
            }
            "shutdown" => reentrant_service.shutdown(),
            "stop" => drop(reentrant_service.stop()),
            _ => panic!("unknown reentry action: {reentry}"),
        }),
    }));
    let mut changed = Box::pin(changes.changed());
    assert!(matches!(
        changed.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));

    let invoked_task = Arc::new(AtomicBool::new(false));
    let invoked_in_task = Arc::clone(&invoked_task);
    match submission.as_str() {
        "runnable" => {
            service
                .try_submit::<_, io::Error>(move || {
                    invoked_in_task.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .expect("runnable should be accepted");
        }
        "callable" => {
            let handle = service
                .try_submit_callable(move || {
                    invoked_in_task.store(true, Ordering::SeqCst);
                    Ok::<(), io::Error>(())
                })
                .expect("callable should be accepted");
            assert!(matches!(handle.get(), Err(qubit_executor::TaskExecutionError::Dropped)));
        }
        "tracked" => {
            let handle = service
                .try_submit_tracked_callable(move || {
                    invoked_in_task.store(true, Ordering::SeqCst);
                    Ok::<(), io::Error>(())
                })
                .expect("tracked callable should be accepted");
            assert!(matches!(handle.get(), Err(qubit_executor::TaskExecutionError::Dropped)));
        }
        _ => panic!("unknown submission kind: {submission}"),
    }

    assert!(invoked.load(Ordering::SeqCst), "capacity watcher should be woken");
    assert!(
        !invoked_task.load(Ordering::SeqCst),
        "closed runtime must not run accepted work"
    );
    assert_eq!(service.stats().queued + service.stats().running, 0);
    service.shutdown();
    assert!(service.wait_termination_timeout(Duration::from_secs(2)));
}

#[test]
fn test_closed_runtime_capacity_waker_watchdog_matrix() {
    if std::env::var_os(CLOSED_RUNTIME_CHILD).is_some() {
        return;
    }

    for submission in ["runnable", "callable", "tracked"] {
        for reentry in ["submit", "shutdown", "stop"] {
            let mut child = Command::new(std::env::current_exe().expect("test executable path"))
                .args(["--exact", "test_closed_runtime_capacity_waker_child", "--nocapture"])
                .env(CLOSED_RUNTIME_CHILD, "1")
                .env("QUBIT_CLOSED_RUNTIME_SUBMISSION", submission)
                .env("QUBIT_CLOSED_RUNTIME_REENTRY", reentry)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("child test should start");
            let deadline = Instant::now() + WATCHDOG_TIMEOUT;
            loop {
                if let Some(status) = child.try_wait().expect("child status should be readable") {
                    let mut stderr = String::new();
                    child
                        .stderr
                        .take()
                        .expect("child stderr should exist")
                        .read_to_string(&mut stderr)
                        .expect("stderr should read");
                    assert!(
                        status.success(),
                        "{submission}/{reentry} child exited with {status}: {stderr}"
                    );
                    break;
                }
                if Instant::now() >= deadline {
                    child.kill().expect("timed-out child should be killed");
                    let status = child.wait().expect("timed-out child should be reaped");
                    let mut stderr = String::new();
                    child
                        .stderr
                        .take()
                        .expect("child stderr should exist")
                        .read_to_string(&mut stderr)
                        .expect("child stderr should read");
                    panic!(
                        "{submission}/{reentry} Waker reentry timed out; child was killed and reaped ({status}): {stderr}"
                    );
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
