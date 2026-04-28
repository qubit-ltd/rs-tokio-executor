/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026.
 *    Haixing Hu, Qubit Co. Ltd.
 *
 *    All rights reserved.
 *
 ******************************************************************************/
//! Tests for [`TokioExecutor`](qubit_tokio_executor::TokioExecutor).

use std::{
    io,
    sync::mpsc,
    time::Duration,
};

use qubit_tokio_executor::{
    Executor,
    TokioExecutor,
};

#[tokio::test]
async fn test_tokio_executor_execute_returns_future_result() {
    let executor = TokioExecutor;

    executor
        .execute(|| Ok::<(), io::Error>(()))
        .await
        .expect("tokio executor should run runnable successfully");
}

#[tokio::test]
async fn test_tokio_executor_call_returns_future_value() {
    let executor = TokioExecutor;

    let value = executor
        .call(|| Ok::<usize, io::Error>(42))
        .await
        .expect("tokio executor should return callable value");

    assert_eq!(value, 42);
}

#[tokio::test]
async fn test_tokio_execution_is_finished_reports_completion() {
    let executor = TokioExecutor;

    let mut execution = executor.call(|| {
        std::thread::sleep(Duration::from_millis(25));
        Ok::<usize, io::Error>(42)
    });

    assert!(!execution.is_finished());
    assert_eq!(
        (&mut execution)
            .await
            .expect("tokio execution should complete"),
        42,
    );
}

#[test]
fn test_tokio_execution_cancel_queued_blocking_task_panics_when_awaited() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime should be created");

    runtime.block_on(async {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx
                .send(())
                .expect("test should receive blocking start signal");
            release_rx
                .recv()
                .expect("blocking task should receive release signal");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking slot should be occupied");

        let executor = TokioExecutor;
        let execution = executor.call(|| Ok::<(), io::Error>(()));

        assert!(execution.cancel());
        release_tx
            .send(())
            .expect("blocking task should receive release signal");
        blocker.await.expect("blocking slot task should finish");

        let waiter = tokio::spawn(execution);
        let error = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("cancelled execution should complete promptly")
            .expect_err("cancelled execution should panic when awaited");
        assert!(error.is_panic());
    });
}

#[tokio::test]
#[should_panic(expected = "tokio executor panic")]
async fn test_tokio_execution_resumes_task_panic() {
    let executor = TokioExecutor;

    executor
        .call(|| -> Result<(), io::Error> { panic!("tokio executor panic") })
        .await
        .expect("panic should be resumed before this result is observed");
}
