// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use crate::tokio_executor_service_state::TokioExecutorServiceState;

const CHILD_MARKER: &str = "QUBIT_TERMINATION_WAIT_REGISTRATION_CHILD";
const WATCHDOG: Duration = Duration::from_secs(10);

#[test]
fn termination_wait_registration_child() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        return;
    }

    eprintln!("stage=entering_waiter");
    let state = std::sync::Arc::new(TokioExecutorServiceState::default());
    assert!(state.try_accept_task());
    state.mark_task_started();
    state.shutdown();

    let (checked_tx, checked_rx) = mpsc::sync_channel(1);
    let waiter_state = std::sync::Arc::clone(&state);
    let waiter = thread::spawn(move || waiter_state.wait_termination_with_observer(checked_tx));
    checked_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("waiter did not check the non-terminated predicate");

    eprintln!("stage=predicate_checked_releasing_last_task");
    state.finish_task(true);
    eprintln!("stage=joining_waiter");
    waiter
        .join()
        .expect("original waiter should join after final task completion");
    assert_eq!(state.task_count_snapshot(), (0, 0));
}

#[test]
fn termination_wait_registration_has_process_watchdog() {
    let mut child = Command::new(std::env::current_exe().expect("test executable path"))
        .args([
            "--exact",
            "tests::termination_wait_registration_tests::termination_wait_registration_child",
            "--nocapture",
        ])
        .env(CHILD_MARKER, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("watchdog child should start");

    let deadline = Instant::now() + WATCHDOG;
    loop {
        if let Some(status) = child.try_wait().expect("child status should be readable") {
            let output = child
                .wait_with_output()
                .expect("child output should be readable");
            assert!(
                status.success(),
                "waiter child failed: {status}; stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            break;
        }
        if Instant::now() >= deadline {
            child.kill().expect("timed-out child should be killed");
            let output = child
                .wait_with_output()
                .expect("killed child should be reaped");
            panic!(
                "termination waiter watchdog expired; child output: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}
