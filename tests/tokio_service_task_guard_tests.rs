use std::{io, time::Duration};

use qubit_tokio_executor::{ExecutorService, TokioExecutorService};

#[tokio::test]
async fn test_tokio_service_task_guard_notifies_termination_when_last_task_drops() {
    let service = TokioExecutorService::new();
    let handle = service
        .submit_tracked(|| {
            std::thread::sleep(Duration::from_millis(10));
            Ok::<_, io::Error>(())
        })
        .unwrap();

    service.shutdown();
    handle.await.unwrap();
    service.wait_termination();

    assert!(service.is_terminated());
}
