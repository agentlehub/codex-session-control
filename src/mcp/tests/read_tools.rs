use super::*;

#[tokio::test]
async fn list_forwards_omissions_and_explicit_values_without_filtering() {
    let listed = native_thread("archived-thread", json!({"type": "idle"}), 20);
    let omitted = FakeAppServer::start(vec![FakeStep::result(
        "thread/list",
        json!({}),
        json!({"data": [listed.clone()], "nextCursor": null}),
    )])
    .await;
    let result = execute_tool(
        "threads_list",
        ValidatedInput::ThreadsList(ThreadsListInput {
            cursor: None,
            limit: None,
            archived: None,
            cwd: None,
        }),
        &omitted.config,
    )
    .await
    .unwrap();
    assert_eq!(
        result.structured_content,
        Some(json!({
            "threads": [listed],
            "nextCursor": null,
        }))
    );
    assert_eq!(
        omitted.log(),
        vec![json!({"method": "thread/list", "params": {}})]
    );
    assert_eq!(omitted.connection_count(), 1);

    let explicit = FakeAppServer::start(vec![FakeStep::result(
        "thread/list",
        json!({
            "cursor": "cursor-1",
            "limit": 7,
            "archived": true,
            "cwd": "relative-native-cwd",
        }),
        json!({"data": [], "nextCursor": "cursor-2"}),
    )])
    .await;
    let result = execute_tool(
        "threads_list",
        ValidatedInput::ThreadsList(ThreadsListInput {
            cursor: Some("cursor-1".to_owned()),
            limit: Some(7),
            archived: Some(true),
            cwd: Some("relative-native-cwd".to_owned()),
        }),
        &explicit.config,
    )
    .await
    .unwrap();
    assert_eq!(
        result.structured_content,
        Some(json!({"threads": [], "nextCursor": "cursor-2"}))
    );
}

#[tokio::test]
async fn thread_read_uses_one_connection_and_normalizes_optional_nulls() {
    let thread = native_thread("target", json!({"type": "idle"}), 30);
    let turn = json!({
        "id": "turn-1",
        "status": "completed",
        "items": [{"type": "agentMessage", "text": "safe"}],
        "itemsView": "full",
    });
    let harness = FakeAppServer::start(vec![
        FakeStep::result(
            "thread/read",
            json!({"threadId": "target", "includeTurns": false}),
            json!({"thread": thread.clone()}),
        ),
        FakeStep::result(
            "thread/turns/list",
            json!({
                "threadId": "target",
                "cursor": "cursor-1",
                "limit": 4,
                "itemsView": "full",
            }),
            json!({"data": [turn], "nextCursor": null}),
        ),
    ])
    .await;
    let result = execute_tool(
        "thread_read",
        ValidatedInput::ThreadRead(ThreadReadInput {
            thread_id: "target".to_owned(),
            cursor: Some("cursor-1".to_owned()),
            limit: Some(4),
            items_view: Some(TurnItemsView::Full),
        }),
        &harness.config,
    )
    .await
    .unwrap();
    assert_eq!(
        result.structured_content,
        Some(json!({
            "thread": thread,
            "turns": [{
                "id": "turn-1",
                "status": "completed",
                "items": [{"type": "agentMessage", "text": "safe"}],
                "itemsView": "full",
                "startedAt": null,
                "completedAt": null,
                "durationMs": null,
                "error": null,
            }],
            "nextCursor": null,
        }))
    );
    assert_eq!(harness.connection_count(), 1);
    assert_eq!(harness.log().len(), 2);
}

#[tokio::test]
async fn explicit_goal_target_never_loads_cross_authority_caller() {
    let caller = "caller-not-loadable-by-target-authority";
    let target = "explicit-target-on-configured-socket";
    let validated = validate_input(
        "thread_goal_get",
        arguments(json!({"threadId": target})),
        &meta(caller),
    )
    .unwrap();
    let goal = json!({
        "threadId": target,
        "objective": "objective",
        "status": "paused",
        "tokenBudget": null,
        "tokensUsed": 2,
        "timeUsedSeconds": 3,
        "createdAt": 4,
        "updatedAt": 5,
    });
    let harness = FakeAppServer::start(vec![FakeStep::result(
        "thread/goal/get",
        json!({"threadId": target}),
        json!({"goal": goal.clone()}),
    )])
    .await;
    let result = execute_tool("thread_goal_get", validated, &harness.config)
        .await
        .unwrap();
    assert_eq!(result.structured_content, Some(json!({"goal": goal})));
    assert_eq!(
        harness.log(),
        vec![json!({
            "method": "thread/goal/get",
            "params": {"threadId": target},
        })]
    );
    assert!(
        !serde_json::to_string(&harness.log())
            .unwrap()
            .contains(caller)
    );
}
