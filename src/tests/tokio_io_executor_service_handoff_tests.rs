// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use qubit_executor::TaskExecutionError;

use crate::TokioIoExecutorService;

const CHILD_MODE: &str = "QUBIT_IO_HANDOFF_WATCHDOG_CHILD";
const NORMAL_WATCHDOG: Duration = Duration::from_secs(10);
const FAILURE_WATCHDOG: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

enum ChildOutcome {
    Exited(ExitStatus),
    TimedOut(ExitStatus),
    Error(String),
}

fn reap_after_error(child: &mut Child, phase: &str) -> String {
    let kill = child.kill();
    let wait = child.wait();
    format!("{phase}; cleanup kill={kill:?}, wait={wait:?}")
}

fn run_child(mode: &str, timeout: Duration) -> ChildOutcome {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => return ChildOutcome::Error(format!("current_exe failed: {error}")),
    };
    let mut child = match Command::new(executable)
        .args([
            "--exact",
            "tests::tokio_io_executor_service_handoff_tests::test_io_handoff_watchdog_child",
            "--nocapture",
        ])
        .env(CHILD_MODE, mode)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return ChildOutcome::Error(format!("spawn failed: {error}")),
    };

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return ChildOutcome::Exited(status),
            Ok(None) => {}
            Err(error) => {
                return ChildOutcome::Error(reap_after_error(&mut child, &format!("try_wait failed: {error}")));
            }
        }
        if Instant::now() >= deadline {
            let kill = child.kill();
            return match child.wait() {
                Ok(status) => ChildOutcome::TimedOut(status),
                Err(error) => ChildOutcome::Error(format!("watchdog timeout; kill={kill:?}, wait failed: {error}")),
            };
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn assert_child_exited(outcome: ChildOutcome, mode: &str) -> ExitStatus {
    match outcome {
        ChildOutcome::Exited(status) => status,
        ChildOutcome::TimedOut(status) => panic!(
            "child mode={mode} exceeded its watchdog and was killed/reaped with status {status}; rerun with --nocapture for child stage logs"
        ),
        ChildOutcome::Error(message) => panic!("child mode={mode}: {message}"),
    }
}

struct DropProbe(Arc<AtomicUsize>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn test_stop_after_registration_keeps_capacity_until_handoff_payload_drops() {
    let status = assert_child_exited(run_child("normal", NORMAL_WATCHDOG), "normal");
    assert!(
        status.success(),
        "normal handoff child failed with {status}; rerun with --nocapture for child stage logs"
    );
}

#[test]
fn test_io_handoff_watchdog_detects_nonzero_exit() {
    let status = assert_child_exited(run_child("fail", FAILURE_WATCHDOG), "fail");
    assert!(!status.success(), "fail-mode child unexpectedly exited successfully");
}

#[test]
fn test_io_handoff_watchdog_kills_and_reaps_hung_child() {
    match run_child("hang", FAILURE_WATCHDOG) {
        ChildOutcome::TimedOut(status) => assert!(
            !status.success(),
            "killed hang-mode child unexpectedly exited successfully"
        ),
        ChildOutcome::Exited(status) => {
            panic!("hang-mode child exited before watchdog: {status}")
        }
        ChildOutcome::Error(message) => panic!("hang-mode child: {message}"),
    }
}

#[test]
fn test_io_handoff_watchdog_child() {
    let Some(mode) = std::env::var_os(CHILD_MODE) else {
        return;
    };
    let mode = mode.to_string_lossy();
    match mode.as_ref() {
        "normal" => run_handoff_scenario(),
        "fail" => panic!("intentional handoff watchdog failure mode"),
        "hang" => loop {
            thread::park();
        },
        other => panic!("unknown handoff watchdog child mode: {other}"),
    }
}

fn run_handoff_scenario() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let service = TokioIoExecutorService::with_task_capacity(
        runtime.handle().clone(),
        std::num::NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let drops = Arc::new(AtomicUsize::new(0));
    let drops_in_future = Arc::clone(&drops);
    let probe = DropProbe(drops_in_future);
    let future = async move {
        let _probe = probe;
        Ok::<usize, io::Error>(42)
    };

    let pending = match service.prepare_spawn(future) {
        Ok(pending) => pending,
        Err(_) => panic!("running service should register the task"),
    };
    eprintln!("stage=registered");
    assert_eq!(service.stats().accepted_unfinished, 1);

    let report = service.stop();
    eprintln!("stage=stopped");
    assert_eq!(report.cancelled, 1, "stop should find the registered task");
    assert_eq!(service.stats().accepted_unfinished, 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    let handle = pending.deliver();
    eprintln!("stage=delivered");
    eprintln!("stage=result_wait_before");
    let result = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(2), handle).await });
    eprintln!("stage=result_waited");
    assert!(
        matches!(result, Ok(Err(TaskExecutionError::Cancelled))),
        "handoff result should become Cancelled within 2 seconds"
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(service.stats().accepted_unfinished, 0);
    eprintln!("stage=termination_wait_before");
    let termination =
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(2), service.await_termination()).await });
    eprintln!("stage=termination_waited");
    assert!(termination.is_ok(), "service should terminate within 2 seconds");
    eprintln!("stage=runtime_drop_before");
    drop(runtime);
    eprintln!("stage=runtime_drop_after");
}
