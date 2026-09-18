// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
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
