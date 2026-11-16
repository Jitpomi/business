use super::Worker;
use tokio::sync::oneshot;

#[tokio::test]
async fn shutdown_waits_for_in_flight_work_to_finish() -> anyhow::Result<()> {
    let (stop, stopped) = oneshot::channel();
    let (stopping, observed_stop) = oneshot::channel();
    let (finish, may_finish) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = stopped.await;
        let _ = stopping.send(());
        let _ = may_finish.await;
    });
    let worker = Worker {
        stop: Some(stop),
        task: Some(task),
    };
    let shutdown = tokio::spawn(worker.shutdown());
    observed_stop.await?;
    assert!(
        !shutdown.is_finished(),
        "shutdown abandoned an in-flight operation"
    );
    finish.send(()).unwrap();
    shutdown.await??;
    Ok(())
}

#[tokio::test]
async fn shutdown_reports_a_failed_poller_instead_of_claiming_quiescence() {
    let (stop, _stopped) = oneshot::channel();
    let task = tokio::spawn(async { panic!("synthetic poller failure") });
    let worker = Worker {
        stop: Some(stop),
        task: Some(task),
    };
    assert!(worker.shutdown().await.is_err());
}
