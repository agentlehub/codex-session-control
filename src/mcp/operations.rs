use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{
    app_server::{
        AppServerClient, AppServerConnection, goal_from_native, thread_from_native,
        turn_from_native,
    },
    error::{ToolErrorCategory, ToolErrorData},
    model::{ThreadGoal, ThreadStatus},
};

use super::contract::*;

const PINNED_THREAD_SECTION_ID: &str = "01984de2-8f74-7c91-a3b2-5c5e937cf318";

#[derive(Debug)]
pub(super) enum Reconciliation {
    GoalGet { thread_id: String },
    LatestTurnRead { thread_id: String },
    CompactThreadRead { thread_id: String },
    None,
}

pub(super) async fn create_thread(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadCreateInput,
) -> Result<ThreadCreateResult, ToolErrorData> {
    let mut start_params = Map::new();
    start_params.insert("cwd".to_owned(), Value::String(input.cwd));
    if let Some(model) = input.model {
        start_params.insert("model".to_owned(), Value::String(model));
    }
    if let Some(reasoning_effort) = input.reasoning_effort {
        start_params.insert(
            "config".to_owned(),
            json!({"model_reasoning_effort": reasoning_effort}),
        );
    }
    let start = mutation_request(
        client,
        connection,
        "thread_create",
        "thread/start",
        start_params,
        None,
        None,
        Reconciliation::None,
    )
    .await?;
    let thread_value = start
        .get("thread")
        .ok_or_else(|| malformed_result("thread_create", "thread/start"))?;
    let thread = thread_from_native(thread_value, "thread/start")?;
    let thread_id = thread.id.clone();

    let turn = mutation_request(
        client,
        connection,
        "thread_create",
        "turn/start",
        json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": input.prompt}],
        }),
        Some(&thread.id),
        None,
        Reconciliation::CompactThreadRead {
            thread_id: thread.id.clone(),
        },
    )
    .await?;
    let turn_value = turn
        .get("turn")
        .ok_or_else(|| malformed_result("thread_create", "turn/start"))?;
    let turn = turn_from_native(turn_value, "turn/start")?;
    Ok(ThreadCreateResult {
        thread_id: thread.id,
        turn_id: turn.id,
        cwd: native_required_string(&start, "cwd", "thread_create", "thread/start")?,
        model: native_optional_string(&start, "model", "thread_create", "thread/start")?,
        reasoning_effort: native_optional_string(
            &start,
            "reasoningEffort",
            "thread_create",
            "thread/start",
        )?,
    })
}

pub(super) async fn fork_thread(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadForkInput,
) -> Result<ThreadForkResult, ToolErrorData> {
    let thread_id = input
        .thread_id
        .ok_or_else(|| malformed_result("thread_fork", "validation"))?;
    let response = mutation_request(
        client,
        connection,
        "thread_fork",
        "thread/fork",
        json!({
            "threadId": thread_id,
            "deferGoalContinuation": input.defer_goal_continuation,
        }),
        Some(&thread_id),
        None,
        Reconciliation::None,
    )
    .await?;
    let thread = response
        .get("thread")
        .ok_or_else(|| malformed_result("thread_fork", "thread/fork"))
        .and_then(|thread| thread_from_native(thread, "thread/fork"))?;
    Ok(ThreadForkResult {
        thread,
        model: native_optional_string(&response, "model", "thread_fork", "thread/fork")?,
        reasoning_effort: native_optional_string(
            &response,
            "reasoningEffort",
            "thread_fork",
            "thread/fork",
        )?,
    })
}

pub(super) async fn send_message(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadMessageSendInput,
) -> Result<ThreadMessageSendResult, ToolErrorData> {
    let snapshot = connection.compact_snapshot(&input.thread_id).await?;
    let text_input = json!([{"type": "text", "text": input.prompt}]);
    match snapshot.status {
        ThreadStatus::Active { .. } => {
            if input.model.is_some() || input.reasoning_effort.is_some() {
                let mut error = ToolErrorData::fixed(
                    ToolErrorCategory::PolicyRejected,
                    "thread_message_send",
                    "active_override",
                );
                error.thread_id = Some(input.thread_id);
                return Err(error);
            }
            let turn_id = snapshot.active_turn_id.ok_or_else(|| {
                let mut error = ToolErrorData::fixed(
                    ToolErrorCategory::NativeConflict,
                    "thread_message_send",
                    "active_turn",
                );
                error.thread_id = Some(input.thread_id.clone());
                error
            })?;
            let response = mutation_request(
                client,
                connection,
                "thread_message_send",
                "turn/steer",
                json!({
                    "threadId": input.thread_id,
                    "expectedTurnId": turn_id,
                    "input": text_input,
                }),
                Some(&input.thread_id),
                Some(&turn_id),
                Reconciliation::CompactThreadRead {
                    thread_id: input.thread_id.clone(),
                },
            )
            .await?;
            Ok(ThreadMessageSendResult {
                action: ThreadMessageAction::Steered,
                thread_id: input.thread_id,
                turn_id: native_required_string(
                    &response,
                    "turnId",
                    "thread_message_send",
                    "turn/steer",
                )?,
            })
        }
        ThreadStatus::NotLoaded | ThreadStatus::Idle | ThreadStatus::SystemError => {
            let mut params = Map::new();
            params.insert(
                "threadId".to_owned(),
                Value::String(input.thread_id.clone()),
            );
            params.insert("input".to_owned(), text_input);
            if let Some(model) = input.model {
                params.insert("model".to_owned(), Value::String(model));
            }
            if let Some(effort) = input.reasoning_effort {
                params.insert("effort".to_owned(), Value::String(effort));
            }
            let response = mutation_request(
                client,
                connection,
                "thread_message_send",
                "turn/start",
                params,
                Some(&input.thread_id),
                None,
                Reconciliation::CompactThreadRead {
                    thread_id: input.thread_id.clone(),
                },
            )
            .await?;
            let turn = response
                .get("turn")
                .ok_or_else(|| malformed_result("thread_message_send", "turn/start"))
                .and_then(|turn| turn_from_native(turn, "turn/start"))?;
            Ok(ThreadMessageSendResult {
                action: ThreadMessageAction::Started,
                thread_id: input.thread_id,
                turn_id: turn.id,
            })
        }
    }
}

pub(super) async fn set_title(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadTitleSetInput,
) -> Result<BTreeMap<String, Value>, ToolErrorData> {
    let thread_id = input
        .thread_id
        .ok_or_else(|| malformed_result("thread_title_set", "validation"))?;
    mutation_request(
        client,
        connection,
        "thread_title_set",
        "thread/name/set",
        json!({"threadId": thread_id, "name": input.title}),
        Some(&thread_id),
        None,
        Reconciliation::CompactThreadRead {
            thread_id: thread_id.clone(),
        },
    )
    .await?;
    Ok(BTreeMap::new())
}

pub(super) async fn set_pin(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadPinSetInput,
) -> Result<ThreadPinSetResult, ToolErrorData> {
    let thread_id = input
        .thread_id
        .ok_or_else(|| malformed_result("thread_pin_set", "validation"))?;
    let read: Value = connection
        .request(
            "thread/read",
            json!({"threadId": thread_id, "includeTurns": false}),
        )
        .await?;
    let currently_pinned = pin_state_from_native(&read, &thread_id)?;
    if currently_pinned == input.pinned {
        return Ok(ThreadPinSetResult {
            thread_id,
            pinned: currently_pinned,
        });
    }

    let section_id = if input.pinned {
        json!(PINNED_THREAD_SECTION_ID)
    } else {
        Value::Null
    };
    let response = mutation_request(
        client,
        connection,
        "thread_pin_set",
        "thread/section/move",
        json!({"threadId": thread_id, "sectionId": section_id}),
        Some(&thread_id),
        None,
        Reconciliation::CompactThreadRead {
            thread_id: thread_id.clone(),
        },
    )
    .await?;
    if !response.is_object() {
        return Err(malformed_result("thread_pin_set", "thread/section/move"));
    }
    Ok(ThreadPinSetResult {
        thread_id,
        pinned: input.pinned,
    })
}

fn pin_state_from_native(
    response: &Value,
    expected_thread_id: &str,
) -> Result<bool, ToolErrorData> {
    let thread = response
        .get("thread")
        .filter(|thread| thread.is_object())
        .ok_or_else(|| malformed_result("thread_pin_set", "thread/read"))?;
    let returned_thread_id = native_required_string(thread, "id", "thread_pin_set", "thread/read")?;
    if returned_thread_id != expected_thread_id {
        return Err(malformed_result("thread_pin_set", "thread/read"));
    }
    match thread.get("section") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Object(section)) => section
            .get("id")
            .and_then(Value::as_str)
            .map(|section_id| section_id == PINNED_THREAD_SECTION_ID)
            .ok_or_else(|| malformed_result("thread_pin_set", "thread/read")),
        _ => Err(malformed_result("thread_pin_set", "thread/read")),
    }
}

pub(super) async fn set_goal(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadGoalSetInput,
) -> Result<ThreadGoal, ToolErrorData> {
    goal_mutation(
        client,
        connection,
        "thread_goal_set",
        input.thread_id,
        json!({"objective": input.objective, "status": "active"}),
    )
    .await
}

pub(super) async fn pause_goal(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadGoalPauseInput,
) -> Result<ThreadGoal, ToolErrorData> {
    goal_mutation(
        client,
        connection,
        "thread_goal_pause",
        input.thread_id,
        json!({"status": "paused"}),
    )
    .await
}

pub(super) async fn resume_goal(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadGoalResumeInput,
) -> Result<ThreadGoal, ToolErrorData> {
    goal_mutation(
        client,
        connection,
        "thread_goal_resume",
        input.thread_id,
        json!({"status": "active"}),
    )
    .await
}

pub(super) async fn goal_mutation(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    tool: &'static str,
    thread_id: String,
    fields: Value,
) -> Result<ThreadGoal, ToolErrorData> {
    let mut params = fields
        .as_object()
        .expect("goal mutation fields are objects")
        .clone();
    params.insert("threadId".to_owned(), Value::String(thread_id.clone()));
    let response = mutation_request(
        client,
        connection,
        tool,
        "thread/goal/set",
        params,
        Some(&thread_id),
        None,
        Reconciliation::GoalGet {
            thread_id: thread_id.clone(),
        },
    )
    .await?;
    goal_from_native(&response)?.ok_or_else(|| malformed_result(tool, "thread/goal/set"))
}

pub(super) async fn clear_goal(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadGoalClearInput,
) -> Result<ThreadGoalClearResult, ToolErrorData> {
    let response = mutation_request(
        client,
        connection,
        "thread_goal_clear",
        "thread/goal/clear",
        json!({"threadId": input.thread_id}),
        Some(&input.thread_id),
        None,
        Reconciliation::GoalGet {
            thread_id: input.thread_id.clone(),
        },
    )
    .await?;
    let cleared = response
        .get("cleared")
        .and_then(Value::as_bool)
        .ok_or_else(|| malformed_result("thread_goal_clear", "thread/goal/clear"))?;
    Ok(ThreadGoalClearResult { cleared })
}

pub(super) async fn interrupt_thread(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadInterruptInput,
) -> Result<ThreadInterruptResult, ToolErrorData> {
    let snapshot = connection.compact_snapshot(&input.thread_id).await?;
    let Some(turn_id) = snapshot.active_turn_id else {
        return Ok(ThreadInterruptResult::NotInterrupted { interrupted: false });
    };
    mutation_request(
        client,
        connection,
        "thread_interrupt",
        "turn/interrupt",
        json!({"threadId": input.thread_id, "turnId": turn_id}),
        Some(&input.thread_id),
        Some(&turn_id),
        Reconciliation::LatestTurnRead {
            thread_id: input.thread_id.clone(),
        },
    )
    .await?;
    Ok(ThreadInterruptResult::Interrupted {
        interrupted: true,
        turn_id,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn mutation_request(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    tool: &'static str,
    method: &'static str,
    params: impl Serialize,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
    reconciliation: Reconciliation,
) -> Result<Value, ToolErrorData> {
    match connection
        .mutate(tool, method, params, thread_id, turn_id)
        .await
    {
        Ok(result) => Ok(result),
        Err(error) if error.category == ToolErrorCategory::OutcomeUnknown => {
            Err(reconcile_outcome_unknown(client, error, reconciliation).await)
        }
        Err(error) => Err(error),
    }
}

pub(super) async fn reconcile_outcome_unknown(
    client: &AppServerClient,
    mut error: ToolErrorData,
    reconciliation: Reconciliation,
) -> ToolErrorData {
    let result = match reconciliation {
        Reconciliation::None => return error,
        Reconciliation::GoalGet { thread_id } => {
            reconcile_request(client, "thread/goal/get", json!({"threadId": thread_id})).await
        }
        Reconciliation::LatestTurnRead { thread_id } => {
            reconcile_request(
                client,
                "thread/turns/list",
                json!({
                    "threadId": thread_id,
                    "limit": 1,
                    "itemsView": "notLoaded"
                }),
            )
            .await
        }
        Reconciliation::CompactThreadRead { thread_id } => {
            reconcile_request(
                client,
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )
            .await
        }
    };
    match result {
        Ok(observation) => error.observation = Some(observation),
        Err(reconciliation_error) => {
            error.reconciliation_error = Some(reconciliation_error.message)
        }
    }
    error
}

pub(super) async fn reconcile_request(
    client: &AppServerClient,
    method: &'static str,
    params: Value,
) -> Result<Value, ToolErrorData> {
    let mut connection = client.connect_initialized().await?;
    connection.request(method, params).await
}

pub(super) fn native_required_string(
    value: &Value,
    field: &str,
    tool: &'static str,
    stage: &'static str,
) -> Result<String, ToolErrorData> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| malformed_result(tool, stage))
}

pub(super) fn native_optional_string(
    value: &Value,
    field: &str,
    tool: &'static str,
    stage: &'static str,
) -> Result<Option<String>, ToolErrorData> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(malformed_result(tool, stage)),
    }
}

pub(super) fn malformed_result(tool: &str, stage: &str) -> ToolErrorData {
    ToolErrorData::fixed(ToolErrorCategory::NativeError, tool, stage)
}
