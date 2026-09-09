// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0 (the "License");
//    you may not use this file except in compliance with the License.
//    You may obtain a copy of the License at
//
//        http://www.apache.org/licenses/LICENSE-2.0
//
//    Unless required by applicable law or agreed to in writing, software
//    distributed under the License is distributed on an "AS IS" BASIS,
//    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//    See the License for the specific language governing permissions and
//    limitations under the License.
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
