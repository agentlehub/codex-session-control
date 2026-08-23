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
    let client = AppServerClient::from_config(&harness.config);
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

fn native_error(message: &str) -> Value {
    json!({"code": -32090, "message": message})
}

fn methods(harness: &FakeAppServer) -> Vec<String> {
    harness
        .log()
        .into_iter()
        .map(|request| request["method"].as_str().unwrap().to_owned())
        .collect()
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

fn assert_no_goal_operation(harness: &FakeAppServer) {
    assert!(
        harness
            .log()
            .iter()
            .all(|request| !request["method"].as_str().unwrap().contains("goal")),
        "{}",
        serde_json::to_string(&harness.log()).unwrap()
    );
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

    assert_eq!(
        methods(&active_harness),
        [
            "thread/read",
            "thread/turns/list",
            "turn/interrupt",
            "thread/list",
        ]
    );
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

    assert_eq!(
        methods(&inactive_harness),
        ["thread/read", "thread/turns/list", "thread/list"]
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
async fn root_error_stops_before_descendant_discovery() {
    let mut latest_error = vec![FakeStep::result(
        "thread/read",
        json!({"threadId": "root", "includeTurns": false}),
        json!({
            "thread": native_thread("root", json!({"type": "idle"}), 20),
        }),
    )];
    latest_error.push(FakeStep::error(
        "thread/turns/list",
        json!({"threadId": "root", "limit": 1, "itemsView": "notLoaded"}),
        native_error("latest-turn failed"),
    ));

    let mut native_mutation_error = root_active_steps();
    native_mutation_error.pop();
    native_mutation_error.push(FakeStep::error(
        "turn/interrupt",
        json!({"threadId": "root", "turnId": "root-turn"}),
        native_error("interrupt failed"),
    ));

    let mut uncertain_root = root_active_steps();
    uncertain_root.pop();
    uncertain_root.push(FakeStep {
        method: "turn/interrupt",
        params: json!({"threadId": "root", "turnId": "root-turn"}),
        response: FakeResponse::Disconnect,
        notify_after: false,
        delay: Duration::ZERO,
    });

    let cases = [
        (
            ToolErrorCategory::NativeError,
            vec![vec![FakeStep::error(
                "thread/read",
                json!({"threadId": "root", "includeTurns": false}),
                native_error("read failed"),
            )]],
        ),
        (ToolErrorCategory::NativeError, vec![latest_error]),
        (ToolErrorCategory::NativeError, vec![native_mutation_error]),
        (
            ToolErrorCategory::OutcomeUnknown,
            vec![
                uncertain_root,
                vec![FakeStep::result(
                    "thread/turns/list",
                    json!({"threadId": "root", "limit": 1, "itemsView": "notLoaded"}),
                    json!({
                        "data": [native_turn("replacement-turn", "inProgress")],
                        "nextCursor": null,
                    }),
                )],
            ],
        ),
    ];

    for (expected_category, scripts) in cases {
        let harness = FakeAppServer::start_connections(scripts).await;
        let error = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
            .await
            .unwrap_err();

        assert_eq!(error.category, expected_category, "{error:?}");
        assert!(
            !methods(&harness)
                .iter()
                .any(|method| method == "thread/list"),
            "{}",
            serde_json::to_string(&harness.log()).unwrap()
        );
    }
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

    assert_eq!(harness.connection_count(), 1);
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
async fn exact_scope_empty_scan_preserves_root_shape() {
    for data in [
        vec![],
        vec![native_thread("idle-child", json!({"type": "idle"}), 30)],
    ] {
        let mut steps = root_active_steps();
        steps.push(descendant_page("root", None, data, None));
        let harness = FakeAppServer::start(steps).await;

        let result = run_descendant_interrupt(&harness, interrupt_input(false), "caller")
            .await
            .unwrap();

        assert_eq!(harness.connection_count(), 1);
        assert_eq!(
            methods(&harness),
            [
                "thread/read",
                "thread/turns/list",
                "turn/interrupt",
                "thread/list",
            ]
        );
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({"interrupted": true, "turnId": "root-turn"})
        );
    }
}

#[tokio::test]
async fn opted_in_empty_scan_returns_empty_results() {
    let mut steps = root_inactive_steps();
    steps.push(descendant_page("root", None, vec![], None));
    let harness = FakeAppServer::start(steps).await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();

    assert_eq!(harness.connection_count(), 1);
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"interrupted": false, "descendants": {"results": []}})
    );
}

#[tokio::test]
async fn discovery_failure_on_first_or_later_page_preserves_root_and_stops_descendants() {
    let mut first_page_failure = root_active_steps();
    first_page_failure.push(FakeStep::error(
        "thread/list",
        json!({
            "ancestorThreadId": "root",
            "sourceKinds": ["subAgentThreadSpawn"],
        }),
        native_error("first page failed"),
    ));

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
        native_error("later page failed"),
    ));

    for scripts in [vec![first_page_failure], vec![later_page_failure]] {
        let harness = FakeAppServer::start_connections(scripts).await;

        let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
            .await
            .unwrap();

        assert_eq!(harness.connection_count(), 1);
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
        assert!(
            value["descendants"]["error"]["native"]["message"]
                .as_str()
                .unwrap()
                .ends_with("page failed")
        );
        let methods = methods(&harness);
        assert_eq!(
            methods
                .iter()
                .filter(|method| method.as_str() == "thread/read")
                .count(),
            1
        );
        assert_eq!(
            methods
                .iter()
                .filter(|method| method.as_str() == "turn/interrupt")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn discovery_fully_paginates_before_mutation_and_preserves_stable_active_order() {
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
        vec![
            native_thread("B", json!({"type": "active", "activeFlags": []}), 33),
            native_thread("C", json!({"type": "active", "activeFlags": []}), 34),
        ],
        None,
    ));
    let harness = FakeAppServer::start_connections(vec![
        root_steps,
        target_active_steps("A", "A-turn"),
        target_active_steps("B", "B-turn"),
        target_active_steps("C", "C-turn"),
    ])
    .await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({
            "interrupted": false,
            "descendants": {
                "results": [
                    {"threadId": "A", "result": {"interrupted": true, "turnId": "A-turn"}},
                    {"threadId": "B", "result": {"interrupted": true, "turnId": "B-turn"}},
                    {"threadId": "C", "result": {"interrupted": true, "turnId": "C-turn"}},
                ]
            }
        })
    );
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
    for thread_id in ["A", "B", "C"] {
        assert_eq!(interrupt_count(&harness, thread_id), 1);
    }
    assert_eq!(harness.connection_count(), 4);
    assert_no_goal_operation(&harness);
}

#[tokio::test]
async fn active_at_discovery_but_inactive_at_refresh_returns_false_without_mutation() {
    let mut root_steps = root_inactive_steps();
    root_steps.push(descendant_page(
        "root",
        None,
        vec![native_thread(
            "became-idle",
            json!({"type": "active", "activeFlags": []}),
            30,
        )],
        None,
    ));
    let harness = FakeAppServer::start_connections(vec![
        root_steps,
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

    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({
            "interrupted": false,
            "descendants": {
                "results": [
                    {"threadId": "became-idle", "result": {"interrupted": false}},
                ]
            }
        })
    );
    assert_eq!(interrupt_count(&harness, "became-idle"), 0);
    assert_eq!(harness.connection_count(), 2);
    assert_no_goal_operation(&harness);
}

#[tokio::test]
async fn caller_descendant_is_rejected_without_connection_while_other_targets_continue() {
    let mut root_steps = root_inactive_steps();
    root_steps.push(descendant_page(
        "root",
        None,
        vec![
            native_thread("caller", json!({"type": "active", "activeFlags": []}), 30),
            native_thread("other", json!({"type": "active", "activeFlags": []}), 31),
        ],
        None,
    ));
    let harness = FakeAppServer::start_connections(vec![
        root_steps,
        target_active_steps("other", "other-turn"),
    ])
    .await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({
            "interrupted": false,
            "descendants": {
                "results": [
                    {
                        "threadId": "caller",
                        "error": {
                            "category": "policy_rejected",
                            "message": "request rejected by session-control policy",
                            "tool": "thread_interrupt",
                            "stage": "self_target",
                        }
                    },
                    {"threadId": "other", "result": {"interrupted": true, "turnId": "other-turn"}},
                ]
            }
        })
    );
    assert_eq!(harness.connection_count(), 2);
    assert!(harness.log().iter().all(|request| {
        request["params"].get("threadId").and_then(Value::as_str) != Some("caller")
    }));
    assert_eq!(interrupt_count(&harness, "caller"), 0);
    assert_eq!(interrupt_count(&harness, "other"), 1);
    assert_no_goal_operation(&harness);
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
            native_thread("B", json!({"type": "active", "activeFlags": []}), 31),
        ],
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
    let harness = FakeAppServer::start_connections(vec![root_steps, a_steps, b_steps]).await;
    let config = harness.config.clone();

    let operation = tokio::spawn(async move {
        let client = AppServerClient::from_config(&config);
        let mut root_connection = client.connect_initialized().await.unwrap();
        interrupt_thread(
            &client,
            &mut root_connection,
            interrupt_input(true),
            "caller".to_owned(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        harness.wait_for_requests(5).await;
    })
    .await
    .expect("both target reads must dispatch before either response is released");
    release_b.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        harness.wait_for_requests(7).await;
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
    assert_no_goal_operation(&harness);
}

#[tokio::test]
async fn descendant_connection_snapshot_and_native_errors_are_isolated() {
    let mut root_steps = root_inactive_steps();
    root_steps.push(descendant_page(
        "root",
        None,
        [
            "init-failure",
            "snapshot-failure",
            "mutation-failure",
            "success",
        ]
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
        native_error("snapshot failed"),
    )];
    let mut mutation_failure = target_active_steps("mutation-failure", "mutation-turn");
    *mutation_failure.last_mut().unwrap() = FakeStep::error(
        "turn/interrupt",
        json!({"threadId": "mutation-failure", "turnId": "mutation-turn"}),
        native_error("mutation failed"),
    );
    let harness = FakeAppServer::start_scripted_connections(vec![
        FakeConnectionScript::initialized(root_steps),
        FakeConnectionScript::disconnect_on_initialize(),
        FakeConnectionScript::initialized(snapshot_failure),
        FakeConnectionScript::initialized(mutation_failure),
        FakeConnectionScript::initialized(target_active_steps("success", "success-turn")),
    ])
    .await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();
    let value = serde_json::to_value(result).unwrap();
    let results = value["descendants"]["results"].as_array().unwrap();

    assert_eq!(
        results
            .iter()
            .map(|entry| entry["threadId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "init-failure",
            "snapshot-failure",
            "mutation-failure",
            "success"
        ]
    );
    assert_eq!(
        results[0]["error"]["category"],
        "authority_transport_failure"
    );
    assert_eq!(results[0]["error"]["tool"], "thread_interrupt");
    assert_eq!(results[0]["error"]["stage"], "initialize");
    assert_eq!(results[0]["error"]["threadId"], "init-failure");
    assert_eq!(results[1]["error"]["category"], "native_error");
    assert_eq!(results[1]["error"]["tool"], "thread_interrupt");
    assert_eq!(results[1]["error"]["stage"], "thread/read");
    assert_eq!(results[1]["error"]["threadId"], "snapshot-failure");
    assert_eq!(results[1]["error"]["native"]["message"], "snapshot failed");
    assert_eq!(results[2]["error"]["category"], "native_error");
    assert_eq!(results[2]["error"]["tool"], "thread_interrupt");
    assert_eq!(results[2]["error"]["stage"], "turn/interrupt");
    assert_eq!(results[2]["error"]["threadId"], "mutation-failure");
    assert_eq!(results[2]["error"]["turnId"], "mutation-turn");
    assert_eq!(results[2]["error"]["native"]["message"], "mutation failed");
    assert_eq!(results[2]["error"]["dispatch"], "may_have_been_dispatched");
    assert_eq!(
        results[3]["result"],
        json!({"interrupted": true, "turnId": "success-turn"})
    );
    assert_eq!(harness.connection_count(), 5);
    for (thread_id, expected) in [
        ("init-failure", 0),
        ("snapshot-failure", 0),
        ("mutation-failure", 1),
        ("success", 1),
    ] {
        assert_eq!(interrupt_count(&harness, thread_id), expected);
    }
    assert_no_goal_operation(&harness);
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
    let harness = FakeAppServer::start_connections(vec![root_steps, slow_steps, fast_steps]).await;
    let config = harness.config.clone();

    let operation = tokio::spawn(async move {
        let client = AppServerClient::from_config(&config);
        let mut root_connection = client.connect_initialized().await.unwrap();
        interrupt_thread(
            &client,
            &mut root_connection,
            interrupt_input(true),
            "caller".to_owned(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        harness.wait_for_requests(7).await;
        fast_response_sent.notified().await;
    })
    .await
    .expect("fast mutation must dispatch before the slow snapshot timeout");
    assert!(!operation.is_finished());

    tokio::time::pause();
    tokio::time::advance(crate::app_server::NATIVE_STAGE_TIMEOUT).await;
    let result = operation.await.unwrap().unwrap();
    let value = serde_json::to_value(result).unwrap();
    let results = value["descendants"]["results"].as_array().unwrap();

    assert_eq!(results[0]["threadId"], "slow");
    assert_eq!(results[0]["error"]["category"], "stage_timeout");
    assert_eq!(results[0]["error"]["tool"], "thread_interrupt");
    assert_eq!(results[0]["error"]["stage"], "thread/read");
    assert_eq!(results[0]["error"]["threadId"], "slow");
    assert_eq!(results[1]["threadId"], "fast");
    assert_eq!(
        results[1]["result"],
        json!({"interrupted": true, "turnId": "fast-turn"})
    );
    assert_eq!(interrupt_count(&harness, "slow"), 0);
    assert_eq!(interrupt_count(&harness, "fast"), 1);
    assert_eq!(harness.connection_count(), 3);
    assert_no_goal_operation(&harness);
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
        uncertain_steps,
        target_active_steps("success", "success-turn"),
        reconciliation_steps,
    ])
    .await;

    let result = run_descendant_interrupt(&harness, interrupt_input(true), "caller")
        .await
        .unwrap();
    let value = serde_json::to_value(result).unwrap();
    let results = value["descendants"]["results"].as_array().unwrap();

    assert_eq!(results[0]["threadId"], "uncertain");
    assert_eq!(results[0]["error"]["category"], "outcome_unknown");
    assert_eq!(
        results[0]["error"]["message"],
        "Mutation outcome is unknown. The request may already have been applied."
    );
    assert_eq!(results[0]["error"]["tool"], "thread_interrupt");
    assert_eq!(results[0]["error"]["stage"], "turn/interrupt");
    assert_eq!(results[0]["error"]["threadId"], "uncertain");
    assert_eq!(results[0]["error"]["turnId"], "uncertain-turn");
    assert_eq!(results[0]["error"]["dispatch"], "may_have_been_dispatched");
    assert_eq!(
        results[0]["error"]["observation"]["data"][0]["id"],
        "replacement-turn"
    );
    assert_eq!(
        results[0]["error"]["observation"]["data"][0]["itemsView"],
        "notLoaded"
    );
    assert!(results[0]["error"].get("reconciliationError").is_none());
    assert_eq!(results[1]["threadId"], "success");
    assert_eq!(
        results[1]["result"],
        json!({"interrupted": true, "turnId": "success-turn"})
    );
    assert_eq!(interrupt_count(&harness, "uncertain"), 1);
    assert_eq!(interrupt_count(&harness, "success"), 1);
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
    assert_eq!(harness.connection_count(), 4);
    assert_no_goal_operation(&harness);
}
