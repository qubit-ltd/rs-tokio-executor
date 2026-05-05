use std::io;

use qubit_tokio_executor::{
    ExecutorService,
    RejectedExecution,
    TokioExecutorService,
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
        Err(RejectedExecution::Shutdown),
    ));
    service.await_termination().await;
}
