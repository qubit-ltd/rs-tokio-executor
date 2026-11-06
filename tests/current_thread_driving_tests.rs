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
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use qubit_tokio_executor::TokioIoExecutorService;

const CHILD_MODE: &str = "QUBIT_CURRENT_THREAD_DRIVING_CHILD";
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn test_bound_current_thread_runtime_drives_submitted_future_child() {
    if std::env::var_os(CHILD_MODE).is_none() {
        return;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let service = TokioIoExecutorService::new(runtime.handle().clone());
    let polled = Arc::new(AtomicBool::new(false));
    let polled_by_future = Arc::clone(&polled);
    let handle = service
        .spawn(async move {
            polled_by_future.store(true, Ordering::SeqCst);
            Ok::<usize, io::Error>(42)
        })
        .expect("service should accept the future");

    assert!(
        !polled.load(Ordering::SeqCst),
        "spawn must not drive a current-thread runtime"
    );
    let result = runtime
        .block_on(handle)
        .expect("future should finish when runtime is driven");
    assert_eq!(result, 42);
    assert!(
        polled.load(Ordering::SeqCst),
        "future should have been polled by block_on"
    );
    service.shutdown();
    runtime.block_on(service.await_termination());
}

#[test]
fn test_current_thread_runtime_driver_watchdog() {
    let mut child = Command::new(std::env::current_exe().expect("test executable path"))
        .args([
            "--exact",
            "test_bound_current_thread_runtime_drives_submitted_future_child",
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
            assert!(status.success(), "current-thread child should finish successfully");
            return;
        }
        if Instant::now() >= deadline {
            child.kill().expect("timed-out child should be killed");
            let _ = child.wait();
            panic!("bound current-thread runtime did not make progress under block_on");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
