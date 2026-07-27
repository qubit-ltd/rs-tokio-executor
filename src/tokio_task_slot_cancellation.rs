// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::sync::Arc;

use qubit_executor::task::spi::{TaskSlot, TaskSlotCell};

/// Shared runner-side task slot used by service stop and task execution paths.
pub type SharedTaskSlot<R, E> = Arc<TaskSlotCell<R, E>>;

/// Wraps a task slot in shared optional storage.
///
/// # Parameters
///
/// * `slot` - Runner-side completion endpoint for an accepted task.
///
/// # Returns
///
/// A shared optional slot that can be taken by either the runner path or the
/// queued-cancellation path.
#[inline]
pub fn share_task_slot<R, E>(slot: TaskSlot<R, E>) -> SharedTaskSlot<R, E> {
    Arc::new(TaskSlotCell::new(slot))
}

/// Takes the task slot from shared storage while tolerating poisoned locks.
///
/// # Parameters
///
/// * `slot` - Shared optional task slot.
///
/// # Returns
///
/// `Some(TaskSlot)` if this call won the slot ownership race, otherwise `None`.
pub fn take_task_slot<R, E>(slot: &SharedTaskSlot<R, E>) -> Option<TaskSlot<R, E>> {
    slot.take()
}

/// Cancels an unstarted task slot if queued service accounting is still active.
///
/// The queued-accounting callback is consulted before the slot is taken. This
/// prevents a late stop request from cancelling the result slot after service
/// accounting has already moved the task to running.
///
/// # Parameters
///
/// * `slot` - Shared optional task slot to cancel.
/// * `finish_queued` - Callback that finishes queued service accounting.
///
/// # Returns
///
/// `true` if queued service accounting was completed by this call.
pub fn cancel_unstarted_task_slot_if_queued<R, E, F>(
    slot: &SharedTaskSlot<R, E>,
    finish_queued: F,
) -> bool
where
    F: FnOnce() -> bool,
{
    if finish_queued() {
        let _ = slot.cancel_unstarted();
        true
    } else {
        false
    }
}
