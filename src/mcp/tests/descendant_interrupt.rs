use super::*;

fn descendant_page(
    root: &str,
    cursor: Option<&str>,
    data: Vec<Value>,
    next_cursor: Option<&str>,
) -> FakeStep {
    let mut params = serde_json::Map::new();
    params.insert("ancestorThreadId".to_owned(), json!(root));
    params.insert("sourceKinds".to_owned(), json!(["subAgentThreadSpawn"]));
    if let Some(cursor) = cursor {
        params.insert("cursor".to_owned(), json!(cursor));
    }
    FakeStep::result(
        "thread/list",
        Value::Object(params),
        json!({"data": data, "nextCursor": next_cursor}),
    )
}

async fn run_descendant_interrupt(
    harness: &FakeAppServer,
    input: ThreadInterruptInput,
    caller_thread_id: &str,
) -> Result<ThreadInterruptResult, ToolErrorData> {
    let client = harness.client();
    let mut root_connection = client.connect_initialized().await.unwrap();
    interrupt_thread(
        &client,
        &mut root_connection,
        input,
        caller_thread_id.to_owned(),
    )
    .await
}

fn interrupt_input(include_descendants: bool) -> ThreadInterruptInput {
    ThreadInterruptInput {
        thread_id: "root".to_owned(),
        include_descendants,
    }
}

fn root_active_steps() -> Vec<FakeStep> {
    let mut steps = snapshot_steps(
        "root",
        json!({"type": "active", "activeFlags": []}),
        20,
        Some(native_turn("root-turn", "inProgress")),
        false,
    );
    steps.push(FakeStep::result(
        "turn/interrupt",
        json!({"threadId": "root", "turnId": "root-turn"}),
        json!({}),
    ));
    steps
}

fn root_inactive_steps() -> Vec<FakeStep> {
    snapshot_steps(
        "root",
        json!({"type": "idle"}),
        20,
        Some(native_turn("previous-turn", "completed")),
        false,
    )
}

fn native_error() -> Value {
    json!({"code": -32090, "message": "native failure"})
}

fn target_active_steps(thread_id: &'static str, turn_id: &'static str) -> Vec<FakeStep> {
    let mut steps = snapshot_steps(
        thread_id,
        json!({"type": "active", "activeFlags": []}),
        40,
        Some(native_turn(turn_id, "inProgress")),
        false,
    );
    steps.push(FakeStep::result(
        "turn/interrupt",
        json!({"threadId": thread_id, "turnId": turn_id}),
        json!({}),
    ));
    steps
}

fn interrupt_count(harness: &FakeAppServer, thread_id: &str) -> usize {
    harness
        .log()
        .iter()
        .filter(|request| {
            request["method"] == "turn/interrupt" && request["params"]["threadId"] == thread_id
        })
        .count()
}

fn descendant_entry<'a>(results: &'a [Value], thread_id: &str) -> &'a Value {
    results
        .iter()
        .find(|entry| entry["threadId"].as_str() == Some(thread_id))
        .unwrap_or_else(|| panic!("missing descendant result for {thread_id}"))
}

#[tokio::test]
async fn root_interrupt_precedes_discovery_and_inactive_root_still_discovers() {
    let mut active_steps = root_active_steps();
    active_steps.push(descendant_page(
        "root",
        None,
        vec![native_thread(
            "active-child",
            json!({"type": "active", "activeFlags": []}),
            30,
        )],
        None,
    ));
    let active_harness = FakeAppServer::start(active_steps).await;

    let active = run_descendant_interrupt(&active_harness, interrupt_input(false), "caller")
        .await
        .unwrap();

    let active_log = active_harness.log();
    let root_interrupt = active_log
        .iter()
        .position(|request| {
            request["method"] == "turn/interrupt" && request["params"]["threadId"] == "root"
        })
        .unwrap();
    let discovery = active_log
        .iter()
        .position(|request| request["method"] == "thread/list")
        .unwrap();
    assert!(root_interrupt < discovery, "{active_log:?}");
    assert_eq!(
        serde_json::to_value(active).unwrap(),
        json!({
            "interrupted": true,
            "turnId": "root-turn",
            "descendants": {
                "warning": {
                    "code": "active_descendants_not_interrupted",
                    "activeCount": 1,
                    "activeThreadIds": ["active-child"],
                }
            }
        })
    );

    let mut inactive_steps = root_inactive_steps();
    inactive_steps.push(descendant_page(
        "root",
        None,
        vec![native_thread(
            "active-child",
            json!({"type": "active", "activeFlags": []}),
            30,
        )],
        None,
    ));
    let inactive_harness = FakeAppServer::start(inactive_steps).await;

    let inactive = run_descendant_interrupt(&inactive_harness, interrupt_input(false), "caller")
        .await
        .unwrap();

    let inactive_log = inactive_harness.log();
    assert!(
        inactive_log
            .iter()
            .any(|request| request["method"] == "thread/list")
    );
    assert!(
        inactive_log
            .iter()
            .all(|request| request["method"] != "turn/interrupt")
    );
    assert_eq!(
        serde_json::to_value(inactive).unwrap(),
        json!({
            "interrupted": false,
            "descendants": {
                "warning": {
                    "code": "active_descendants_not_interrupted",
                    "activeCount": 1,
                    "activeThreadIds": ["active-child"],
                }
            }
        })
    );
}

#[tokio::test]
async fn root_malformed_active_turn_stops_before_descendant_discovery() {
    let mut malformed_active_turn = snapshot_steps(
        "root",
        json!({"type": "active", "activeFlags": []}),
        20,
        None,
        false,
    );
    malformed_active_turn[1].response = FakeResponse::Result(json!({
        "data": [{"status": "inProgress"}],
        "nextCursor": null,
    }));
    let harness = FakeAppServer::start(malformed_active_turn).await;
    let error = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap_err();

    assert_eq!(error.category, ToolErrorCategory::NativeError);
    assert_eq!(error.stage, "thread/turns/list");
    assert!(
        harness
            .log()
            .iter()
            .all(|request| request["method"] != "thread/list")
    );
}

#[tokio::test]
async fn discovery_paginates_and_warning_preserves_stable_active_order() {
    let mut steps = root_inactive_steps();
    steps.push(descendant_page(
        "root",
        None,
        vec![
            native_thread(
                "active-child",
                json!({"type": "active", "activeFlags": []}),
                30,
            ),
            native_thread("idle-child", json!({"type": "idle"}), 31),
            native_thread(
                "deep-active",
                json!({"type": "active", "activeFlags": []}),
                32,
            ),
        ],
        Some("second-page"),
    ));
    steps.push(descendant_page(
        "root",
        Some("second-page"),
        vec![
            native_thread(
                "deep-active",
                json!({"type": "active", "activeFlags": []}),
                33,
            ),
            native_thread(
                "deeper-active",
                json!({"type": "active", "activeFlags": []}),
                34,
            ),
        ],
        None,
    ));
    let harness = FakeAppServer::start(steps).await;

    let result = run_descendant_interrupt(&harness, interrupt_input(false), "caller")
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({
            "interrupted": false,
            "descendants": {
                "warning": {
                    "code": "active_descendants_not_interrupted",
                    "activeCount": 3,
                    "activeThreadIds": ["active-child", "deep-active", "deeper-active"],
                }
            }
        })
    );
}

#[tokio::test]
async fn empty_descendant_scan_scope_defaults_and_opt_in_use_public_tool_result() {
    for (public_input, mut steps, expected) in [
        (
            json!({"threadId": "root"}),
            root_active_steps(),
            json!({"interrupted": true, "turnId": "root-turn"}),
        ),
        (
            json!({"threadId": "root", "includeDescendants": false}),
            root_active_steps(),
            json!({"interrupted": true, "turnId": "root-turn"}),
        ),
        (
            json!({"threadId": "root", "includeDescendants": true}),
            root_inactive_steps(),
            json!({"interrupted": false, "descendants": {"results": []}}),
        ),
    ] {
        steps.push(descendant_page("root", None, vec![], None));
        let harness = FakeAppServer::start(steps).await;
        let validated =
            validate_input("thread_interrupt", arguments(public_input), &meta("caller")).unwrap();

        let result = execute_tool("thread_interrupt", validated, &harness.client())
            .await
            .unwrap();

        assert_eq!(result.structured_content, Some(expected));
    }
}

#[tokio::test]
async fn later_page_discovery_failure_preserves_root_and_stops_descendants() {
    let mut later_page_failure = root_active_steps();
    later_page_failure.push(descendant_page(
        "root",
        None,
        vec![native_thread(
            "active-child",
            json!({"type": "active", "activeFlags": []}),
            30,
        )],
        Some("second-page"),
    ));
    later_page_failure.push(FakeStep::error(
        "thread/list",
        json!({
            "ancestorThreadId": "root",
            "sourceKinds": ["subAgentThreadSpawn"],
            "cursor": "second-page",
        }),
        native_error(),
    ));

    let harness = FakeAppServer::start(later_page_failure).await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();

    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["interrupted"], true);
    assert_eq!(value["turnId"], "root-turn");
    assert_eq!(
        value["descendants"]["error"]["code"],
        "descendant_discovery_failed"
    );
    assert_eq!(value["descendants"]["error"]["category"], "native_error");
    assert_eq!(value["descendants"]["error"]["tool"], "thread_interrupt");
    assert_eq!(value["descendants"]["error"]["stage"], "thread/list");
    assert_eq!(value["descendants"]["error"]["threadId"], "root");
    assert_eq!(value["descendants"]["error"]["native"]["code"], -32090);
    assert!(harness.log().iter().all(|request| {
        request["params"].get("threadId").and_then(Value::as_str) != Some("active-child")
    }));
}

#[tokio::test]
async fn refresh_and_caller_policy_are_isolated_while_other_target_succeeds() {
    let mut root_steps = root_inactive_steps();
    root_steps.push(descendant_page(
        "root",
        None,
        vec![
            native_thread(
                "became-idle",
                json!({"type": "active", "activeFlags": []}),
                30,
            ),
            native_thread("caller", json!({"type": "active", "activeFlags": []}), 31),
            native_thread("other", json!({"type": "active", "activeFlags": []}), 32),
        ],
        None,
    ));
    let harness = FakeAppServer::start_connections(vec![
        root_steps,
        target_active_steps("other", "other-turn"),
        snapshot_steps(
            "became-idle",
            json!({"type": "idle"}),
            40,
            Some(native_turn("completed-turn", "completed")),
            false,
        ),
    ])
    .await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();

    let results = serde_json::to_value(result).unwrap()["descendants"]["results"]
        .as_array()
        .unwrap()
        .to_owned();
    let became_idle = descendant_entry(&results, "became-idle");
    let caller = descendant_entry(&results, "caller");
    let other = descendant_entry(&results, "other");
    assert_eq!(became_idle["result"], json!({"interrupted": false}));
    assert_eq!(caller["error"]["category"], "policy_rejected");
    assert_eq!(caller["error"]["stage"], "self_target");
    assert_eq!(
        other["result"],
        json!({"interrupted": true, "turnId": "other-turn"})
    );
    assert_eq!(interrupt_count(&harness, "became-idle"), 0);
    assert_eq!(interrupt_count(&harness, "caller"), 0);
    assert!(harness.log().iter().all(|request| {
        request["params"].get("threadId").and_then(Value::as_str) != Some("caller")
    }));
}

#[tokio::test]
async fn descendant_attempts_overlap_and_reversed_completion_preserves_discovery_order() {
    let release_a = Arc::new(tokio::sync::Notify::new());
    let release_b = Arc::new(tokio::sync::Notify::new());
    let b_response_sent = Arc::new(tokio::sync::Notify::new());
    let mut root_steps = root_inactive_steps();
    root_steps.push(descendant_page(
        "root",
        None,
        vec![
            native_thread("A", json!({"type": "active", "activeFlags": []}), 30),
            native_thread("idle", json!({"type": "idle"}), 31),
            native_thread("B", json!({"type": "active", "activeFlags": []}), 32),
        ],
        Some("second-page"),
    ));
    root_steps.push(descendant_page(
        "root",
        Some("second-page"),
        vec![native_thread(
            "B",
            json!({"type": "active", "activeFlags": []}),
            33,
        )],
        None,
    ));
    let mut a_steps = target_active_steps("A", "A-turn");
    a_steps[0] = a_steps[0]
        .clone()
        .controlled(Some(Arc::clone(&release_a)), None);
    let mut b_steps = target_active_steps("B", "B-turn");
    b_steps[0] = b_steps[0]
        .clone()
        .controlled(Some(Arc::clone(&release_b)), None);
    b_steps[2] = b_steps[2]
        .clone()
        .controlled(None, Some(Arc::clone(&b_response_sent)));
    let harness = FakeAppServer::start_connections(vec![root_steps, b_steps, a_steps]).await;
    let client = harness.client();

    let operation = tokio::spawn(async move {
        let mut root_connection = client.connect_initialized().await.unwrap();
        interrupt_thread(
            &client,
            &mut root_connection,
            interrupt_input(true),
            "caller".to_owned(),
        )
        .await
    });
    let target_reads = [
        (
            "thread/read",
            json!({"threadId": "A", "includeTurns": false}),
        ),
        (
            "thread/read",
            json!({"threadId": "B", "includeTurns": false}),
        ),
    ];
    tokio::time::timeout(
        Duration::from_secs(1),
        harness.wait_for_requests_matching(&target_reads),
    )
    .await
    .expect("both target reads must dispatch before either response is released");
    release_b.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        b_response_sent.notified().await;
    })
    .await
    .expect("B must complete while A's snapshot response remains gated");
    assert!(!operation.is_finished());
    release_a.notify_one();
    let result = operation.await.unwrap().unwrap();

    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({
            "interrupted": false,
            "descendants": {
                "results": [
                    {"threadId": "A", "result": {"interrupted": true, "turnId": "A-turn"}},
                    {"threadId": "B", "result": {"interrupted": true, "turnId": "B-turn"}},
                ]
            }
        })
    );
    assert_eq!(interrupt_count(&harness, "A"), 1);
    assert_eq!(interrupt_count(&harness, "B"), 1);
    let log = harness.log();
    let last_discovery = log
        .iter()
        .rposition(|request| request["method"] == "thread/list")
        .unwrap();
    let first_target_read = log
        .iter()
        .position(|request| {
            request["method"] == "thread/read" && request["params"]["threadId"] != "root"
        })
        .unwrap();
    assert!(last_discovery < first_target_read, "{log:?}");
    assert!(log.iter().all(|request| {
        request["params"].get("threadId").and_then(Value::as_str) != Some("idle")
    }));
}

#[tokio::test]
async fn descendant_initialization_failure_is_isolated_after_root_discovery() {
    let mut root_steps = root_inactive_steps();
    root_steps.push(descendant_page(
        "root",
        None,
        vec![native_thread(
            "init-failure",
            json!({"type": "active", "activeFlags": []}),
            30,
        )],
        None,
    ));
    let harness = FakeAppServer::start_with_initialization_disconnect(root_steps).await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();
    let results = serde_json::to_value(result).unwrap()["descendants"]["results"]
        .as_array()
        .unwrap()
        .to_owned();
    let initialization_failure = descendant_entry(&results, "init-failure");

    assert_eq!(
        initialization_failure["error"]["category"],
        "authority_transport_failure"
    );
    assert_eq!(initialization_failure["error"]["stage"], "initialize");
    assert_eq!(initialization_failure["error"]["threadId"], "init-failure");
}

#[tokio::test]
async fn descendant_snapshot_and_native_errors_are_isolated() {
    let mut root_steps = root_inactive_steps();
    root_steps.push(descendant_page(
        "root",
        None,
        ["snapshot-failure", "mutation-failure", "success"]
            .into_iter()
            .enumerate()
            .map(|(index, thread_id)| {
                native_thread(
                    thread_id,
                    json!({"type": "active", "activeFlags": []}),
                    30 + index as i64,
                )
            })
            .collect(),
        None,
    ));
    let snapshot_failure = vec![FakeStep::error(
        "thread/read",
        json!({"threadId": "snapshot-failure", "includeTurns": false}),
        native_error(),
    )];
    let mut mutation_failure = target_active_steps("mutation-failure", "mutation-turn");
    *mutation_failure.last_mut().unwrap() = FakeStep::error(
        "turn/interrupt",
        json!({"threadId": "mutation-failure", "turnId": "mutation-turn"}),
        native_error(),
    );
    let harness = FakeAppServer::start_connections(vec![
        root_steps,
        target_active_steps("success", "success-turn"),
        mutation_failure,
        snapshot_failure,
    ])
    .await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();
    let value = serde_json::to_value(result).unwrap();
    let results = value["descendants"]["results"].as_array().unwrap();

    let snapshot_failure = descendant_entry(results, "snapshot-failure");
    let mutation_failure = descendant_entry(results, "mutation-failure");
    let success = descendant_entry(results, "success");
    assert_eq!(snapshot_failure["error"]["category"], "native_error");
    assert_eq!(snapshot_failure["error"]["tool"], "thread_interrupt");
    assert_eq!(snapshot_failure["error"]["stage"], "thread/read");
    assert_eq!(snapshot_failure["error"]["threadId"], "snapshot-failure");
    assert_eq!(mutation_failure["error"]["category"], "native_error");
    assert_eq!(mutation_failure["error"]["stage"], "turn/interrupt");
    assert_eq!(mutation_failure["error"]["threadId"], "mutation-failure");
    assert_eq!(mutation_failure["error"]["turnId"], "mutation-turn");
    assert_eq!(
        mutation_failure["error"]["dispatch"],
        "may_have_been_dispatched"
    );
    assert_eq!(
        success["result"],
        json!({"interrupted": true, "turnId": "success-turn"})
    );
}

#[tokio::test]
async fn descendant_timeout_is_isolated_and_does_not_block_other_dispatch() {
    let fast_response_sent = Arc::new(tokio::sync::Notify::new());
    let mut root_steps = root_inactive_steps();
    root_steps.push(descendant_page(
        "root",
        None,
        vec![
            native_thread("slow", json!({"type": "active", "activeFlags": []}), 30),
            native_thread("fast", json!({"type": "active", "activeFlags": []}), 31),
        ],
        None,
    ));
    let slow_steps = vec![FakeStep {
        method: "thread/read",
        params: json!({"threadId": "slow", "includeTurns": false}),
        response: FakeResponse::Pending,
        notify_after: false,
        delay: Duration::ZERO,
    }];
    let mut fast_steps = target_active_steps("fast", "fast-turn");
    *fast_steps.last_mut().unwrap() = fast_steps
        .last()
        .unwrap()
        .clone()
        .controlled(None, Some(Arc::clone(&fast_response_sent)));
    let harness = FakeAppServer::start_connections(vec![root_steps, fast_steps, slow_steps]).await;
    let client = harness.client();

    let operation = tokio::spawn(async move {
        let mut root_connection = client.connect_initialized().await.unwrap();
        interrupt_thread(
            &client,
            &mut root_connection,
            interrupt_input(true),
            "caller".to_owned(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), fast_response_sent.notified())
        .await
        .expect("fast mutation must dispatch before the slow snapshot timeout");
    assert!(!operation.is_finished());

    tokio::time::pause();
    tokio::time::advance(crate::app_server::NATIVE_STAGE_TIMEOUT).await;
    let result = operation.await.unwrap().unwrap();
    let value = serde_json::to_value(result).unwrap();
    let results = value["descendants"]["results"].as_array().unwrap();

    let slow = descendant_entry(results, "slow");
    let fast = descendant_entry(results, "fast");
    assert_eq!(slow["error"]["category"], "stage_timeout");
    assert_eq!(slow["error"]["stage"], "thread/read");
    assert_eq!(slow["error"]["threadId"], "slow");
    assert_eq!(
        fast["result"],
        json!({"interrupted": true, "turnId": "fast-turn"})
    );
    assert_eq!(interrupt_count(&harness, "slow"), 0);
}

#[tokio::test]
async fn descendant_outcome_unknown_retains_evidence_and_is_never_retried() {
    let mut root_steps = root_inactive_steps();
    root_steps.push(descendant_page(
        "root",
        None,
        vec![
            native_thread(
                "uncertain",
                json!({"type": "active", "activeFlags": []}),
                30,
            ),
            native_thread("success", json!({"type": "active", "activeFlags": []}), 31),
        ],
        None,
    ));
    let mut uncertain_steps = target_active_steps("uncertain", "uncertain-turn");
    uncertain_steps.pop();
    uncertain_steps.push(FakeStep {
        method: "turn/interrupt",
        params: json!({"threadId": "uncertain", "turnId": "uncertain-turn"}),
        response: FakeResponse::Disconnect,
        notify_after: false,
        delay: Duration::ZERO,
    });
    let reconciliation_steps = vec![FakeStep::result(
        "thread/turns/list",
        json!({"threadId": "uncertain", "limit": 1, "itemsView": "notLoaded"}),
        json!({
            "data": [native_turn("replacement-turn", "inProgress")],
            "nextCursor": null,
        }),
    )];
    let harness = FakeAppServer::start_connections(vec![
        root_steps,
        target_active_steps("success", "success-turn"),
        reconciliation_steps,
        uncertain_steps,
    ])
    .await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();
    let value = serde_json::to_value(result).unwrap();
    let results = value["descendants"]["results"].as_array().unwrap();

    let uncertain = descendant_entry(results, "uncertain");
    let success = descendant_entry(results, "success");
    assert_eq!(uncertain["error"]["category"], "outcome_unknown");
    assert_eq!(uncertain["error"]["stage"], "turn/interrupt");
    assert_eq!(uncertain["error"]["threadId"], "uncertain");
    assert_eq!(uncertain["error"]["turnId"], "uncertain-turn");
    assert_eq!(uncertain["error"]["dispatch"], "may_have_been_dispatched");
    assert_eq!(
        uncertain["error"]["observation"]["data"][0]["id"],
        "replacement-turn"
    );
    assert_eq!(
        success["result"],
        json!({"interrupted": true, "turnId": "success-turn"})
    );
    assert_eq!(interrupt_count(&harness, "uncertain"), 1);
    assert_eq!(
        harness
            .log()
            .iter()
            .filter(|request| {
                request["method"] == "thread/turns/list"
                    && request["params"]["threadId"] == "uncertain"
            })
            .count(),
        2
    );
}
