use std::collections::HashSet;

use futures_util::future::join_all;
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

const THREAD_HISTORY_NOT_MATERIALIZED: &str = "Codex has not materialized this thread's history yet. No message was sent. Wait a few seconds, then retry `thread_message_send`.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReconciliationPolicy {
    GoalGet,
    LatestTurnRead,
    CompactThreadRead,
    None,
}

#[derive(Debug)]
pub(super) struct MutationContext {
    thread_id: Option<String>,
    turn_id: Option<String>,
    reconciliation: ReconciliationPolicy,
}

impl MutationContext {
    pub(super) fn unscoped() -> Self {
        Self {
            thread_id: None,
            turn_id: None,
            reconciliation: ReconciliationPolicy::None,
        }
    }

    pub(super) fn for_thread(thread_id: String, reconciliation: ReconciliationPolicy) -> Self {
        Self {
            thread_id: Some(thread_id),
            turn_id: None,
            reconciliation,
        }
    }

    pub(super) fn for_turn(
        thread_id: String,
        turn_id: String,
        reconciliation: ReconciliationPolicy,
    ) -> Self {
        Self {
            thread_id: Some(thread_id),
            turn_id: Some(turn_id),
            reconciliation,
        }
    }

    pub(super) fn known_ids(&self) -> (Option<&str>, Option<&str>) {
        (self.thread_id.as_deref(), self.turn_id.as_deref())
    }

    fn thread_id(&self) -> &str {
        self.thread_id
            .as_deref()
            .expect("thread-scoped mutation context")
    }

    fn turn_id(&self) -> &str {
        self.turn_id
            .as_deref()
            .expect("turn-scoped mutation context")
    }

    fn into_thread_id(self) -> String {
        self.thread_id.expect("thread-scoped mutation context")
    }

    fn into_turn_id(self) -> String {
        self.turn_id.expect("turn-scoped mutation context")
    }

    pub(super) fn reconciliation_request(&self) -> Option<(&'static str, Value)> {
        let thread_id = self.thread_id.as_deref()?;
        match self.reconciliation {
            ReconciliationPolicy::GoalGet => {
                Some(("thread/goal/get", json!({"threadId": thread_id})))
            }
            ReconciliationPolicy::LatestTurnRead => Some((
                "thread/turns/list",
                json!({
                    "threadId": thread_id,
                    "limit": 1,
                    "itemsView": "notLoaded",
                }),
            )),
            ReconciliationPolicy::CompactThreadRead => Some((
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )),
            ReconciliationPolicy::None => None,
        }
    }
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
    let start_context = MutationContext::unscoped();
    let start = mutation_request(
        client,
        connection,
        "thread_create",
        "thread/start",
        start_params,
        &start_context,
    )
    .await?;
    let thread_value = start
        .get("thread")
        .ok_or_else(|| malformed_result("thread_create", "thread/start"))?;
    let thread = thread_from_native(thread_value, "thread/start")?;
    let turn_context =
        MutationContext::for_thread(thread.id.clone(), ReconciliationPolicy::CompactThreadRead);

    let turn = mutation_request(
        client,
        connection,
        "thread_create",
        "turn/start",
        json!({
            "threadId": turn_context.thread_id(),
            "input": [{"type": "text", "text": input.prompt}],
        }),
        &turn_context,
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
    let context = MutationContext::for_thread(thread_id, ReconciliationPolicy::None);
    let response = mutation_request(
        client,
        connection,
        "thread_fork",
        "thread/fork",
        json!({
            "threadId": context.thread_id(),
            "deferGoalContinuation": input.defer_goal_continuation,
        }),
        &context,
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
    let snapshot = connection
        .compact_snapshot(&input.thread_id)
        .await
        .map_err(|error| message_snapshot_error(error, &input.thread_id))?;
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
            let context = MutationContext::for_turn(
                input.thread_id,
                turn_id,
                ReconciliationPolicy::CompactThreadRead,
            );
            let response = mutation_request(
                client,
                connection,
                "thread_message_send",
                "turn/steer",
                json!({
                    "threadId": context.thread_id(),
                    "expectedTurnId": context.turn_id(),
                    "input": text_input,
                }),
                &context,
            )
            .await?;
            Ok(ThreadMessageSendResult {
                action: ThreadMessageAction::Steered,
                thread_id: context.into_thread_id(),
                turn_id: native_required_string(
                    &response,
                    "turnId",
                    "thread_message_send",
                    "turn/steer",
                )?,
            })
        }
        ThreadStatus::NotLoaded | ThreadStatus::Idle | ThreadStatus::SystemError => {
            let context = MutationContext::for_thread(
                input.thread_id,
                ReconciliationPolicy::CompactThreadRead,
            );
            let mut params = Map::new();
            params.insert(
                "threadId".to_owned(),
                Value::String(context.thread_id().to_owned()),
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
                &context,
            )
            .await?;
            let turn = response
                .get("turn")
                .ok_or_else(|| malformed_result("thread_message_send", "turn/start"))
                .and_then(|turn| turn_from_native(turn, "turn/start"))?;
            Ok(ThreadMessageSendResult {
                action: ThreadMessageAction::Started,
                thread_id: context.into_thread_id(),
                turn_id: turn.id,
            })
        }
    }
}

fn message_snapshot_error(mut error: ToolErrorData, thread_id: &str) -> ToolErrorData {
    let rollout_is_empty = error.category == ToolErrorCategory::NativeError
        && matches!(error.stage.as_str(), "thread/read" | "thread/turns/list")
        && error.native.as_ref().is_some_and(|native| {
            native.code == Some(-32603)
                && native.message.contains(": rollout at ")
                && native.message.ends_with(" is empty")
        });
    if rollout_is_empty {
        error.message = THREAD_HISTORY_NOT_MATERIALIZED.to_owned();
        error.thread_id = Some(thread_id.to_owned());
    }
    error
}

pub(super) async fn set_title(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadTitleSetInput,
) -> Result<ThreadTitleSetResult, ToolErrorData> {
    let thread_id = input
        .thread_id
        .ok_or_else(|| malformed_result("thread_title_set", "validation"))?;
    let context = MutationContext::for_thread(thread_id, ReconciliationPolicy::CompactThreadRead);
    mutation_request(
        client,
        connection,
        "thread_title_set",
        "thread/name/set",
        json!({"threadId": context.thread_id(), "name": input.title}),
        &context,
    )
    .await?;
    Ok(ThreadTitleSetResult {})
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
    let context = MutationContext::for_thread(thread_id, ReconciliationPolicy::GoalGet);
    let mut params = fields
        .as_object()
        .expect("goal mutation fields are objects")
        .clone();
    params.insert(
        "threadId".to_owned(),
        Value::String(context.thread_id().to_owned()),
    );
    let response = mutation_request(
        client,
        connection,
        tool,
        "thread/goal/set",
        params,
        &context,
    )
    .await?;
    goal_from_native(&response, context.thread_id(), "thread/goal/set")?
        .ok_or_else(|| malformed_result(tool, "thread/goal/set"))
}

pub(super) async fn clear_goal(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    input: ThreadGoalClearInput,
) -> Result<ThreadGoalClearResult, ToolErrorData> {
    let context = MutationContext::for_thread(input.thread_id, ReconciliationPolicy::GoalGet);
    let response = mutation_request(
        client,
        connection,
        "thread_goal_clear",
        "thread/goal/clear",
        json!({"threadId": context.thread_id()}),
        &context,
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
    caller_thread_id: String,
) -> Result<ThreadInterruptResult, ToolErrorData> {
    let exact = interrupt_exact_thread(client, connection, &input.thread_id).await?;
    let root_thread_id = input.thread_id;
    let mut active_descendant_ids = Vec::new();
    let mut seen_active_descendant_ids = HashSet::new();
    let mut cursor = None;

    loop {
        let (threads, next_cursor) = match connection
            .spawned_descendants_page(&root_thread_id, cursor.as_deref())
            .await
        {
            Ok(page) => page,
            Err(error) => {
                return Ok(ThreadInterruptResult {
                    exact,
                    descendants: Some(ThreadInterruptDescendants::Error {
                        error: DescendantDiscoveryError {
                            code: "descendant_discovery_failed",
                            error: attribute_interrupt_error(error, &root_thread_id),
                        },
                    }),
                });
            }
        };

        for thread in threads {
            if matches!(&thread.status, ThreadStatus::Active { .. })
                && seen_active_descendant_ids.insert(thread.id.clone())
            {
                active_descendant_ids.push(thread.id);
            }
        }

        let Some(next_cursor) = next_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }

    let descendants = if input.include_descendants {
        let attempts = active_descendant_ids.into_iter().map(|thread_id| {
            let caller_thread_id = caller_thread_id.as_str();
            async move {
                if let Err(error) =
                    require_other_thread("thread_interrupt", caller_thread_id, &thread_id)
                {
                    return DescendantInterruptEntry::Error { thread_id, error };
                }

                let outcome = match client.connect_initialized().await {
                    Ok(mut target_connection) => {
                        interrupt_exact_thread(client, &mut target_connection, &thread_id).await
                    }
                    Err(error) => Err(error),
                };
                match outcome {
                    Ok(result) => DescendantInterruptEntry::Result { thread_id, result },
                    Err(error) => DescendantInterruptEntry::Error {
                        error: attribute_interrupt_error(error, &thread_id),
                        thread_id,
                    },
                }
            }
        });
        Some(ThreadInterruptDescendants::Results {
            results: join_all(attempts).await,
        })
    } else if active_descendant_ids.is_empty() {
        None
    } else {
        Some(ThreadInterruptDescendants::Warning {
            warning: ActiveDescendantsWarning {
                code: "active_descendants_not_interrupted",
                active_count: active_descendant_ids.len(),
                active_thread_ids: active_descendant_ids,
            },
        })
    };

    Ok(ThreadInterruptResult { exact, descendants })
}

fn attribute_interrupt_error(mut error: ToolErrorData, thread_id: &str) -> ToolErrorData {
    error.tool = "thread_interrupt".to_owned();
    error.thread_id = Some(thread_id.to_owned());
    error
}

async fn interrupt_exact_thread(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    thread_id: &str,
) -> Result<ExactThreadInterruptResult, ToolErrorData> {
    let snapshot = connection.compact_snapshot(thread_id).await?;
    let Some(turn_id) = snapshot.active_turn_id else {
        return Ok(ExactThreadInterruptResult::NotInterrupted { interrupted: false });
    };
    let context = MutationContext::for_turn(
        thread_id.to_owned(),
        turn_id,
        ReconciliationPolicy::LatestTurnRead,
    );
    mutation_request(
        client,
        connection,
        "thread_interrupt",
        "turn/interrupt",
        json!({"threadId": context.thread_id(), "turnId": context.turn_id()}),
        &context,
    )
    .await?;
    Ok(ExactThreadInterruptResult::Interrupted {
        interrupted: true,
        turn_id: context.into_turn_id(),
    })
}

pub(super) async fn mutation_request(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    tool: &'static str,
    method: &'static str,
    params: impl Serialize,
    context: &MutationContext,
) -> Result<Value, ToolErrorData> {
    let (thread_id, turn_id) = context.known_ids();
    match connection
        .mutate(tool, method, params, thread_id, turn_id)
        .await
    {
        Ok(result) => Ok(result),
        Err(error) if error.category == ToolErrorCategory::OutcomeUnknown => {
            Err(reconcile_outcome_unknown(client, error, context).await)
        }
        Err(error) => Err(error),
    }
}

pub(super) async fn reconcile_outcome_unknown(
    client: &AppServerClient,
    mut error: ToolErrorData,
    context: &MutationContext,
) -> ToolErrorData {
    let Some((method, params)) = context.reconciliation_request() else {
        return error;
    };
    let result = reconcile_request(client, method, params).await;
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
