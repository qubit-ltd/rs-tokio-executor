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

use qubit_executor::service::ExecutorService;
use qubit_tokio_executor::TokioBlockingExecutorService;

#[tokio::test]
async fn test_tokio_blocking_task_handle_into_future_returns_result() {
    let service = TokioBlockingExecutorService::new();
    let handle = service
        .submit_tracked_callable(|| Ok::<usize, io::Error>(42))
        .expect("service should accept tracked callable");

    assert_eq!(handle.await.expect("task should finish"), 42);
    service.shutdown();
    service.wait_termination();
}
