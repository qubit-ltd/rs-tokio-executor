use std::{
    io,
    panic::AssertUnwindSafe,
};

use qubit_tokio_executor::{
    ExecutorService,
    ExecutorServiceLifecycle,
    SubmissionError,
    TokioExecutorService,
};
use std::sync::{
    Arc,
    atomic::{
        AtomicBool,
        Ordering,
    },
};

#[tokio::test]
async fn test_tokio_executor_service_runs_blocking_tasks_and_rejects_after_shutdown() {
    let service = TokioExecutorService::new();
    let handle = service
        .submit_callable(|| Ok::<_, io::Error>("done".to_owned()))
        .expect("service should accept callable");

    assert_eq!("done", handle.await.expect("callable should finish"));

    service.shutdown();
    assert!(matches!(
        service.submit(|| Ok::<_, io::Error>(())),
        Err(SubmissionError::Shutdown),
    ));
    service.wait_termination();
}

#[tokio::test]
async fn test_tokio_executor_service_runs_tracked_callable() {
    let service = TokioExecutorService::new();
    let handle = service
        .submit_tracked_callable(|| Ok::<_, io::Error>(17))
        .expect("service should accept tracked callable");

    assert_eq!(handle.await.expect("tracked callable should finish"), 17);

    service.shutdown();
    service.wait_termination();
}

#[test]
fn test_tokio_executor_service_rejects_callable_submissions_after_shutdown() {
    let service = TokioExecutorService::new();

    service.shutdown();
    service.wait_termination();

    let callable = service.submit_callable(|| Ok::<_, io::Error>(1));
    assert!(matches!(callable, Err(SubmissionError::Shutdown)));

    let tracked = service.submit_tracked_callable(|| Ok::<_, io::Error>(2));
    assert!(matches!(tracked, Err(SubmissionError::Shutdown)));
}

#[test]
fn test_tokio_executor_service_submit_without_runtime_returns_submission_error() {
    let service = TokioExecutorService::new();

    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        service.submit_callable(|| Ok::<usize, io::Error>(42))
    }))
    .expect("tokio executor service should not panic without a runtime");

    assert!(matches!(
        result,
        Err(SubmissionError::WorkerSpawnFailed { .. })
    ));
    assert!(service.is_running());

    service.shutdown();
    service.wait_termination();
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_submit_runs_detached_task() {
    let service = TokioExecutorService::new();
    let completed = Arc::new(AtomicBool::new(false));
    let completed_for_task = Arc::clone(&completed);

    service
        .submit(move || {
            completed_for_task.store(true, Ordering::Release);
            Ok::<(), std::io::Error>(())
        })
        .expect("service should accept runnable");

    service.shutdown();
    service.wait_termination();

    assert!(completed.load(Ordering::Acquire));
    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::Terminated);
    assert!(service.is_not_running());
    assert!(service.is_terminated());

    let rejected = service.submit(|| Ok::<(), std::io::Error>(()));
    assert!(matches!(rejected, Err(SubmissionError::Shutdown)));
}
