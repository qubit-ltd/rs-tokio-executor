use std::panic::AssertUnwindSafe;

use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_tokio_executor::{
    SubmissionError,
    TokioIoExecutorService,
};

#[tokio::test]
async fn test_tokio_io_executor_service_spawns_future_and_rejects_after_shutdown() {
    let service = TokioIoExecutorService::new();
    let handle = service
        .spawn(async { Ok::<_, &'static str>(5usize) })
        .expect("io service should accept future");

    assert_eq!(5, handle.await.unwrap());

    service.shutdown();
    assert!(matches!(
        service.spawn(async { Ok::<_, &'static str>(()) }),
        Err(SubmissionError::Shutdown),
    ));
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_lifecycle_accessors() {
    let service = TokioIoExecutorService::new();

    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::Running);
    assert!(service.is_running());
    assert!(!service.is_shutting_down());
    assert!(!service.is_stopping());
    assert!(!service.is_not_running());
    assert!(!service.is_terminated());

    service.shutdown();

    assert!(service.is_not_running());
    assert!(service.is_terminated());
    assert!(!service.is_running());
}

#[test]
fn test_tokio_io_executor_service_spawn_without_runtime_returns_submission_error() {
    let service = TokioIoExecutorService::new();

    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        service.spawn(async { Ok::<usize, std::io::Error>(42) })
    }))
    .expect("tokio IO executor service should not panic without a runtime");

    assert!(matches!(
        result,
        Err(SubmissionError::WorkerSpawnFailed { .. })
    ));
    assert!(service.is_running());

    service.shutdown();
    assert!(service.is_terminated());
}
