/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use std::{
    io,
    sync::atomic::{
        AtomicBool,
        Ordering,
    },
};

use qubit_executor::{
    TaskExecutionError,
    task::spi::{
        TaskEndpointPair,
        TaskRunner,
    },
};

use qubit_tokio_executor::testing::{
    cancel_unstarted_task_slot_if_queued,
    share_task_slot,
    take_task_slot,
};

/// Verifies queued cancellation does not steal a slot whose service accounting
/// has already moved to running.
#[test]
fn test_cancel_unstarted_task_slot_if_queued_keeps_running_slot() {
    let (handle, slot) = TaskEndpointPair::<usize, io::Error>::new().into_parts();
    slot.accept();
    let slot = share_task_slot(slot);

    let cancelled = cancel_unstarted_task_slot_if_queued(&slot, || false);

    assert!(!cancelled);
    let slot = take_task_slot(&slot).expect("running task should keep its slot");
    let ran = AtomicBool::new(false);
    assert!(
        TaskRunner::new(|| {
            ran.store(true, Ordering::Release);
            Ok::<usize, io::Error>(42)
        })
        .run(slot)
    );
    assert!(ran.load(Ordering::Acquire));
    assert_eq!(handle.get().expect("running task should finish"), 42);
}

/// Verifies queued cancellation consumes and cancels a slot only when queued
/// service accounting is still active.
#[test]
fn test_cancel_unstarted_task_slot_if_queued_cancels_queued_slot() {
    let (handle, slot) = TaskEndpointPair::<usize, io::Error>::new().into_parts();
    slot.accept();
    let slot = share_task_slot(slot);

    let cancelled = cancel_unstarted_task_slot_if_queued(&slot, || true);

    assert!(cancelled);
    assert!(take_task_slot(&slot).is_none());
    assert!(matches!(handle.get(), Err(TaskExecutionError::Cancelled)));
}

/// Verifies queued cancellation tolerates a slot that another path already
/// took from shared storage.
#[test]
fn test_cancel_unstarted_task_slot_if_queued_allows_missing_slot() {
    let (_handle, slot) = TaskEndpointPair::<usize, io::Error>::new().into_parts();
    slot.accept();
    let slot = share_task_slot(slot);
    let _taken = take_task_slot(&slot).expect("test should take the slot first");

    let cancelled = cancel_unstarted_task_slot_if_queued(&slot, || true);

    assert!(cancelled);
    assert!(take_task_slot(&slot).is_none());
}
