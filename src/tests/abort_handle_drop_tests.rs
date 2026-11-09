// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::Future;
use std::future::IntoFuture;
use std::pin::Pin;
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

use qubit_executor::TaskExecutionError;
use qubit_executor::task::spi::TaskEndpointPair;
use qubit_executor::task::spi::TaskSlot;

use crate::tokio_executor_service_state::TokioExecutorServiceState;
use crate::tokio_service_task_guard::TokioServiceTaskGuard;
use crate::tokio_task_registration::TokioTaskRegistration;
use crate::tokio_task_slot_cancellation::share_task_slot;

const CHILD: &str = "QUBIT_ABORT_HANDLE_DROP_CHILD";
const WATCHDOG: Duration = Duration::from_secs(5);

type ResultFuture = Pin<Box<dyn Future<Output = Result<(), TaskExecutionError<std::io::Error>>>>>;
type TestTaskSlot = TaskSlot<(), std::io::Error>;

struct ReenterAbortMap {
    state: Arc<TokioExecutorServiceState>,
    invoked: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    returned: Arc<AtomicBool>,
}

impl Wake for ReenterAbortMap {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if !self.invoked.swap(true, Ordering::SeqCst) {
            self.entered.store(true, Ordering::SeqCst);
            // This is the abort_handles lock acquisition performed by stop().
            self.state.abort_tracked_tasks();
            self.returned.store(true, Ordering::SeqCst);
        }
    }
}

#[test]
fn test_remove_abort_handle_reentrant_drop_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }

    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let abort_handle = runtime.spawn(std::future::pending::<()>()).abort_handle();
    let state = Arc::new(TokioExecutorServiceState::default());
    let marker = TokioTaskRegistration::new();
    let submission = state.lock_submission();
    assert!(state.try_accept_task(), "test task should reserve service capacity");
    drop(submission);
    let task_guard = TokioServiceTaskGuard::new(Arc::clone(&state), Arc::clone(&marker));
    let tracked = std::env::var("QUBIT_ABORT_HANDLE_DROP_TRACKED").expect("tracked case flag") == "1";
    let (result, slot): (ResultFuture, TestTaskSlot) = if tracked {
        let (handle, slot) = TaskEndpointPair::<(), std::io::Error>::new().into_tracked_parts();
        (Box::pin(handle.into_future()), slot)
    } else {
        let (handle, slot) = TaskEndpointPair::<(), std::io::Error>::new().into_parts();
        (Box::pin(handle.into_future()), slot)
    };
    slot.accept();
    let slot = share_task_slot(slot);

    let invoked = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(AtomicBool::new(false));
    let returned = Arc::new(AtomicBool::new(false));
    let waker = Waker::from(Arc::new(ReenterAbortMap {
        state: Arc::clone(&state),
        invoked: Arc::clone(&invoked),
        entered: Arc::clone(&entered),
        returned: Arc::clone(&returned),
    }));
    let mut context = Context::from_waker(&waker);
    let mut result = result;
    assert!(
        matches!(result.as_mut().poll(&mut context), Poll::Pending),
        "result Waker must be registered first"
    );

    // The registry callback owns the final SharedTaskSlot reference. Removing
    // it drops the TaskSlot, publishes Dropped, and synchronously invokes Waker.
    state.register_abort_handle(Arc::clone(&marker), abort_handle, move || {
        let _keep_slot_alive_until_registry_drop = slot;
        let _task_guard = task_guard;
        false
    });
    eprintln!("stage=remove_abort_handle");
    state.remove_abort_handle(&marker);

    assert!(invoked.load(Ordering::SeqCst), "result Waker should run");
    assert!(
        entered.load(Ordering::SeqCst),
        "Waker should enter stop-equivalent map path"
    );
    assert!(
        returned.load(Ordering::SeqCst),
        "reentrant abort map operation should return"
    );
    assert!(matches!(
        result.as_mut().poll(&mut context),
        Poll::Ready(Err(TaskExecutionError::Dropped))
    ));
    assert_eq!(state.task_count_snapshot(), (0, 0));
    runtime.shutdown_background();
}

#[test]
fn test_remove_abort_handle_drops_slot_outside_abort_map_lock() {
    for tracked in [false, true] {
        let mut child = Command::new(std::env::current_exe().expect("test executable path"))
            .args([
                "--exact",
                "tests::abort_handle_drop_tests::test_remove_abort_handle_reentrant_drop_child",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("QUBIT_ABORT_HANDLE_DROP_TRACKED", if tracked { "1" } else { "0" })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("watchdog child should start");

        let deadline = Instant::now() + WATCHDOG;
        loop {
            if let Some(status) = child.try_wait().expect("child status should be readable") {
                let output = child.wait_with_output().expect("child output should be readable");
                assert!(
                    status.success(),
                    "tracked={tracked} reentrant drop child failed: {status}; stderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                break;
            }
            if Instant::now() >= deadline {
                child.kill().expect("timed-out child should be killed");
                let output = child.wait_with_output().expect("timed-out child should be reaped");
                panic!(
                    "tracked={tracked} abort-handle removal held the map lock during result-Waker reentry; child killed/reaped; stderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
