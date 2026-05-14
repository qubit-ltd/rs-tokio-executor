/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use qubit_executor::service::SubmissionError;

/// Verifies that the current thread is entered into a Tokio runtime.
///
/// # Returns
///
/// `Ok(())` when Tokio task spawning can use the current runtime.
///
/// # Errors
///
/// Returns [`SubmissionError::WorkerSpawnFailed`] when no Tokio runtime is
/// entered on the current thread. Tokio's spawn APIs panic in that state, so
/// public submission APIs use this helper to reject the task explicitly.
pub(crate) fn ensure_tokio_runtime_entered() -> Result<(), SubmissionError> {
    tokio::runtime::Handle::try_current()
        .map(|_| ())
        .map_err(|error| {
            SubmissionError::worker_spawn_failed(std::io::Error::other(format!(
                "Tokio runtime is not entered: {error}",
            )))
        })
}
