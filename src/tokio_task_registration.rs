// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
// qubit-style: allow type-file-name
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

/// Tracks whether a service-local task has finished registration work.
pub(crate) struct TaskRegistration {
    /// Atomic completion flag shared by task lifecycle paths.
    finished: AtomicBool,
}

impl TaskRegistration {
    /// Allocates a registration marker in its unfinished state.
    ///
    /// # Returns
    ///
    /// A reference-counted registration marker.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            finished: AtomicBool::new(false),
        })
    }

    /// Marks this registration as finished for concurrent observers.
    pub(crate) fn finish(&self) {
        self.finished.store(true, Ordering::Release);
    }

    /// Returns whether this registration has finished.
    ///
    /// # Returns
    ///
    /// `true` after [`Self::finish`] publishes completion.
    #[must_use]
    #[inline]
    pub(crate) fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

/// Returns a stable identity key for a shared registration marker.
///
/// # Parameters
///
/// * `marker` - Reference-counted marker whose allocation identity is used.
///
/// # Returns
///
/// The address-derived key used only inside this service's abort-handle map.
#[must_use]
#[inline]
pub(crate) fn key(marker: &Arc<TaskRegistration>) -> usize {
    Arc::as_ptr(marker) as usize
}
