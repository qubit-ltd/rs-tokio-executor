// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;

use qubit_executor::task::spi::TaskEndpointPair;
use qubit_tokio_executor::testing::share_task_slot;
use qubit_tokio_executor::testing::take_task_slot;

#[test]
fn test_testing_reexports_share_and_take_task_slots() {
    let (_handle, slot) = TaskEndpointPair::<usize, io::Error>::new().into_parts();
    slot.accept();
    let shared_slot = share_task_slot(slot);

    assert!(take_task_slot(&shared_slot).is_some());
    assert!(take_task_slot(&shared_slot).is_none());
}
