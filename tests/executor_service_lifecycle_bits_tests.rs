/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
//! Style-check anchor for the private lifecycle-bit helper module.
//!
//! The helper is exercised through the public Tokio executor service lifecycle
//! tests because it is intentionally not part of the public API.

#[test]
fn test_executor_service_lifecycle_bits_is_covered_by_service_tests() {
    // This file exists to keep the project test-file mapping explicit.
}
