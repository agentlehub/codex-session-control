use super::*;

#[tokio::test(start_paused = true)]
async fn sequential_native_reads_receive_fresh_ten_second_deadlines() {
    let harness = FakeAppServer::start(vec![
        FakeStep::result(
            "thread/read",
            json!({"threadId": "target", "includeTurns": false}),
            json!({
                "thread": native_thread("target", json!({"type": "idle"}), 10)
            }),
        )
        .delayed(Duration::from_secs(9)),
        FakeStep::result(
            "thread/turns/list",
            json!({"threadId": "target"}),
            json!({"data": [], "nextCursor": null}),
        )
        .delayed(Duration::from_secs(9)),
    ])
    .await;
    let client = harness.client();
    let mut connection = client.connect_initialized().await.unwrap();
    let task =
        tokio::spawn(async move { connection.thread_read("target", None, None, None).await });
    while harness.log().is_empty() {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(9)).await;
    while harness.log().len() < 2 {
        tokio::task::yield_now().await;
    }
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_secs(9)).await;
    let (thread, turns, next_cursor) = task.await.unwrap().unwrap();
    assert_eq!(thread.id, "target");
    assert!(turns.is_empty());
    assert_eq!(next_cursor, None);
}
