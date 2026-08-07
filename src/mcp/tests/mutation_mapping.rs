use super::*;

fn native_error() -> Value {
    json!({"code": -32090, "message": "native mutation rejected"})
}

const EXPECTED_PINNED_SECTION_ID: &str = "01984de2-8f74-7c91-a3b2-5c5e937cf318";

fn pin_read_step(section: Value) -> FakeStep {
    FakeStep::result(
        "thread/read",
        json!({"threadId": "target", "includeTurns": false}),
        json!({"thread": {"id": "target", "section": section}}),
    )
}

#[tokio::test]
async fn create_uses_exact_sequence_and_effective_native_values() {
    let prompt = "CREATE_PROMPT_SENTINEL";
    let harness = FakeAppServer::start(vec![
        FakeStep::result(
            "thread/start",
            json!({
                "cwd": "/requested",
                "model": "requested-model",
                "config": {"model_reasoning_effort": "high"},
            }),
            json!({
                "thread": native_thread(
                    "created",
                    json!({"type": "idle"}),
                    11
                ),
                "cwd": "/effective",
                "model": "effective-model",
                "reasoningEffort": "medium",
            }),
        ),
        FakeStep::result(
            "turn/start",
            json!({
                "threadId": "created",
                "input": [{"type": "text", "text": prompt}],
            }),
            json!({"turn": native_turn("initial-turn", "inProgress")}),
        ),
    ])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let result = create_thread(
        &client,
        &mut connection,
        ThreadCreateInput {
            prompt: prompt.to_owned(),
            cwd: "/requested".to_owned(),
            model: Some("requested-model".to_owned()),
            reasoning_effort: Some("high".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(result.thread_id, "created");
    assert_eq!(result.turn_id, "initial-turn");
    assert_eq!(result.cwd, "/effective");
    assert_eq!(result.model.as_deref(), Some("effective-model"));
    assert_eq!(result.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(harness.connection_count(), 1);
    assert_eq!(harness.log().len(), 2);
}

#[tokio::test]
async fn create_initial_turn_failure_preserves_known_thread_without_cleanup() {
    let harness = FakeAppServer::start(vec![
        FakeStep::result(
            "thread/start",
            json!({"cwd": "/workspace"}),
            json!({
                "thread": native_thread(
                    "created",
                    json!({"type": "idle"}),
                    11
                ),
                "cwd": "/workspace",
                "model": null,
                "reasoningEffort": null,
            }),
        ),
        FakeStep::error(
            "turn/start",
            json!({
                "threadId": "created",
                "input": [{"type": "text", "text": "prompt"}],
            }),
            native_error(),
        ),
    ])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let error = create_thread(
        &client,
        &mut connection,
        ThreadCreateInput {
            prompt: "prompt".to_owned(),
            cwd: "/workspace".to_owned(),
            model: None,
            reasoning_effort: None,
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::NativeError);
    assert_eq!(error.thread_id.as_deref(), Some("created"));
    assert_eq!(
        harness
            .log()
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["thread/start", "turn/start"]
    );
}

#[tokio::test]
async fn fork_defaults_and_explicit_values_map_without_extra_reads() {
    for (public_input, expected) in [
        (json!({"threadId": "source"}), true),
        (
            json!({"threadId": "source", "deferGoalContinuation": false}),
            false,
        ),
        (
            json!({"threadId": "source", "deferGoalContinuation": true}),
            true,
        ),
    ] {
        let harness = FakeAppServer::start(vec![FakeStep::result(
            "thread/fork",
            json!({
                "threadId": "source",
                "deferGoalContinuation": expected,
            }),
            json!({
                "thread": native_thread(
                    "forked",
                    json!({"type": "idle"}),
                    12
                ),
                "model": "effective-model",
                "reasoningEffort": "high",
            }),
        )])
        .await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();
        let ValidatedInput::ThreadFork(input) =
            validate_input("thread_fork", arguments(public_input), &meta("caller")).unwrap()
        else {
            panic!("wrong validated input")
        };

        let result = fork_thread(&client, &mut connection, input).await.unwrap();

        assert_eq!(result.thread.id, "forked");
        assert_eq!(result.model.as_deref(), Some("effective-model"));
        assert_eq!(result.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(harness.connection_count(), 1);
        assert_eq!(harness.log().len(), 1);
    }
}

#[tokio::test]
async fn message_send_reports_empty_rollout_as_safe_to_retry_without_dispatching() {
    let native_message = "failed to read thread: thread-store internal error: failed to read session metadata /home/operator/.codex/sessions/rollout.jsonl: rollout at /home/operator/.codex/sessions/rollout.jsonl is empty";

    for _ in 0..2 {
        let harness = FakeAppServer::start(vec![FakeStep::error(
            "thread/read",
            json!({"threadId": "target", "includeTurns": false}),
            json!({"code": -32603, "message": native_message}),
        )])
        .await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let error = send_message(
            &client,
            &mut connection,
            ThreadMessageSendInput {
                thread_id: "target".to_owned(),
                prompt: "message".to_owned(),
                model: None,
                reasoning_effort: None,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.category, ToolErrorCategory::NativeError);
        assert_eq!(error.stage, "thread/read");
        assert_eq!(error.thread_id.as_deref(), Some("target"));
        assert_eq!(
            error.message,
            "Codex has not materialized this thread's history yet. No message was sent. Wait a few seconds, then retry `thread_message_send`."
        );
        assert_eq!(error.native.as_ref().unwrap().code, Some(-32603));
        assert_eq!(error.native.as_ref().unwrap().message, native_message);
        assert_eq!(harness.connection_count(), 1);
        assert_eq!(
            harness.log(),
            [json!({
                "method": "thread/read",
                "params": {"threadId": "target", "includeTurns": false},
            })]
        );
    }
}

#[tokio::test]
async fn message_send_preserves_other_pre_read_failures() {
    let harness = FakeAppServer::start(vec![FakeStep::error(
        "thread/read",
        json!({"threadId": "target", "includeTurns": false}),
        native_error(),
    )])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let error = send_message(
        &client,
        &mut connection,
        ThreadMessageSendInput {
            thread_id: "target".to_owned(),
            prompt: "message".to_owned(),
            model: None,
            reasoning_effort: None,
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::NativeError);
    assert_eq!(error.message, "native app-server request failed");
    assert!(error.thread_id.is_none());
    let native = error.native.unwrap();
    assert_eq!(native.code, Some(-32090));
    assert_eq!(native.message, "native mutation rejected");
    assert_eq!(harness.log().len(), 1);
}

#[tokio::test]
async fn idle_message_reads_once_then_starts_with_overrides() {
    let mut steps = snapshot_steps(
        "target",
        json!({"type": "idle"}),
        20,
        Some(native_turn("previous", "completed")),
        false,
    );
    steps.push(FakeStep::result(
        "turn/start",
        json!({
            "threadId": "target",
            "input": [{"type": "text", "text": "message"}],
            "model": "model",
            "effort": "high",
        }),
        json!({"turn": native_turn("started", "inProgress")}),
    ));
    let harness = FakeAppServer::start(steps).await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let result = send_message(
        &client,
        &mut connection,
        ThreadMessageSendInput {
            thread_id: "target".to_owned(),
            prompt: "message".to_owned(),
            model: Some("model".to_owned()),
            reasoning_effort: Some("high".to_owned()),
        },
    )
    .await
    .unwrap();

    assert!(matches!(result.action, ThreadMessageAction::Started));
    assert_eq!(result.turn_id, "started");
    assert_eq!(harness.connection_count(), 1);
    assert_eq!(
        harness
            .log()
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["thread/read", "thread/turns/list", "turn/start"]
    );
}

#[tokio::test]
async fn active_message_reads_once_then_steers_the_exact_turn() {
    let mut steps = snapshot_steps(
        "target",
        json!({"type": "active", "activeFlags": []}),
        20,
        Some(native_turn("active-turn", "inProgress")),
        false,
    );
    steps.push(FakeStep::result(
        "turn/steer",
        json!({
            "threadId": "target",
            "expectedTurnId": "active-turn",
            "input": [{"type": "text", "text": "message"}],
        }),
        json!({"turnId": "active-turn"}),
    ));
    let harness = FakeAppServer::start(steps).await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let result = send_message(
        &client,
        &mut connection,
        ThreadMessageSendInput {
            thread_id: "target".to_owned(),
            prompt: "message".to_owned(),
            model: None,
            reasoning_effort: None,
        },
    )
    .await
    .unwrap();

    assert!(matches!(result.action, ThreadMessageAction::Steered));
    assert_eq!(result.turn_id, "active-turn");
    assert_eq!(harness.connection_count(), 1);
    assert_eq!(
        harness
            .log()
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["thread/read", "thread/turns/list", "turn/steer"]
    );
}

#[tokio::test]
async fn active_message_override_is_rejected_before_prompt_dispatch() {
    for (model, effort) in [
        (Some("model".to_owned()), None),
        (None, Some("high".to_owned())),
    ] {
        let prompt = "ACTIVE_OVERRIDE_PROMPT_SENTINEL";
        let harness = FakeAppServer::start(snapshot_steps(
            "target",
            json!({"type": "active", "activeFlags": []}),
            20,
            Some(native_turn("active-turn", "inProgress")),
            false,
        ))
        .await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let error = send_message(
            &client,
            &mut connection,
            ThreadMessageSendInput {
                thread_id: "target".to_owned(),
                prompt: prompt.to_owned(),
                model,
                reasoning_effort: effort,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.category, ToolErrorCategory::PolicyRejected);
        assert_eq!(harness.log().len(), 2);
        assert!(
            !serde_json::to_string(&harness.log())
                .unwrap()
                .contains(prompt)
        );
    }
}

#[tokio::test]
async fn message_race_never_retries_the_opposite_operation() {
    for (status, latest, method, params) in [
        (
            json!({"type": "idle"}),
            Some(native_turn("previous", "completed")),
            "turn/start",
            json!({
                "threadId": "target",
                "input": [{"type": "text", "text": "message"}],
            }),
        ),
        (
            json!({"type": "active", "activeFlags": []}),
            Some(native_turn("active-turn", "inProgress")),
            "turn/steer",
            json!({
                "threadId": "target",
                "expectedTurnId": "active-turn",
                "input": [{"type": "text", "text": "message"}],
            }),
        ),
    ] {
        let mut steps = snapshot_steps("target", status, 20, latest, false);
        steps.push(FakeStep::error(method, params, native_error()));
        let harness = FakeAppServer::start(steps).await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let error = send_message(
            &client,
            &mut connection,
            ThreadMessageSendInput {
                thread_id: "target".to_owned(),
                prompt: "message".to_owned(),
                model: None,
                reasoning_effort: None,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.category, ToolErrorCategory::NativeError);
        let log = harness.log();
        assert_eq!(log.len(), 3);
        assert_eq!(log[2]["method"], method);
        assert_eq!(
            log.iter()
                .filter(|request| matches!(
                    request["method"].as_str(),
                    Some("turn/start" | "turn/steer")
                ))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn title_and_goal_clear_use_exact_single_requests() {
    let title_harness = FakeAppServer::start(vec![FakeStep::result(
        "thread/name/set",
        json!({"threadId": "target", "name": "title"}),
        json!({}),
    )])
    .await;
    let title_client = AppServerClient::from_config(&title_harness.config);
    let mut title_connection = title_client.connect_initialized().await.unwrap();
    let result = set_title(
        &title_client,
        &mut title_connection,
        ThreadTitleSetInput {
            thread_id: Some("target".to_owned()),
            title: "title".to_owned(),
        },
    )
    .await
    .unwrap();
    assert!(result.is_empty());
    assert_eq!(title_harness.log().len(), 1);

    let clear_harness = FakeAppServer::start(vec![FakeStep::result(
        "thread/goal/clear",
        json!({"threadId": "target"}),
        json!({"cleared": true}),
    )])
    .await;
    let clear_client = AppServerClient::from_config(&clear_harness.config);
    let mut clear_connection = clear_client.connect_initialized().await.unwrap();
    let result = clear_goal(
        &clear_client,
        &mut clear_connection,
        ThreadGoalClearInput {
            thread_id: "target".to_owned(),
        },
    )
    .await
    .unwrap();
    assert!(result.cleared);
    assert_eq!(clear_harness.log().len(), 1);
}

#[tokio::test]
async fn pin_set_noops_when_native_section_already_matches() {
    for (section, requested) in [
        (
            json!({"id": EXPECTED_PINNED_SECTION_ID, "name": "Pinned"}),
            true,
        ),
        (Value::Null, false),
        (json!({"id": "custom-section", "name": "Custom"}), false),
    ] {
        let harness = FakeAppServer::start(vec![pin_read_step(section)]).await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let result = set_pin(
            &client,
            &mut connection,
            ThreadPinSetInput {
                thread_id: Some("target".to_owned()),
                pinned: requested,
            },
        )
        .await
        .unwrap();

        assert_eq!(result.thread_id, "target");
        assert_eq!(result.pinned, requested);
        assert_eq!(harness.connection_count(), 1);
        assert_eq!(
            harness.log(),
            [json!({
                "method": "thread/read",
                "params": {"threadId": "target", "includeTurns": false},
            })]
        );
    }
}

#[tokio::test]
async fn pin_set_treats_omitted_native_section_as_unpinned() {
    let harness = FakeAppServer::start(vec![FakeStep::result(
        "thread/read",
        json!({"threadId": "target", "includeTurns": false}),
        json!({"thread": {"id": "target"}}),
    )])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let result = set_pin(
        &client,
        &mut connection,
        ThreadPinSetInput {
            thread_id: Some("target".to_owned()),
            pinned: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(result.thread_id, "target");
    assert!(!result.pinned);
    assert_eq!(harness.log().len(), 1);
    assert_eq!(harness.log()[0]["method"], "thread/read");
}

#[tokio::test]
async fn pin_set_moves_only_when_native_section_differs() {
    for (section, requested, destination) in [
        (Value::Null, true, json!(EXPECTED_PINNED_SECTION_ID)),
        (
            json!({"id": "custom-section", "name": "Custom"}),
            true,
            json!(EXPECTED_PINNED_SECTION_ID),
        ),
        (
            json!({"id": EXPECTED_PINNED_SECTION_ID, "name": "Pinned"}),
            false,
            Value::Null,
        ),
    ] {
        let harness = FakeAppServer::start(vec![
            pin_read_step(section),
            FakeStep::result(
                "thread/section/move",
                json!({"threadId": "target", "sectionId": destination}),
                json!({}),
            ),
        ])
        .await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let result = set_pin(
            &client,
            &mut connection,
            ThreadPinSetInput {
                thread_id: Some("target".to_owned()),
                pinned: requested,
            },
        )
        .await
        .unwrap();

        assert_eq!(result.thread_id, "target");
        assert_eq!(result.pinned, requested);
        assert_eq!(harness.connection_count(), 1);
        assert_eq!(
            harness
                .log()
                .iter()
                .map(|request| request["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["thread/read", "thread/section/move"]
        );
    }
}

#[tokio::test]
async fn pin_set_rejects_malformed_or_mismatched_pre_reads_without_mutating() {
    for response in [
        json!({}),
        json!({"thread": null}),
        json!({"thread": {}}),
        json!({"thread": {"id": null, "section": null}}),
        json!({"thread": {"id": "other", "section": null}}),
        json!({"thread": {"id": "target", "section": "Pinned"}}),
        json!({"thread": {"id": "target", "section": {}}}),
        json!({"thread": {"id": "target", "section": {"id": 1}}}),
    ] {
        let harness = FakeAppServer::start(vec![FakeStep::result(
            "thread/read",
            json!({"threadId": "target", "includeTurns": false}),
            response,
        )])
        .await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let error = set_pin(
            &client,
            &mut connection,
            ThreadPinSetInput {
                thread_id: Some("target".to_owned()),
                pinned: true,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.category, ToolErrorCategory::NativeError);
        assert_eq!(error.stage, "thread/read");
        assert_eq!(harness.connection_count(), 1);
        assert_eq!(
            harness
                .log()
                .iter()
                .filter(|request| request["method"] == "thread/section/move")
                .count(),
            0
        );
    }
}

#[tokio::test]
async fn pin_set_pre_read_failure_never_mutates() {
    let harness = FakeAppServer::start(vec![FakeStep::error(
        "thread/read",
        json!({"threadId": "target", "includeTurns": false}),
        native_error(),
    )])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let error = set_pin(
        &client,
        &mut connection,
        ThreadPinSetInput {
            thread_id: Some("target".to_owned()),
            pinned: true,
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::NativeError);
    assert_eq!(harness.log().len(), 1);
    assert_eq!(harness.log()[0]["method"], "thread/read");
}

#[tokio::test]
async fn pin_set_rejected_move_returns_native_error_without_replay() {
    let harness = FakeAppServer::start(vec![
        pin_read_step(Value::Null),
        FakeStep::error(
            "thread/section/move",
            json!({"threadId": "target", "sectionId": EXPECTED_PINNED_SECTION_ID}),
            native_error(),
        ),
    ])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let error = set_pin(
        &client,
        &mut connection,
        ThreadPinSetInput {
            thread_id: Some("target".to_owned()),
            pinned: true,
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::NativeError);
    assert_eq!(error.stage, "thread/section/move");
    assert_eq!(
        harness
            .log()
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["thread/read", "thread/section/move"]
    );
}

#[tokio::test]
async fn pin_set_unknown_move_observes_once_without_replay_or_inference() {
    let harness = FakeAppServer::start_connections(vec![
        vec![
            pin_read_step(Value::Null),
            FakeStep {
                method: "thread/section/move",
                params: json!({
                    "threadId": "target",
                    "sectionId": EXPECTED_PINNED_SECTION_ID,
                }),
                response: FakeResponse::Disconnect,
                notify_after: false,
                delay: Duration::ZERO,
            },
        ],
        vec![pin_read_step(json!({
            "id": EXPECTED_PINNED_SECTION_ID,
            "name": "Pinned",
        }))],
    ])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let error = set_pin(
        &client,
        &mut connection,
        ThreadPinSetInput {
            thread_id: Some("target".to_owned()),
            pinned: true,
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::OutcomeUnknown);
    assert_eq!(
        error.observation.as_ref().unwrap()["thread"]["section"]["id"],
        EXPECTED_PINNED_SECTION_ID
    );
    assert_eq!(harness.connection_count(), 2);
    assert_eq!(
        harness
            .log()
            .iter()
            .filter(|request| request["method"] == "thread/section/move")
            .count(),
        1
    );
    assert_eq!(
        harness
            .log()
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["thread/read", "thread/section/move", "thread/read"]
    );
}

#[tokio::test]
async fn pin_set_failed_reconciliation_keeps_unknown_outcome_without_replay() {
    let harness = FakeAppServer::start_connections(vec![
        vec![
            pin_read_step(Value::Null),
            FakeStep {
                method: "thread/section/move",
                params: json!({
                    "threadId": "target",
                    "sectionId": EXPECTED_PINNED_SECTION_ID,
                }),
                response: FakeResponse::Disconnect,
                notify_after: false,
                delay: Duration::ZERO,
            },
        ],
        vec![FakeStep {
            method: "thread/read",
            params: json!({"threadId": "target", "includeTurns": false}),
            response: FakeResponse::Disconnect,
            notify_after: false,
            delay: Duration::ZERO,
        }],
    ])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let error = set_pin(
        &client,
        &mut connection,
        ThreadPinSetInput {
            thread_id: Some("target".to_owned()),
            pinned: true,
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::OutcomeUnknown);
    assert!(error.observation.is_none());
    assert_eq!(
        error.reconciliation_error.as_deref(),
        Some("app-server transport failed")
    );
    assert_eq!(harness.connection_count(), 2);
    assert_eq!(
        harness
            .log()
            .iter()
            .filter(|request| request["method"] == "thread/section/move")
            .count(),
        1
    );
}

#[tokio::test]
async fn pin_set_rejects_non_object_move_acknowledgement() {
    let harness = FakeAppServer::start(vec![
        pin_read_step(Value::Null),
        FakeStep::result(
            "thread/section/move",
            json!({"threadId": "target", "sectionId": EXPECTED_PINNED_SECTION_ID}),
            Value::Null,
        ),
    ])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let error = set_pin(
        &client,
        &mut connection,
        ThreadPinSetInput {
            thread_id: Some("target".to_owned()),
            pinned: true,
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::NativeError);
    assert_eq!(error.stage, "thread/section/move");
    assert_eq!(harness.log().len(), 2);
}

#[tokio::test]
async fn interrupt_targets_only_the_fresh_exact_active_turn() {
    let mut steps = snapshot_steps(
        "target",
        json!({"type": "active", "activeFlags": []}),
        20,
        Some(native_turn("active-turn", "inProgress")),
        false,
    );
    steps.push(FakeStep::result(
        "turn/interrupt",
        json!({"threadId": "target", "turnId": "active-turn"}),
        json!({}),
    ));
    let harness = FakeAppServer::start(steps).await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let result = interrupt_thread(
        &client,
        &mut connection,
        ThreadInterruptInput {
            thread_id: "target".to_owned(),
        },
    )
    .await
    .unwrap();

    assert!(matches!(
        result,
        ThreadInterruptResult::Interrupted {
            interrupted: true,
            ref turn_id
        } if turn_id == "active-turn"
    ));
    assert_eq!(harness.connection_count(), 1);
    let log = harness.log();
    let methods = log
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        ["thread/read", "thread/turns/list", "turn/interrupt"]
    );
    assert!(!methods.iter().any(|method| method.contains("goal")));
}

#[test]
fn interrupt_result_serializes_with_the_public_camel_case_field_name() {
    assert_eq!(
        serde_json::to_value(ThreadInterruptResult::Interrupted {
            interrupted: true,
            turn_id: "active-turn".to_owned(),
        })
        .unwrap(),
        json!({"interrupted": true, "turnId": "active-turn"})
    );
    assert_eq!(
        serde_json::to_value(ThreadInterruptResult::NotInterrupted { interrupted: false }).unwrap(),
        json!({"interrupted": false})
    );
}

#[tokio::test]
async fn terminal_interrupt_returns_false_without_mutation_or_replacement_chase() {
    let harness = FakeAppServer::start(snapshot_steps(
        "target",
        json!({"type": "idle"}),
        20,
        Some(native_turn("terminal-turn", "completed")),
        false,
    ))
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let result = interrupt_thread(
        &client,
        &mut connection,
        ThreadInterruptInput {
            thread_id: "target".to_owned(),
        },
    )
    .await
    .unwrap();

    assert!(matches!(
        result,
        ThreadInterruptResult::NotInterrupted { interrupted: false }
    ));
    assert_eq!(harness.connection_count(), 1);
    assert_eq!(harness.log().len(), 2);
}
