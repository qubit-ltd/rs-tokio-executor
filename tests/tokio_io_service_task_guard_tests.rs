use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::test]
async fn test_tokio_io_service_task_guard_notifies_termination_when_future_finishes() {
    let service = TokioIoExecutorService::new();
    let handle = service
        .spawn(async { Ok::<_, &'static str>("ok") })
        .unwrap();

    service.shutdown();
    assert_eq!("ok", handle.await.unwrap());
    service.await_termination().await;
    assert!(service.is_terminated());
}
