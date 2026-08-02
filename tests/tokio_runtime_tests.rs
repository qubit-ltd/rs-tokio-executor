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

use qubit_executor::executor::Executor;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioExecutor;

#[test]
fn test_tokio_runtime_check_rejects_submission_without_runtime() {
    let result = TokioExecutor.call(|| Ok::<usize, io::Error>(42));

    assert!(matches!(
        result,
        Err(SubmissionError::WorkerSpawnFailed { .. })
    ));
}
