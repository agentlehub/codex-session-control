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

#[test]
fn mutation_context_owns_attribution_and_derives_reconciliation_from_one_identity() {
    let unscoped = MutationContext::unscoped();
    assert_eq!(unscoped.known_ids(), (None, None));
    assert_eq!(unscoped.reconciliation_request(), None);

    let attributed_only =
        MutationContext::for_thread("thread-context".to_owned(), ReconciliationPolicy::None);
    assert_eq!(attributed_only.known_ids(), (Some("thread-context"), None));
    assert_eq!(attributed_only.reconciliation_request(), None);

    let thread = MutationContext::for_thread(
        "thread-context".to_owned(),
        ReconciliationPolicy::CompactThreadRead,
    );
    assert_eq!(thread.known_ids(), (Some("thread-context"), None));
    assert_eq!(
        thread.reconciliation_request(),
        Some((
            "thread/read",
            json!({"threadId": "thread-context", "includeTurns": false}),
        ))
    );

    let turn = MutationContext::for_turn(
        "thread-context".to_owned(),
        "turn-context".to_owned(),
        ReconciliationPolicy::LatestTurnRead,
    );
    assert_eq!(
        turn.known_ids(),
        (Some("thread-context"), Some("turn-context"))
    );
    assert_eq!(
        turn.reconciliation_request(),
        Some((
            "thread/turns/list",
            json!({
                "threadId": "thread-context",
                "limit": 1,
                "itemsView": "notLoaded",
            }),
        ))
    );

    let goal =
        MutationContext::for_thread("thread-context".to_owned(), ReconciliationPolicy::GoalGet);
    assert_eq!(
        goal.reconciliation_request(),
        Some(("thread/goal/get", json!({"threadId": "thread-context"}),))
    );
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

fn reconciliation_policy(kind: ReconciliationKind) -> ReconciliationPolicy {
    match kind {
        ReconciliationKind::Goal => ReconciliationPolicy::GoalGet,
        ReconciliationKind::LatestTurn => ReconciliationPolicy::LatestTurnRead,
        ReconciliationKind::CompactThread => ReconciliationPolicy::CompactThreadRead,
        ReconciliationKind::None => ReconciliationPolicy::None,
    }
}

fn mutation_context(case: &Case) -> MutationContext {
    match case.mutation_method {
        "thread/start" => MutationContext::unscoped(),
        "turn/interrupt" => MutationContext::for_turn(
            "target".to_owned(),
            "turn".to_owned(),
            reconciliation_policy(case.reconciliation),
        ),
        _ => MutationContext::for_thread(
            "target".to_owned(),
            reconciliation_policy(case.reconciliation),
        ),
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
        let client = harness.client();
        let mut connection = client.connect_initialized().await.unwrap();
        let context = mutation_context(&case);

        let error = mutation_request(
            &client,
            &mut connection,
            case.tool,
            case.mutation_method,
            case.mutation_params,
            &context,
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
    let client = harness.client();
    let mut connection = client.connect_initialized().await.unwrap();
    let context =
        MutationContext::for_thread("target".to_owned(), ReconciliationPolicy::CompactThreadRead);

    let error = mutation_request(
        &client,
        &mut connection,
        "thread_title_set",
        "thread/name/set",
        json!({"threadId": "target", "name": "title"}),
        &context,
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

#[tokio::test(start_paused = true)]
async fn fork_timeout_preserves_context_attribution_without_replay() {
    let harness = FakeAppServer::start(vec![FakeStep {
        method: "thread/fork",
        params: json!({
            "threadId": "target",
            "deferGoalContinuation": true,
        }),
        response: FakeResponse::Pending,
        notify_after: false,
        delay: Duration::ZERO,
    }])
    .await;
    let client = harness.client();
    let mut connection = client.connect_initialized().await.unwrap();

    let operation = tokio::spawn(async move {
        fork_thread(
            &client,
            &mut connection,
            ThreadForkInput {
                thread_id: Some("target".to_owned()),
                defer_goal_continuation: true,
            },
        )
        .await
    });
    harness.wait_for_requests(1).await;

    tokio::time::advance(crate::app_server::NATIVE_STAGE_TIMEOUT).await;
    let error = operation.await.unwrap().unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::OutcomeUnknown);
    assert_eq!(
        error.dispatch,
        Some(crate::error::DispatchState::MayHaveBeenDispatched)
    );
    assert_eq!(error.thread_id.as_deref(), Some("target"));
    assert!(error.turn_id.is_none());
    assert!(error.observation.is_none());
    assert!(error.reconciliation_error.is_none());
    assert_eq!(harness.log().len(), 1);
    assert_eq!(harness.connection_count(), 1);
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
    let client = harness.client();
    let mut connection = client.connect_initialized().await.unwrap();
    let context = MutationContext::for_turn(
        "target".to_owned(),
        "original-turn".to_owned(),
        ReconciliationPolicy::LatestTurnRead,
    );

    let error = mutation_request(
        &client,
        &mut connection,
        "thread_interrupt",
        "turn/interrupt",
        json!({
            "threadId": "target",
            "turnId": "original-turn",
        }),
        &context,
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
