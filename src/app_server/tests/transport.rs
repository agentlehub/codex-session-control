use super::*;
use crate::model::ThreadStatus;

#[tokio::test]
async fn websocket_upgrade_uses_exact_rpc_path() {
    let harness = FakeAppServer::start(FakeScript::happy()).await;

    for path in ["/", "/rpc?x=1", "/other"] {
        let stream = UnixStream::connect(&harness.socket_path).await.unwrap();
        let target = format!("ws://localhost{path}");
        assert!(client_async(target, stream).await.is_err(), "{path}");
    }

    harness
        .client(TESTED_CODEX_VERSION)
        .connect_initialized()
        .await
        .unwrap();

    assert_eq!(
        harness.recorded_upgrade_targets(),
        ["/", "/rpc?x=1", "/other", "/rpc"]
    );
}

#[tokio::test]
async fn future_operation_re_resolves_replaced_socket() {
    let mut harness = FakeAppServer::start(FakeScript::happy()).await;
    let client = harness.client(TESTED_CODEX_VERSION);

    let mut first_connection = client.connect_initialized().await.unwrap();
    let _: serde_json::Value = first_connection
        .request("thread/read", json!({"threadId": "thread-1"}))
        .await
        .unwrap();
    drop(first_connection);
    harness.wait_for_close().await;

    harness.replace_socket(FakeScript::happy()).await;

    let mut second_connection = client.connect_initialized().await.unwrap();
    let response: serde_json::Value = second_connection
        .request("thread/read", json!({"threadId": "thread-1"}))
        .await
        .unwrap();

    assert_eq!(response["thread"]["id"], "thread-1");
    assert_eq!(harness.connection_count(), 2);
}

#[tokio::test]
async fn initialize_precedes_initialized_and_accepts_any_absolute_codex_home() {
    let harness = FakeAppServer::start(FakeScript::happy()).await;
    let client = harness.client(TESTED_CODEX_VERSION);
    let connection = client.connect_initialized().await.unwrap();
    harness.wait_for_frames(2).await;
    assert_eq!(
        connection.initialize_result,
        Some(json!({
            "codexHome": harness.codex_home(),
            "platformFamily": "unix",
            "platformOs": "linux",
            "userAgent": TESTED_CODEX_CLI_VERSION
        }))
    );

    assert_eq!(
        harness.recorded_frames().await,
        vec![
            initialize_frame(1, harness.codex_home()),
            json!({"method": "initialized"})
        ]
    );
    assert_eq!(harness.connection_count(), 1);
    drop(connection);

    let different_absolute_home =
        FakeAppServer::start(FakeScript::happy().with_initialize_codex_home(Some("/other/home")))
            .await;
    different_absolute_home
        .client(TESTED_CODEX_VERSION)
        .connect_initialized()
        .await
        .unwrap();

    for codex_home in [None, Some("relative/home")] {
        let invalid =
            FakeAppServer::start(FakeScript::happy().with_initialize_codex_home(codex_home)).await;
        let error = invalid
            .client(TESTED_CODEX_VERSION)
            .connect_initialized()
            .await
            .unwrap_err();
        assert_eq!(error.category, ToolErrorCategory::TargetUnavailable);
        assert_eq!(error.stage, "initialize");
    }
}

#[tokio::test]
async fn request_ids_correlate_across_notifications() {
    let harness = FakeAppServer::start(FakeScript::happy().with_native_frames(vec![
        ServerFrame::Notification(json!({
            "method": "thread/status/changed",
            "params": {"threadId": "thread-1"}
        })),
        ServerFrame::Response(json!({
            "id": 2,
            "result": {"thread": {"id": "thread-1"}}
        })),
    ]))
    .await;
    let mut connection = harness
        .client(TESTED_CODEX_VERSION)
        .connect_initialized()
        .await
        .unwrap();
    let result: serde_json::Value = connection
        .request("thread/read", json!({"threadId": "thread-1"}))
        .await
        .unwrap();

    assert_eq!(result, json!({"thread": {"id": "thread-1"}}));
    assert_eq!(
        harness.recorded_frames().await,
        vec![
            initialize_frame(1, harness.codex_home()),
            json!({"method": "initialized"}),
            json!({
                "id": 2,
                "method": "thread/read",
                "params": {"threadId": "thread-1"}
            })
        ]
    );
    assert_eq!(harness.connection_count(), 1);
}

#[tokio::test]
async fn server_requests_receive_no_client_response() {
    let harness = FakeAppServer::start(FakeScript::happy().with_native_frames(vec![
        ServerFrame::Request(json!({
            "id": 2,
            "method": "item/tool/requestUserInput",
            "params": {}
        })),
        ServerFrame::Response(json!({"id": 2, "result": {"data": []}})),
    ]))
    .await;
    let mut connection = harness
        .client(TESTED_CODEX_VERSION)
        .connect_initialized()
        .await
        .unwrap();
    let _: serde_json::Value = connection.request("thread/list", json!({})).await.unwrap();
    drop(connection);
    harness.wait_for_close().await;

    assert_eq!(
        harness.recorded_frames().await,
        vec![
            initialize_frame(1, harness.codex_home()),
            json!({"method": "initialized"}),
            json!({"id": 2, "method": "thread/list", "params": {}})
        ]
    );
    assert_eq!(harness.connection_count(), 1);
}

#[tokio::test]
async fn socket_parent_owner_mode_and_type_are_required() {
    for violation in [
        SocketViolation::ParentMode,
        SocketViolation::SocketMode,
        SocketViolation::SocketSymlink,
        SocketViolation::NotSocket,
    ] {
        let harness = FakeAppServer::with_socket_violation(violation).await;
        let error = harness
            .client(TESTED_CODEX_VERSION)
            .connect_initialized()
            .await
            .unwrap_err();
        assert_eq!(
            error.category,
            ToolErrorCategory::AuthorityTransportFailure,
            "{violation:?}"
        );
        assert_eq!(error.stage, "socket_validation");
        assert_eq!(harness.connection_count(), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn each_non_wait_stage_has_an_independent_ten_second_deadline() {
    assert_eq!(NATIVE_STAGE_TIMEOUT, Duration::from_secs(10));

    let timer = tokio::spawn(with_native_stage_timeout::<()>("connect", pending()));
    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(!timer.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    let error = timer.await.unwrap().unwrap_err();
    assert_eq!(error.category, ToolErrorCategory::StageTimeout);
    assert_eq!(error.stage, "connect");

    for barrier in [
        TransportBarrier::BeforeInitializeResponse,
        TransportBarrier::BeforeNativeResponse,
    ] {
        let harness = FakeAppServer::start(FakeScript::blocked_at(barrier)).await;
        let operation = tokio::spawn(run_to_barrier(
            harness.client(TESTED_CODEX_VERSION),
            barrier,
        ));
        harness.wait_until_barrier().await;
        tokio::time::advance(Duration::from_secs(9)).await;
        assert!(!operation.is_finished());
        tokio::time::advance(Duration::from_secs(1)).await;
        let error = operation.await.unwrap().unwrap_err();
        assert_eq!(error.category, ToolErrorCategory::StageTimeout);
        assert_eq!(
            error.stage,
            match barrier {
                TransportBarrier::BeforeInitializeResponse => "initialize",
                TransportBarrier::BeforeNativeResponse => "thread/read",
            }
        );
    }
}

#[tokio::test]
async fn one_primary_connection_is_closed_after_result() {
    let harness = FakeAppServer::start(FakeScript::happy()).await;
    {
        let mut connection = harness
            .client(TESTED_CODEX_VERSION)
            .connect_initialized()
            .await
            .unwrap();
        let _: serde_json::Value = connection.request("thread/list", json!({})).await.unwrap();
        let snapshot = connection.compact_snapshot("thread-1").await.unwrap();
        assert_eq!(snapshot.thread_id, "thread-1");
        assert_eq!(snapshot.status, ThreadStatus::NotLoaded);
    }
    harness.wait_for_close().await;

    assert_eq!(harness.connection_count(), 1);
    assert_eq!(
        harness.recorded_frames().await,
        vec![
            initialize_frame(1, harness.codex_home()),
            json!({"method": "initialized"}),
            json!({"id": 2, "method": "thread/list", "params": {}}),
            json!({
                "id": 3,
                "method": "thread/read",
                "params": {"threadId": "thread-1", "includeTurns": false}
            }),
            json!({
                "id": 4,
                "method": "thread/turns/list",
                "params": {
                    "threadId": "thread-1",
                    "limit": 1,
                    "itemsView": "notLoaded"
                }
            })
        ]
    );
}

#[tokio::test]
async fn mutation_failure_points_preserve_dispatch_truth() {
    for failure_point in [
        FailurePoint::BeforeWrite,
        FailurePoint::AfterPartialWrite,
        FailurePoint::AfterFullWrite,
        FailurePoint::BeforeResponse,
        FailurePoint::AfterNativeStateChange,
    ] {
        let harness = FakeAppServer::start(FakeScript::at_failure(failure_point)).await;
        let mut connection = harness
            .client(TESTED_CODEX_VERSION)
            .connect_initialized()
            .await
            .unwrap();
        harness.wait_for_frames(2).await;
        let error = connection
            .mutate::<Value>(
                "thread_set_name",
                "thread/name/set",
                json!({"threadId": "thread-1", "name": "name"}),
                Some("thread-1"),
                None,
            )
            .await
            .unwrap_err();

        let expected_dispatch = if matches!(failure_point, FailurePoint::BeforeWrite) {
            DispatchState::NotDispatched
        } else {
            DispatchState::MayHaveBeenDispatched
        };
        let expected_category = if matches!(failure_point, FailurePoint::BeforeWrite) {
            ToolErrorCategory::AuthorityTransportFailure
        } else {
            ToolErrorCategory::OutcomeUnknown
        };
        assert_eq!(error.category, expected_category, "{failure_point:?}");
        assert_eq!(error.dispatch, Some(expected_dispatch), "{failure_point:?}");
        assert_eq!(harness.connection_count(), 1);

        if matches!(
            failure_point,
            FailurePoint::AfterFullWrite
                | FailurePoint::BeforeResponse
                | FailurePoint::AfterNativeStateChange
        ) {
            harness.wait_for_frames(3).await;
        }
        assert_eq!(
            harness.recorded_frames().await.len(),
            if matches!(
                failure_point,
                FailurePoint::AfterFullWrite
                    | FailurePoint::BeforeResponse
                    | FailurePoint::AfterNativeStateChange
            ) {
                3
            } else {
                2
            },
            "{failure_point:?}"
        );
        assert_eq!(
            harness.native_state_changed(),
            matches!(failure_point, FailurePoint::AfterNativeStateChange),
            "{failure_point:?}"
        );
    }
}

#[tokio::test]
async fn correlated_native_error_marks_the_mutation_result_received() {
    let conflict = protocol_fixture().error_exemplars["activeTurnMismatch"].clone();
    let harness =
        FakeAppServer::start(
            FakeScript::happy().with_native_frames(vec![ServerFrame::Response(
                json!({"id": 2, "error": conflict}),
            )]),
        )
        .await;
    let mut connection = harness
        .client(TESTED_CODEX_VERSION)
        .connect_initialized()
        .await
        .unwrap();
    let error = connection
        .mutate::<Value>(
            "message_send",
            "turn/steer",
            json!({
                "threadId": "thread-1",
                "expectedTurnId": "turn-expected",
                "input": []
            }),
            Some("thread-1"),
            Some("turn-expected"),
        )
        .await
        .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::NativeConflict);
    assert_eq!(error.dispatch, Some(DispatchState::MayHaveBeenDispatched));
    assert_eq!(
        connection.dispatch,
        MutationDispatch::CorrelatedResultReceived
    );
    assert_eq!(harness.connection_count(), 1);
}

#[tokio::test]
async fn tested_version_has_no_warning() {
    assert_eq!(
        extract_codex_version(&format!(
            "Codex Desktop/{TESTED_CODEX_VERSION} (Arch Linux Unknown; x86_64) dumb \
             (codex_session_control; 0.1.0)"
        )),
        Some(TESTED_CODEX_VERSION.to_owned())
    );
    let harness =
        FakeAppServer::start(FakeScript::happy().with_codex_version(TESTED_CODEX_VERSION)).await;
    let connection = harness
        .client(TESTED_CODEX_VERSION)
        .connect_initialized()
        .await
        .unwrap();

    assert_eq!(connection.compatibility_warning(), None);
    assert_eq!(harness.connection_count(), 1);
}

#[tokio::test]
async fn untested_version_exposes_connection_warning() {
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    let harness =
        FakeAppServer::start(FakeScript::happy().with_codex_version(&untested_version)).await;
    let connection = harness
        .client(TESTED_CODEX_VERSION)
        .connect_initialized()
        .await
        .unwrap();
    let warning = connection
        .compatibility_warning()
        .expect("untested Codex version must produce a compatibility warning");
    assert!(warning.contains(&untested_version));
    assert!(warning.contains(TESTED_CODEX_VERSION));
    assert_eq!(harness.connection_count(), 1);
}
