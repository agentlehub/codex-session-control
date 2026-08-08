use super::*;

const GOAL_CASES: [(&str, &str, &str); 12] = [
    ("set_any", "any", "active"),
    ("pause_active", "active", "paused"),
    ("pause_paused", "paused", "paused_noop"),
    ("pause_blocked", "blocked", "native_rejection"),
    ("pause_usage_limited", "usageLimited", "native_rejection"),
    ("pause_budget_limited", "budgetLimited", "native_rejection"),
    ("pause_complete", "complete", "native_rejection"),
    ("resume_paused", "paused", "active"),
    ("resume_blocked", "blocked", "active"),
    ("resume_usage_limited", "usageLimited", "active"),
    ("resume_active", "active", "active_noop"),
    ("resume_budget_or_complete", "terminal", "native_rejection"),
];

#[tokio::test]
async fn native_owns_the_complete_one_request_goal_matrix() {
    for (operation, _initial, outcome) in GOAL_CASES {
        let (tool, params, result_status) = if operation == "set_any" {
            (
                "thread_goal_set",
                json!({
                    "threadId": "target",
                    "objective": "replacement objective",
                    "status": "active",
                }),
                "active",
            )
        } else if operation.starts_with("pause_") {
            (
                "thread_goal_pause",
                json!({"threadId": "target", "status": "paused"}),
                "paused",
            )
        } else {
            (
                "thread_goal_resume",
                json!({"threadId": "target", "status": "active"}),
                "active",
            )
        };
        let step = if outcome == "native_rejection" {
            FakeStep::error(
                "thread/goal/set",
                params,
                json!({"code": -32090, "message": "transition rejected"}),
            )
        } else {
            FakeStep::result(
                "thread/goal/set",
                params,
                json!({"goal": native_goal("target", result_status)}),
            )
        };
        let harness = FakeAppServer::start(vec![step]).await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let result = match tool {
            "thread_goal_set" => {
                set_goal(
                    &client,
                    &mut connection,
                    ThreadGoalSetInput {
                        thread_id: "target".to_owned(),
                        objective: "replacement objective".to_owned(),
                    },
                )
                .await
            }
            "thread_goal_pause" => {
                pause_goal(
                    &client,
                    &mut connection,
                    ThreadGoalPauseInput {
                        thread_id: "target".to_owned(),
                    },
                )
                .await
            }
            "thread_goal_resume" => {
                resume_goal(
                    &client,
                    &mut connection,
                    ThreadGoalResumeInput {
                        thread_id: "target".to_owned(),
                    },
                )
                .await
            }
            _ => unreachable!(),
        };

        if outcome == "native_rejection" {
            assert_eq!(
                result.unwrap_err().category,
                ToolErrorCategory::NativeError,
                "{operation}"
            );
        } else {
            let goal = result.unwrap();
            assert_eq!(
                serde_json::to_value(goal.status).unwrap(),
                json!(result_status),
                "{operation}"
            );
        }
        assert_eq!(harness.connection_count(), 1, "{operation}");
        let log = harness.log();
        assert_eq!(log.len(), 1, "{operation}");
        assert_eq!(log[0]["method"], "thread/goal/set", "{operation}");
        assert!(
            log.iter()
                .all(|request| request["method"] != "thread/goal/get"),
            "{operation}"
        );
    }
}

#[tokio::test]
async fn malformed_goal_mutation_results_are_attributed_to_the_set_stage() {
    for result in [json!({}), json!({"goal": {"threadId": "target"}})] {
        let harness = FakeAppServer::start(vec![FakeStep::result(
            "thread/goal/set",
            json!({
                "threadId": "target",
                "objective": "replacement objective",
                "status": "active",
            }),
            result,
        )])
        .await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let error = set_goal(
            &client,
            &mut connection,
            ThreadGoalSetInput {
                thread_id: "target".to_owned(),
                objective: "replacement objective".to_owned(),
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.stage, "thread/goal/set");
        assert_eq!(harness.log().len(), 1);
    }
}

#[tokio::test]
async fn absent_goal_pause_and_resume_still_issue_exactly_one_native_mutation() {
    for (tool, status) in [
        ("thread_goal_pause", "paused"),
        ("thread_goal_resume", "active"),
    ] {
        let harness = FakeAppServer::start(vec![FakeStep::error(
            "thread/goal/set",
            json!({"threadId": "target", "status": status}),
            json!({"code": -32090, "message": "goal does not exist"}),
        )])
        .await;
        let client = AppServerClient::from_config(&harness.config);
        let mut connection = client.connect_initialized().await.unwrap();

        let result = if tool == "thread_goal_pause" {
            pause_goal(
                &client,
                &mut connection,
                ThreadGoalPauseInput {
                    thread_id: "target".to_owned(),
                },
            )
            .await
        } else {
            resume_goal(
                &client,
                &mut connection,
                ThreadGoalResumeInput {
                    thread_id: "target".to_owned(),
                },
            )
            .await
        };

        assert_eq!(result.unwrap_err().category, ToolErrorCategory::NativeError);
        assert_eq!(harness.log().len(), 1);
    }
}

#[tokio::test]
async fn running_turn_goal_replace_has_no_controller_preflight() {
    let harness = FakeAppServer::start(vec![FakeStep::result(
        "thread/goal/set",
        json!({
            "threadId": "target",
            "objective": "replacement objective",
            "status": "active",
        }),
        json!({"goal": native_goal("target", "active")}),
    )])
    .await;
    let client = AppServerClient::from_config(&harness.config);
    let mut connection = client.connect_initialized().await.unwrap();

    let goal = set_goal(
        &client,
        &mut connection,
        ThreadGoalSetInput {
            thread_id: "target".to_owned(),
            objective: "replacement objective".to_owned(),
        },
    )
    .await
    .unwrap();

    assert_eq!(goal.thread_id, "target");
    assert_eq!(harness.connection_count(), 1);
    assert_eq!(harness.log().len(), 1);
}
