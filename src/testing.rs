// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Internal test support exports.
//!
//! This module is hidden from public documentation and exists only so
//! integration tests can exercise small internal helpers without redirecting
//! test files to source files.

/// Shared task-slot storage used by internal Tokio executor tests.
pub use crate::tokio_task_slot_cancellation::SharedTaskSlot;
/// Cancels a queued task slot after queued service accounting completes.
///
/// The callback runs before the slot is cancelled, so a task that has already
/// started is not cancelled by a late stop request.
///
/// # Parameters
///
/// * `slot` - Shared task slot that may be cancelled.
/// * `finish_queued` - Callback that reports whether queued accounting
///   finished.
///
/// # Returns
///
/// `true` if queued accounting finished and cancellation was attempted;
/// otherwise, `false`.
pub use crate::tokio_task_slot_cancellation::cancel_unstarted_task_slot_if_queued;
/// Wraps a task slot in shared storage for competing execution paths.
///
/// # Parameters
///
/// * `slot` - Task slot to share.
///
/// # Returns
///
/// Shared storage containing the task slot.
pub use crate::tokio_task_slot_cancellation::share_task_slot;
/// Takes the task slot from shared storage if another path has not taken it.
///
/// # Parameters
///
/// * `slot` - Shared task slot to take.
///
/// # Returns
///
/// `Some` containing the slot when this call takes it, or `None` when it has
/// already been taken.
pub use crate::tokio_task_slot_cancellation::take_task_slot;
