// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
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
