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
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::task::Wake;
use std::task::Waker;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use qubit_executor::TaskExecutionError;
use qubit_executor::TryGet;
use qubit_executor::service::ExecutorService;
use qubit_executor::service::SubmissionError;
use qubit_executor::task::spi::TaskResultHandle;
use qubit_tokio_executor::TokioExecutorService;
use qubit_tokio_executor::TokioIoExecutorService;

const CHILD_MODE: &str = "QUBIT_REENTRANT_WAKER_CHILD";
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSED_RUNTIME_CHILD: &str = "QUBIT_CLOSED_RUNTIME_CAPACITY_CHILD";
const QUEUED_STOP_CHILD: &str = "QUBIT_QUEUED_STOP_CHILD";

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

struct ReleaseBlockingWorker(Option<std::sync::mpsc::Sender<()>>);

impl Drop for ReleaseBlockingWorker {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StopNotificationObservation {
    lifecycle: String,
    result: String,
}

fn result_observation_waker<H, R, E>(
    label: &'static str,
    lifecycle: Arc<dyn Fn() -> String + Send + Sync>,
    handle: Arc<Mutex<Option<H>>>,
    observations: Arc<Mutex<Vec<(&'static str, StopNotificationObservation)>>>,
    invoked: Arc<AtomicBool>,
) -> Waker
where
    H: TaskResultHandle<R, E> + Send + 'static,
    R: Send + 'static,
    E: Send + 'static,
{
    Waker::from(Arc::new(ReentrantWake {
        invoked,
        reenter: Box::new(move || {
            let lifecycle = lifecycle();
            let mut handle_guard = handle.lock().expect("result handle lock should be healthy");
            let result = match handle_guard.take() {
                Some(result_handle) => match result_handle.try_get() {
                    TryGet::Ready(Err(TaskExecutionError::Cancelled)) => "cancelled".to_owned(),
                    TryGet::Ready(Ok(_)) => "success".to_owned(),
                    TryGet::Ready(Err(_)) => "other-error".to_owned(),
                    TryGet::Pending(pending) => {
                        *handle_guard = Some(pending);
                        "pending".to_owned()
                    }
                },
                None => "already-consumed".to_owned(),
            };
            observations
                .lock()
                .expect("observation lock should be healthy")
                .push((label, StopNotificationObservation { lifecycle, result }));
        }),
    }))
}

async fn finish_queued_stop_case<H, R, E>(
    service: &TokioExecutorService,
    external_worker: tokio::task::JoinHandle<()>,
    release_worker: ReleaseBlockingWorker,
    handle: H,
    user_called: Arc<AtomicBool>,
    tracked: bool,
) where
    H: TaskResultHandle<R, E> + Send + 'static,
    R: Send + 'static,
    E: Send + 'static,
{
    let observations = Arc::new(Mutex::new(Vec::new()));
    let handle = Arc::new(Mutex::new(Some(handle)));
    let lifecycle_service = service.clone();
    let lifecycle: Arc<dyn Fn() -> String + Send + Sync> =
        Arc::new(move || format!("{:?}", lifecycle_service.lifecycle()));
    let capacity_waker = result_observation_waker(
        "capacity",
        Arc::clone(&lifecycle),
        Arc::clone(&handle),
        Arc::clone(&observations),
        Arc::new(AtomicBool::new(false)),
    );
    let termination_waker = result_observation_waker(
        "termination",
        lifecycle,
        Arc::clone(&handle),
        Arc::clone(&observations),
        Arc::new(AtomicBool::new(false)),
    );
    let mut capacity = service.capacity_changes();
    let mut changed = Box::pin(capacity.changed());
    assert!(matches!(
        changed.as_mut().poll(&mut Context::from_waker(&capacity_waker)),
        Poll::Pending
    ));
    let mut terminated = Box::pin(service.await_termination());
    assert!(matches!(
        terminated.as_mut().poll(&mut Context::from_waker(&termination_waker)),
        Poll::Pending
    ));

    let report = service.stop();
    assert_eq!(report.queued, 1);
    assert_eq!(report.running, 0);
    assert_eq!(report.cancelled, 1);
    assert_eq!(service.stop().cancelled, 0, "repeated stop must not count twice");
    drop(release_worker);
    tokio::time::timeout(WATCHDOG_TIMEOUT, external_worker)
        .await
        .expect("external blocking worker should be released")
        .expect("external blocking worker should join");
    tokio::time::timeout(WATCHDOG_TIMEOUT, &mut terminated)
        .await
        .expect("service should terminate");

    let observed = observations.lock().expect("observations should be readable").clone();
    assert!(observed.iter().any(|(source, _)| *source == "capacity"));
    assert!(observed.iter().any(|(source, _)| *source == "termination"));
    assert!(
        observed
            .iter()
            .all(|(_, event)| matches!(event.result.as_str(), "pending" | "cancelled" | "already-consumed"))
    );
    if observed.iter().any(|(_, event)| event.result == "already-consumed") {
        assert!(
            observed.iter().any(|(_, event)| event.result == "cancelled"),
            "already-consumed requires another callback to record cancellation"
        );
    }
    assert!(observed.iter().all(|(_, event)| event.lifecycle == "Terminated"));
    let remaining_handle = handle.lock().expect("result handle lock should be healthy").take();
    if let Some(handle) = remaining_handle {
        assert!(matches!(handle.get(), Err(TaskExecutionError::Cancelled)));
    } else {
        assert!(
            observed.iter().any(|(_, event)| event.result == "cancelled"),
            "a callback that consumed the result handle must observe cancellation"
        );
    }
    assert!(!user_called.load(Ordering::SeqCst), "queued user callable must not run");
    assert_eq!(service.stats().queued, 0);
    assert_eq!(service.stats().running, 0);
    assert!(service.is_terminated());
    println!("tracked={tracked} result observations: {observed:?}");
}

#[test]
fn test_queued_blocking_stop_child() {
    if std::env::var_os(QUEUED_STOP_CHILD).is_none() {
        return;
    }
    let tracked = std::env::var("QUBIT_QUEUED_STOP_TRACKED").expect("tracked case flag") == "1";
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should build");

    runtime.block_on(async {
        let (worker_started_tx, worker_started_rx) = std::sync::mpsc::channel();
        let (worker_release_tx, worker_release_rx) = std::sync::mpsc::channel();
        let release_worker = ReleaseBlockingWorker(Some(worker_release_tx));
        let external_worker = runtime.handle().spawn_blocking(move || {
            worker_started_tx.send(()).expect("worker start should be observed");
            let _ = worker_release_rx.recv();
        });
        worker_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("external blocker should occupy the only blocking worker");

        let service = TokioExecutorService::with_task_capacity(
            runtime.handle().clone(),
            NonZeroUsize::new(1).expect("capacity should be nonzero"),
        );
        let user_called = Arc::new(AtomicBool::new(false));
        let called_by_callable = Arc::clone(&user_called);
        if tracked {
            let tracked_handle = service
                .submit_tracked_callable(move || {
                    called_by_callable.store(true, Ordering::SeqCst);
                    Ok::<(), io::Error>(())
                })
                .expect("tracked callable should be accepted into queue");
            finish_queued_stop_case(
                &service,
                external_worker,
                release_worker,
                tracked_handle,
                user_called,
                tracked,
            )
            .await;
        } else {
            let ordinary_handle = service
                .submit_callable(move || {
                    called_by_callable.store(true, Ordering::SeqCst);
                    Ok::<(), io::Error>(())
                })
                .expect("ordinary callable should be accepted into queue");
            finish_queued_stop_case(
                &service,
                external_worker,
                release_worker,
                ordinary_handle,
                user_called,
                tracked,
            )
            .await;
        }
    });
}

#[test]
fn test_queued_blocking_stop_watchdog_matrix() {
    if std::env::var_os(QUEUED_STOP_CHILD).is_some() {
        return;
    }
    for tracked in [false, true] {
        let mut child = Command::new(std::env::current_exe().expect("test executable path"))
            .args(["--exact", "test_queued_blocking_stop_child", "--nocapture"])
            .env(QUEUED_STOP_CHILD, "1")
            .env("QUBIT_QUEUED_STOP_TRACKED", if tracked { "1" } else { "0" })
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .spawn()
            .expect("child test should start");
        let deadline = Instant::now() + WATCHDOG_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait().expect("child status should be readable") {
                if !status.success() {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    panic!("tracked={tracked} child exited with {status}; stderr: {stderr}");
                }
                break;
            }
            if Instant::now() >= deadline {
                child.kill().expect("timed-out child should be killed");
                let status = child.wait().expect("timed-out child should be reaped");
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                panic!("tracked={tracked} child timed out; killed/reaped with {status}; stderr: {stderr}");
            }
            thread::sleep(Duration::from_millis(10));
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
