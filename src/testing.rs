/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
//! Internal test support exports.
//!
//! This module is hidden from public documentation and exists only so
//! integration tests can exercise small internal helpers without redirecting
//! test files to source files.

pub use crate::tokio_task_slot_cancellation::{
    SharedTaskSlot,
    cancel_unstarted_task_slot_if_queued,
    share_task_slot,
    take_task_slot,
};
