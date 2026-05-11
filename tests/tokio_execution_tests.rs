use std::{
    io,
    time::Duration,
};

use qubit_executor::CancelResult;
use qubit_tokio_executor::{
    Executor,
    TokioExecution,
    TokioExecutor,
};

#[tokio::test]
async fn test_tokio_execution_reports_finished_and_yields_result() {
    let executor = TokioExecutor;
    let execution = executor
        .call(|| Ok::<_, io::Error>(42))
        .expect("executor should accept callable");

    let value = execution
        .await
        .expect("execution should return callable value");

    assert_eq!(42, value);
}

#[tokio::test]
async fn test_tokio_execution_can_request_cancellation_before_await() {
    let executor = TokioExecutor;
    let execution = executor
        .call(|| Ok::<_, io::Error>(1))
        .expect("executor should accept callable");

    assert!(matches!(
        execution.cancel(),
        CancelResult::Cancelled | CancelResult::AlreadyRunning | CancelResult::AlreadyFinished
    ));
}

#[tokio::test]
async fn test_tokio_execution_wraps_join_handle_result() {
    let execution = TokioExecution::new(tokio::task::spawn_blocking(|| {
        std::thread::sleep(Duration::from_millis(10));
        Ok::<_, io::Error>(7)
    }));

    assert!(!execution.is_finished());
    assert_eq!(execution.await.expect("execution should finish"), 7);
}

#[tokio::test]
async fn test_tokio_execution_can_poll_pending() {
    let execution = TokioExecution::new(tokio::task::spawn_blocking(|| {
        std::thread::sleep(Duration::from_millis(50));
        Ok::<_, io::Error>(())
    }));

    let result = tokio::time::timeout(Duration::from_millis(1), execution).await;

    assert!(result.is_err());
}

#[test]
fn test_tokio_execution_cancel_reports_join_cancellation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should be created");

    runtime.block_on(async {
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            release_rx.recv().expect("release signal should arrive");
        });
        tokio::task::yield_now().await;

        let execution = TokioExecution::new(tokio::task::spawn_blocking(|| Ok::<_, io::Error>(1)));
        assert!(execution.cancel());
        release_tx.send(()).expect("release signal should be sent");
        blocker.await.expect("blocker should finish");

        let join = tokio::spawn(execution);
        let error = join
            .await
            .expect_err("cancelled TokioExecution should panic when awaited");
        assert!(error.is_panic());
    });
}

#[tokio::test]
async fn test_tokio_execution_resumes_join_panic() {
    let execution = TokioExecution::<(), io::Error>::new(tokio::task::spawn_blocking(|| {
        panic!("tokio execution panic")
    }));

    let error = tokio::spawn(execution)
        .await
        .expect_err("panicked TokioExecution should resume its panic");

    assert!(error.is_panic());
}
