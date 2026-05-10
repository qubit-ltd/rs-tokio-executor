use std::time::Duration;

use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::test]
async fn test_tokio_io_executor_service_state_tracks_abort_and_termination() {
    let service = TokioIoExecutorService::new();
    let handle = service
        .spawn(async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok::<_, &'static str>(())
        })
        .unwrap();

    let report = service.stop();
    service.await_termination().await;

    assert!(report.running >= 1);
    assert!(report.cancelled >= 1);
    assert!(service.is_not_running());
    assert!(service.is_terminated());
    assert!(handle.await.is_err());
}
