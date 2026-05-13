/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use std::sync::atomic::{AtomicU8, Ordering};

use qubit_executor::service::ExecutorServiceLifecycle;

/// Loads a stored executor-service lifecycle value.
pub(crate) fn load(lifecycle: &AtomicU8) -> ExecutorServiceLifecycle {
    from_u8(lifecycle.load(Ordering::Acquire))
}

/// Transitions a running service to graceful shutdown.
pub(crate) fn shutdown(lifecycle: &AtomicU8) {
    let _ = lifecycle.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        (from_u8(current) == ExecutorServiceLifecycle::Running)
            .then_some(ExecutorServiceLifecycle::ShuttingDown as u8)
    });
}

/// Transitions a service to abrupt stop.
pub(crate) fn stop(lifecycle: &AtomicU8) {
    lifecycle.store(ExecutorServiceLifecycle::Stopping as u8, Ordering::Release);
}

fn from_u8(value: u8) -> ExecutorServiceLifecycle {
    const STATES: [ExecutorServiceLifecycle; 4] = [
        ExecutorServiceLifecycle::Running,
        ExecutorServiceLifecycle::ShuttingDown,
        ExecutorServiceLifecycle::Stopping,
        ExecutorServiceLifecycle::Terminated,
    ];
    STATES[usize::from(value.min(ExecutorServiceLifecycle::Terminated as u8))]
}
