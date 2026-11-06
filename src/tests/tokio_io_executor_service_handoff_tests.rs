// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use qubit_executor::TaskExecutionError;

use crate::TokioIoExecutorService;

struct DropProbe(Arc<AtomicUsize>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn test_stop_after_registration_keeps_capacity_until_handoff_payload_drops() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let service = TokioIoExecutorService::with_task_capacity(
        runtime.handle().clone(),
        std::num::NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let drops = Arc::new(AtomicUsize::new(0));
    let drops_in_future = Arc::clone(&drops);
    let probe = DropProbe(drops_in_future);
    let future = async move {
        let _probe = probe;
        Ok::<usize, io::Error>(42)
    };

    let pending = match service.prepare_spawn(future) {
        Ok(pending) => pending,
        Err(_) => panic!("running service should register the task"),
    };
    assert_eq!(service.stats().accepted_unfinished, 1);

    let report = service.stop();
    assert_eq!(report.cancelled, 1, "stop should find the registered task");
    assert_eq!(service.stats().accepted_unfinished, 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    let handle = pending.deliver();
    assert!(matches!(runtime.block_on(handle), Err(TaskExecutionError::Cancelled)));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(service.stats().accepted_unfinished, 0);
    runtime.block_on(service.await_termination());
}
