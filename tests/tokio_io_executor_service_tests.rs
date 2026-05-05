use qubit_tokio_executor::{
    RejectedExecution,
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
        Err(RejectedExecution::Shutdown),
    ));
    service.await_termination().await;
}
