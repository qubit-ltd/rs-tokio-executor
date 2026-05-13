use std::{io, sync::mpsc, time::Duration};

use qubit_tokio_executor::{ExecutorService, TokioExecutorService};

#[tokio::test]
async fn test_tokio_executor_service_state_tracks_shutdown_and_active_tasks() {
    let service = TokioExecutorService::new();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let handle = service
        .submit_tracked(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok::<_, io::Error>(())
        })
        .expect("service should accept blocking task");

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    service.shutdown();
    assert!(service.is_not_running());
    assert!(!service.is_terminated());
    release_tx.send(()).unwrap();
    handle.await.unwrap();
    service.wait_termination();
    assert!(service.is_terminated());
}
