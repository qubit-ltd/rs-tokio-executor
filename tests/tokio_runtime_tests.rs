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
use qubit_tokio_executor::TokioExecutor;

#[test]
fn test_tokio_executor_uses_explicit_runtime_handle() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let executor = TokioExecutor::new(runtime.handle().clone());
    let result = executor
        .call(|| Ok::<usize, io::Error>(42))
        .expect("submission should succeed");
    assert_eq!(result.get().expect("task should succeed"), 42);
}
