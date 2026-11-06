// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
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
