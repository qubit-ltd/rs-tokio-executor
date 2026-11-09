// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use qubit_executor::service::ExecutorService;
use qubit_tokio_executor::TokioExecutorService;

const CHILD_MODE: &str = "QUBIT_TOKIO_TASK_GUARD_CHILD";
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn test_tokio_service_task_guard_notifies_termination_when_last_task_drops() {
    if std::env::var_os(CHILD_MODE).is_none() {
        run_child_with_watchdog();
        return;
    }

    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let handle = service
        .submit_tracked(|| {
            std::thread::sleep(Duration::from_millis(10));
            Ok::<_, io::Error>(())
        })
        .expect("service should accept the task");

    service.shutdown();
    handle.await.expect("task should complete successfully");
    service.wait_termination();

    assert!(
        service.is_terminated(),
        "service should terminate after its last task completes"
    );
}

fn run_child_with_watchdog() {
    let mut child = Command::new(std::env::current_exe().expect("test executable path"))
        .args([
            "--exact",
            "test_tokio_service_task_guard_notifies_termination_when_last_task_drops",
            "--nocapture",
        ])
        .env(CHILD_MODE, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("child test should start");
    let deadline = Instant::now() + WATCHDOG_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("child status should be readable") {
            assert!(status.success(), "termination child exited with {status}");
            return;
        }
        if Instant::now() >= deadline {
            child.kill().expect("timed-out child should be killed");
            let status = child.wait().expect("timed-out child should be reaped");
            panic!("last-task termination wait timed out; child was killed and reaped ({status})");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
