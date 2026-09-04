use super::*;

#[tokio::test]
async fn goal_mutations_forward_once_and_report_native_results() {
    for (tool, params, result_status) in [
        (
            "thread_goal_set",
            json!({
                "threadId": "target",
                "objective": "replacement objective",
                "status": "active",
            }),
            "active",
        ),
        (
            "thread_goal_pause",
            json!({"threadId": "target", "status": "paused"}),
            "paused",
        ),
        (
            "thread_goal_resume",
            json!({"threadId": "target", "status": "active"}),
            "active",
        ),
    ] {
        for succeeds in [true, false] {
            let step = if succeeds {
                FakeStep::result(
                    "thread/goal/set",
                    params.clone(),
                    json!({"goal": native_goal("target", result_status)}),
                )
            } else {
                FakeStep::error(
                    "thread/goal/set",
                    params.clone(),
                    json!({"code": -32090, "message": "transition rejected"}),
                )
            };
            let harness = FakeAppServer::start(vec![step]).await;
            let client = harness.client();
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

            if succeeds {
                let goal = result.unwrap();
                assert_eq!(
                    serde_json::to_value(goal.status).unwrap(),
                    json!(result_status)
                );
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.category, ToolErrorCategory::NativeError);
                assert_eq!(error.tool, tool);
                assert_eq!(error.stage, "thread/goal/set");
            }
            assert_eq!(harness.connection_count(), 1);
            let log = harness.log();
            assert_eq!(log.len(), 1);
            assert_eq!(log[0]["method"], "thread/goal/set");
            assert!(
                log.iter()
                    .all(|request| request["method"] != "thread/goal/get")
            );
        }
    }
}

#[tokio::test]
async fn malformed_goal_mutation_results_are_attributed_to_the_set_stage() {
    let harness = FakeAppServer::start(vec![FakeStep::result(
        "thread/goal/set",
        json!({
            "threadId": "target",
            "objective": "replacement objective",
            "status": "active",
        }),
        json!({}),
    )])
    .await;
    let client = harness.client();
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

#[tokio::test]
async fn goal_set_rejects_response_for_a_different_thread() {
    let harness = FakeAppServer::start(vec![FakeStep::result(
        "thread/goal/set",
        json!({
            "threadId": "target",
            "objective": "replacement objective",
            "status": "active",
        }),
        json!({"goal": native_goal("different-thread", "active")}),
    )])
    .await;

    let error = execute_tool(
        "thread_goal_set",
        ValidatedInput::ThreadGoalSet(ThreadGoalSetInput {
            thread_id: "target".to_owned(),
            objective: "replacement objective".to_owned(),
        }),
        &harness.client(),
    )
    .await
    .unwrap_err();
    let error: ToolErrorData = serde_json::from_value(error.data.unwrap()).unwrap();

    assert_eq!(error.category, ToolErrorCategory::NativeError);
    assert_eq!(error.tool, "thread_goal_set");
    assert_eq!(error.stage, "thread/goal/set");
    assert_eq!(harness.log().len(), 1);
}
