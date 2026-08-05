use super::*;

#[derive(Debug)]
struct MutationExpectation {
    category: ToolErrorCategory,
    mutation_writes: usize,
    reconciliation_reads: usize,
    success_inferred: bool,
}

#[derive(Clone, Copy)]
enum ReconciliationKind {
    Goal,
    LatestTurn,
    CompactThread,
    None,
}

struct Case {
    tool: &'static str,
    mutation_method: &'static str,
    mutation_params: Value,
    reconciliation: ReconciliationKind,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            tool: "thread_create",
            mutation_method: "thread/start",
            mutation_params: json!({"cwd": "/workspace"}),
            reconciliation: ReconciliationKind::None,
        },
        Case {
            tool: "thread_fork",
            mutation_method: "thread/fork",
            mutation_params: json!({
                "threadId": "target",
                "deferGoalContinuation": true,
            }),
            reconciliation: ReconciliationKind::None,
        },
        Case {
            tool: "thread_create",
            mutation_method: "turn/start",
            mutation_params: json!({
                "threadId": "target",
                "input": [{"type": "text", "text": "prompt"}],
            }),
            reconciliation: ReconciliationKind::CompactThread,
        },
        Case {
            tool: "thread_message_send",
            mutation_method: "turn/start",
            mutation_params: json!({
                "threadId": "target",
                "input": [{"type": "text", "text": "prompt"}],
            }),
            reconciliation: ReconciliationKind::CompactThread,
        },
        Case {
            tool: "thread_message_send",
            mutation_method: "turn/steer",
            mutation_params: json!({
                "threadId": "target",
                "expectedTurnId": "turn",
                "input": [{"type": "text", "text": "prompt"}],
            }),
            reconciliation: ReconciliationKind::CompactThread,
        },
        Case {
            tool: "thread_title_set",
            mutation_method: "thread/name/set",
            mutation_params: json!({"threadId": "target", "name": "title"}),
            reconciliation: ReconciliationKind::CompactThread,
        },
        Case {
            tool: "thread_pin_set",
            mutation_method: "thread/metadata/update",
            mutation_params: json!({"threadId": "target", "isPinned": true}),
            reconciliation: ReconciliationKind::CompactThread,
        },
        Case {
            tool: "thread_goal_set",
            mutation_method: "thread/goal/set",
            mutation_params: json!({
                "threadId": "target",
                "objective": "objective",
                "status": "active",
            }),
            reconciliation: ReconciliationKind::Goal,
        },
        Case {
            tool: "thread_goal_pause",
            mutation_method: "thread/goal/set",
            mutation_params: json!({"threadId": "target", "status": "paused"}),
            reconciliation: ReconciliationKind::Goal,
        },
        Case {
            tool: "thread_goal_resume",
            mutation_method: "thread/goal/set",
            mutation_params: json!({"threadId": "target", "status": "active"}),
            reconciliation: ReconciliationKind::Goal,
        },
        Case {
            tool: "thread_goal_clear",
            mutation_method: "thread/goal/clear",
            mutation_params: json!({"threadId": "target"}),
            reconciliation: ReconciliationKind::Goal,
        },
        Case {
            tool: "thread_interrupt",
            mutation_method: "turn/interrupt",
            mutation_params: json!({"threadId": "target", "turnId": "turn"}),
            reconciliation: ReconciliationKind::LatestTurn,
        },
    ]
}

fn reconciliation(kind: ReconciliationKind) -> Reconciliation {
    match kind {
        ReconciliationKind::Goal => Reconciliation::GoalGet {
            thread_id: "target".to_owned(),
        },
        ReconciliationKind::LatestTurn => Reconciliation::LatestTurnRead {
            thread_id: "target".to_owned(),
        },
        ReconciliationKind::CompactThread => Reconciliation::CompactThreadRead {
            thread_id: "target".to_owned(),
        },
        ReconciliationKind::None => Reconciliation::None,
    }
}

fn reconciliation_step(kind: ReconciliationKind) -> Option<FakeStep> {
    match kind {
        ReconciliationKind::Goal => Some(FakeStep::result(
            "thread/goal/get",
            json!({"threadId": "target"}),
            json!({"goal": native_goal("target", "active")}),
        )),
        ReconciliationKind::LatestTurn => Some(FakeStep::result(
            "thread/turns/list",
            json!({
                "threadId": "target",
                "limit": 1,
                "itemsView": "notLoaded",
            }),
            json!({
                "data": [native_turn("replacement-turn", "inProgress")],
                "nextCursor": null
            }),
        )),
        ReconciliationKind::CompactThread => Some(FakeStep::result(
            "thread/read",
            json!({"threadId": "target", "includeTurns": false}),
            json!({
                "thread": native_thread(
                    "target",
                    json!({"type": "idle"}),
                    30
                )
            }),
        )),
        ReconciliationKind::None => None,
    }
}

#[tokio::test]
async fn every_mutation_dispatches_once_and_observes_at_most_once() {
    for case in cases() {
        let mut scripts = vec![vec![FakeStep {
            method: case.mutation_method,
            params: case.mutation_params.clone(),
            response: FakeResponse::Disconnect,
            notify_after: false,
            delay: Duration::ZERO,
        }]];
        if let Some(step) = reconciliation_step(case.reconciliation) {
            scripts.push(vec![step]);
        }
        let expectation = MutationExpectation {
            category: ToolErrorCategory::OutcomeUnknown,
            mutation_writes: 1,
            reconciliation_reads: usize::from(scripts.len() == 2),
            success_inferred: false,
        };
        let harness = FakeAppServer::start_connections(scripts).await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let error = mutation_request(
            &client,
            &mut connection,
            case.tool,
            case.mutation_method,
            case.mutation_params,
            Some("target"),
            (case.mutation_method == "turn/interrupt").then_some("turn"),
            reconciliation(case.reconciliation),
        )
        .await
        .unwrap_err();

        let log = harness.log();
        assert_eq!(error.category, expectation.category, "{}", case.tool);
        assert_eq!(
            error.message,
            "Mutation outcome is unknown. The request may already have been applied.",
            "{}",
            case.tool
        );
        assert_eq!(
            log.iter()
                .filter(|request| request["method"] == case.mutation_method)
                .count(),
            expectation.mutation_writes,
            "{}",
            case.tool
        );
        assert_eq!(
            log.len() - expectation.mutation_writes,
            expectation.reconciliation_reads,
            "{}",
            case.tool
        );
        assert_eq!(
            error.observation.is_some(),
            expectation.reconciliation_reads == 1,
            "{}",
            case.tool
        );
        assert!(!expectation.success_inferred);
        assert_eq!(
            harness.connection_count(),
            1 + expectation.reconciliation_reads,
            "{}",
            case.tool
        );
    }
}

#[tokio::test]
async fn reconciliation_failure_keeps_outcome_unknown_without_replay() {
    let harness = FakeAppServer::start_connections(vec![
        vec![FakeStep {
            method: "thread/name/set",
            params: json!({"threadId": "target", "name": "title"}),
            response: FakeResponse::Disconnect,
            notify_after: false,
            delay: Duration::ZERO,
        }],
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

    let error = mutation_request(
        &client,
        &mut connection,
        "thread_title_set",
        "thread/name/set",
        json!({"threadId": "target", "name": "title"}),
        Some("target"),
        None,
        Reconciliation::CompactThreadRead {
            thread_id: "target".to_owned(),
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
    assert_eq!(
        harness
            .log()
            .iter()
            .filter(|request| request["method"] == "thread/name/set")
            .count(),
        1
    );
    assert_eq!(harness.connection_count(), 2);
}

#[tokio::test]
async fn interrupt_race_preserves_targeted_and_latest_turn_ids_without_inference() {
    let harness = FakeAppServer::start_connections(vec![
        vec![FakeStep {
            method: "turn/interrupt",
            params: json!({
                "threadId": "target",
                "turnId": "original-turn",
            }),
            response: FakeResponse::Disconnect,
            notify_after: false,
            delay: Duration::ZERO,
        }],
        vec![FakeStep::result(
            "thread/turns/list",
            json!({
                "threadId": "target",
                "limit": 1,
                "itemsView": "notLoaded",
            }),
            json!({
                "data": [native_turn("replacement-turn", "inProgress")],
                "nextCursor": null,
            }),
        )],
    ])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let error = mutation_request(
        &client,
        &mut connection,
        "thread_interrupt",
        "turn/interrupt",
        json!({
            "threadId": "target",
            "turnId": "original-turn",
        }),
        Some("target"),
        Some("original-turn"),
        Reconciliation::LatestTurnRead {
            thread_id: "target".to_owned(),
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::OutcomeUnknown);
    assert_eq!(
        error.dispatch,
        Some(crate::error::DispatchState::MayHaveBeenDispatched)
    );
    assert_eq!(error.turn_id.as_deref(), Some("original-turn"));
    assert_eq!(
        error.observation.as_ref().unwrap()["data"][0]["id"],
        "replacement-turn"
    );
    assert_eq!(
        error.observation.as_ref().unwrap()["data"][0]["itemsView"],
        "notLoaded"
    );
    assert_ne!(
        error.observation.as_ref().unwrap()["data"][0]["id"],
        error.turn_id.as_deref().unwrap()
    );
    assert_eq!(
        harness
            .log()
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["turn/interrupt", "thread/turns/list"]
    );
    assert_eq!(harness.connection_count(), 2);
}
