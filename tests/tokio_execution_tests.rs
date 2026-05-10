use std::io;

use qubit_tokio_executor::{Executor, TokioExecutor};

#[tokio::test]
async fn test_tokio_execution_reports_finished_and_yields_result() {
    let executor = TokioExecutor;
    let execution = executor.call(|| Ok::<_, io::Error>(42));

    let value = execution
        .await
        .expect("execution should return callable value");

    assert_eq!(42, value);
}

#[tokio::test]
async fn test_tokio_execution_can_request_cancellation_before_await() {
    let executor = TokioExecutor;
    let execution = executor.call(|| Ok::<_, io::Error>(1));

    assert!(execution.cancel());
}
