use super::*;

const WAIT_CASES: [&str; 9] = [
    "initial_idle_is_ready",
    "initial_not_loaded_is_ready",
    "initial_system_error_is_ready",
    "initial_interactive_flag_is_ready",
    "active_to_idle_is_ready",
    "new_terminal_latest_turn_is_ready",
    "requested_zero_timeout_is_successful_timeout",
    "target_error_returns_complete_decisive_pass",
    "shared_transport_failure_is_request_wide_error",
];

async fn run_wait(
    steps: Vec<FakeStep>,
    thread_ids: &[&str],
    timeout: Duration,
) -> (Result<ThreadsWaitResult, ToolErrorData>, FakeAppServer) {
    let harness = FakeAppServer::start(steps).await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();
    let ids = thread_ids
        .iter()
        .map(|thread_id| (*thread_id).to_owned())
        .collect::<Vec<_>>();
    let result = threads_wait(&mut connection, &ids, timeout).await;
    drop(connection);
    (result, harness)
}

#[tokio::test]
async fn initial_ready_states_are_decisive() {
    assert_eq!(WAIT_CASES.len(), 9);
    for status in [
        json!({"type": "idle"}),
        json!({"type": "notLoaded"}),
        json!({"type": "systemError"}),
        json!({
            "type": "active",
            "activeFlags": ["waitingOnApproval"],
        }),
    ] {
        let (result, harness) = run_wait(
            snapshot_steps("target", status, 10, None, false),
            &["target"],
            Duration::from_secs(30),
        )
        .await;
        let result = result.unwrap();
        assert!(matches!(result.reason, ThreadsWaitReason::Ready));
        assert_eq!(result.trigger_thread_ids, ["target"]);
        assert!(result.errors.is_empty());
        assert_eq!(harness.log().len(), 2);
        assert_eq!(harness.connection_count(), 1);
    }
}

#[tokio::test]
async fn active_transitions_preserve_input_order_and_one_connection() {
    let active = json!({"type": "active", "activeFlags": []});
    let mut steps = Vec::new();
    steps.extend(snapshot_steps(
        "first",
        active.clone(),
        10,
        Some(native_turn("turn-first", "inProgress")),
        false,
    ));
    steps.extend(snapshot_steps(
        "second",
        active,
        10,
        Some(native_turn("turn-second", "inProgress")),
        true,
    ));
    steps.extend(snapshot_steps(
        "first",
        json!({"type": "idle"}),
        20,
        Some(native_turn("turn-first", "completed")),
        false,
    ));
    steps.extend(snapshot_steps(
        "second",
        json!({"type": "idle"}),
        20,
        Some(native_turn("turn-second", "failed")),
        false,
    ));
    let (result, harness) = run_wait(steps, &["first", "second"], Duration::from_secs(30)).await;
    let result = result.unwrap();
    assert!(matches!(result.reason, ThreadsWaitReason::Ready));
    assert_eq!(result.trigger_thread_ids, ["first", "second"]);
    assert_eq!(result.threads.len(), 2);
    assert_eq!(harness.log().len(), 8);
    assert_eq!(harness.connection_count(), 1);
}

#[tokio::test]
async fn terminal_latest_turn_wakes_even_if_thread_status_remains_active() {
    let active = json!({"type": "active", "activeFlags": []});
    let mut steps = snapshot_steps(
        "target",
        active.clone(),
        10,
        Some(native_turn("turn-1", "inProgress")),
        true,
    );
    steps.extend(snapshot_steps(
        "target",
        active,
        20,
        Some(native_turn("turn-1", "completed")),
        false,
    ));
    let (result, _) = run_wait(steps, &["target"], Duration::from_secs(30)).await;
    let result = result.unwrap();
    assert!(matches!(result.reason, ThreadsWaitReason::Ready));
    assert_eq!(result.trigger_thread_ids, ["target"]);
}

#[tokio::test]
async fn requested_zero_timeout_is_successful_timeout() {
    let (result, harness) = run_wait(
        snapshot_steps(
            "target",
            json!({"type": "active", "activeFlags": []}),
            10,
            Some(native_turn("turn-1", "inProgress")),
            false,
        ),
        &["target"],
        Duration::ZERO,
    )
    .await;
    let result = result.unwrap();
    assert!(matches!(result.reason, ThreadsWaitReason::Timeout));
    assert!(result.trigger_thread_ids.is_empty());
    assert!(result.errors.is_empty());
    assert_eq!(harness.log().len(), 2);
}

#[tokio::test]
async fn target_error_returns_complete_decisive_pass() {
    let mut steps = vec![FakeStep::error(
        "thread/read",
        json!({"threadId": "missing", "includeTurns": false}),
        json!({
            "code": -32600,
            "message": "thread not loaded: missing",
        }),
    )];
    steps.extend(snapshot_steps(
        "readable",
        json!({"type": "idle"}),
        10,
        None,
        false,
    ));
    let (result, harness) =
        run_wait(steps, &["missing", "readable"], Duration::from_secs(30)).await;
    let result = result.unwrap();
    assert!(matches!(result.reason, ThreadsWaitReason::Error));
    assert!(result.trigger_thread_ids.is_empty());
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].thread_id.as_deref(), Some("missing"));
    assert_eq!(result.threads[0].thread_id, "readable");
    assert_eq!(harness.log().len(), 3);
}

#[tokio::test]
async fn shared_transport_failure_is_request_wide_error() {
    let harness = FakeAppServer::start(vec![FakeStep {
        method: "thread/read",
        params: json!({"threadId": "target", "includeTurns": false}),
        response: FakeResponse::Disconnect,
        notify_after: false,
        delay: Duration::ZERO,
    }])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();
    let error = threads_wait(
        &mut connection,
        &["target".to_owned()],
        Duration::from_secs(30),
    )
    .await
    .unwrap_err();
    assert_eq!(error.category, ToolErrorCategory::AuthorityTransportFailure);
}

#[tokio::test(start_paused = true)]
async fn quiet_poll_occurs_at_exactly_one_second() {
    let mut steps = snapshot_steps(
        "target",
        json!({"type": "active", "activeFlags": []}),
        10,
        Some(native_turn("turn-1", "inProgress")),
        false,
    );
    steps.extend(snapshot_steps(
        "target",
        json!({"type": "idle"}),
        20,
        Some(native_turn("turn-1", "completed")),
        false,
    ));
    let harness = FakeAppServer::start(steps).await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();
    let task = tokio::spawn(async move {
        threads_wait(
            &mut connection,
            &["target".to_owned()],
            Duration::from_secs(30),
        )
        .await
    });
    while harness.log().len() < 2 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_millis(999)).await;
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    assert_eq!(harness.log().len(), 2);
    tokio::time::advance(Duration::from_millis(1)).await;
    let result = task.await.unwrap().unwrap();
    assert!(matches!(result.reason, ThreadsWaitReason::Ready));
    assert_eq!(harness.log().len(), 4);
}

#[tokio::test(start_paused = true)]
async fn per_target_stage_timeout_keeps_complete_pass_attribution() {
    let mut steps = vec![FakeStep {
        method: "thread/read",
        params: json!({"threadId": "slow", "includeTurns": false}),
        response: FakeResponse::Pending,
        notify_after: false,
        delay: Duration::ZERO,
    }];
    steps.extend(snapshot_steps(
        "readable",
        json!({"type": "idle"}),
        10,
        None,
        false,
    ));
    let harness = FakeAppServer::start(steps).await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();
    let task = tokio::spawn(async move {
        threads_wait(
            &mut connection,
            &["slow".to_owned(), "readable".to_owned()],
            Duration::from_secs(30),
        )
        .await
    });
    while harness.log().is_empty() {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(crate::app_server::NATIVE_STAGE_TIMEOUT).await;
    let result = task.await.unwrap().unwrap();
    assert!(matches!(result.reason, ThreadsWaitReason::Error));
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].category, ToolErrorCategory::StageTimeout);
    assert_eq!(result.errors[0].thread_id.as_deref(), Some("slow"));
    assert_eq!(result.threads[0].thread_id, "readable");
}

#[tokio::test(start_paused = true)]
async fn requested_expiry_during_poll_is_successful_timeout() {
    let mut steps = snapshot_steps(
        "target",
        json!({"type": "active", "activeFlags": []}),
        10,
        Some(native_turn("turn-1", "inProgress")),
        true,
    );
    steps.push(FakeStep {
        method: "thread/read",
        params: json!({"threadId": "target", "includeTurns": false}),
        response: FakeResponse::Pending,
        notify_after: false,
        delay: Duration::ZERO,
    });
    let harness = FakeAppServer::start(steps).await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();
    let task = tokio::spawn(async move {
        threads_wait(
            &mut connection,
            &["target".to_owned()],
            Duration::from_secs(5),
        )
        .await
    });
    while harness.log().len() < 3 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(5)).await;
    let result = task.await.unwrap().unwrap();
    assert!(matches!(result.reason, ThreadsWaitReason::Timeout));
    assert!(result.errors.is_empty());
    assert!(result.trigger_thread_ids.is_empty());
}
