use std::{collections::BTreeMap, sync::LazyLock};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    error::{ToolErrorCategory, ToolErrorData},
    model::{Thread, ThreadGoal, ThreadGoalStatus, ThreadSnapshot, ThreadStatus, Turn, TurnStatus},
};

pub(super) fn compact_snapshot_from_native(
    thread_id: &str,
    metadata: &Value,
    latest: &Value,
) -> Result<ThreadSnapshot, ToolErrorData> {
    let thread = metadata
        .get("thread")
        .ok_or_else(|| malformed_native("thread/read"))?;
    let returned_thread_id = required_string(thread, "id", "thread/read")?;
    if returned_thread_id != thread_id {
        return Err(malformed_native("thread/read"));
    }
    let status = required_value::<ThreadStatus>(thread, "status", "thread/read")?;
    let latest_turns = latest
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed_native("thread/turns/list"))?;
    let (active_turn_id, active_turn_status) = match latest_turns.first() {
        Some(turn) => {
            let turn_status = required_value::<TurnStatus>(turn, "status", "thread/turns/list")?;
            if turn_status == TurnStatus::InProgress {
                (
                    Some(required_string(turn, "id", "thread/turns/list")?),
                    Some(turn_status),
                )
            } else {
                (None, None)
            }
        }
        None => (None, None),
    };
    Ok(ThreadSnapshot {
        thread_id: returned_thread_id,
        name: optional_string(thread, "name", "thread/read")?,
        status,
        active_turn_id,
        active_turn_status,
        updated_at: required_i64(thread, "updatedAt", "thread/read")?,
    })
}

pub(super) fn thread_list_from_native(
    response: &Value,
) -> Result<(Vec<Thread>, Option<String>), ToolErrorData> {
    let data = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed_native("thread/list"))?;
    let threads = data
        .iter()
        .map(|thread| thread_from_native(thread, "thread/list"))
        .collect::<Result<_, _>>()?;
    let next_cursor = optional_string(response, "nextCursor", "thread/list")?;
    Ok((threads, next_cursor))
}

pub(super) fn thread_read_from_native(
    metadata: &Value,
    turns_response: &Value,
) -> Result<(Thread, Vec<Turn>, Option<String>), ToolErrorData> {
    let thread = metadata
        .get("thread")
        .ok_or_else(|| malformed_native("thread/read"))
        .and_then(|thread| thread_from_native(thread, "thread/read"))?;
    let data = turns_response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed_native("thread/turns/list"))?;
    let turns = data
        .iter()
        .map(|turn| turn_from_native(turn, "thread/turns/list"))
        .collect::<Result<_, _>>()?;
    let next_cursor = optional_string(turns_response, "nextCursor", "thread/turns/list")?;
    Ok((thread, turns, next_cursor))
}

pub(crate) fn thread_from_native(
    value: &Value,
    stage: &'static str,
) -> Result<Thread, ToolErrorData> {
    Ok(Thread {
        id: required_string(value, "id", stage)?,
        name: optional_string(value, "name", stage)?,
        preview: required_string(value, "preview", stage)?,
        cwd: required_string(value, "cwd", stage)?,
        status: required_value(value, "status", stage)?,
        created_at: required_i64(value, "createdAt", stage)?,
        updated_at: required_i64(value, "updatedAt", stage)?,
        forked_from_id: optional_string(value, "forkedFromId", stage)?,
    })
}

pub(crate) fn turn_from_native(value: &Value, stage: &'static str) -> Result<Turn, ToolErrorData> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed_native(stage))?;
    if items.iter().any(|item| !item.is_object()) {
        return Err(malformed_native(stage));
    }
    let error = match value.get("error") {
        None | Some(Value::Null) => None,
        Some(error @ Value::Object(_)) => Some(error.clone()),
        Some(_) => return Err(malformed_native(stage)),
    };
    Ok(Turn {
        id: required_string(value, "id", stage)?,
        status: required_value(value, "status", stage)?,
        items: items.clone(),
        items_view: required_value(value, "itemsView", stage)?,
        started_at: optional_i64(value, "startedAt", stage)?,
        completed_at: optional_i64(value, "completedAt", stage)?,
        duration_ms: optional_i64(value, "durationMs", stage)?,
        error,
    })
}

pub(crate) fn goal_from_native(
    response: &Value,
    expected_thread_id: &str,
    stage: &'static str,
) -> Result<Option<ThreadGoal>, ToolErrorData> {
    let Some(goal) = response.get("goal") else {
        return Err(malformed_native(stage));
    };
    if goal.is_null() {
        return Ok(None);
    }
    let thread_id = required_string(goal, "threadId", stage)?;
    if thread_id != expected_thread_id {
        return Err(malformed_native(stage));
    }
    Ok(Some(ThreadGoal {
        thread_id,
        objective: required_string(goal, "objective", stage)?,
        status: required_value::<ThreadGoalStatus>(goal, "status", stage)?,
        token_budget: optional_u64(goal, "tokenBudget", stage)?,
        tokens_used: required_u64(goal, "tokensUsed", stage)?,
        time_used_seconds: required_u64(goal, "timeUsedSeconds", stage)?,
        created_at: required_i64(goal, "createdAt", stage)?,
        updated_at: required_i64(goal, "updatedAt", stage)?,
    }))
}

fn required_value<T: DeserializeOwned>(
    value: &Value,
    field: &str,
    stage: &'static str,
) -> Result<T, ToolErrorData> {
    value
        .get(field)
        .cloned()
        .ok_or_else(|| malformed_native(stage))
        .and_then(|value| serde_json::from_value(value).map_err(|_| malformed_native(stage)))
}

fn required_string(
    value: &Value,
    field: &str,
    stage: &'static str,
) -> Result<String, ToolErrorData> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| malformed_native(stage))
}

fn optional_string(
    value: &Value,
    field: &str,
    stage: &'static str,
) -> Result<Option<String>, ToolErrorData> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(malformed_native(stage)),
    }
}

fn required_i64(value: &Value, field: &str, stage: &'static str) -> Result<i64, ToolErrorData> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| malformed_native(stage))
}

fn optional_i64(
    value: &Value,
    field: &str,
    stage: &'static str,
) -> Result<Option<i64>, ToolErrorData> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| malformed_native(stage)),
    }
}

fn required_u64(value: &Value, field: &str, stage: &'static str) -> Result<u64, ToolErrorData> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed_native(stage))
}

fn optional_u64(
    value: &Value,
    field: &str,
    stage: &'static str,
) -> Result<Option<u64>, ToolErrorData> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| malformed_native(stage)),
    }
}

fn malformed_native(stage: &'static str) -> ToolErrorData {
    ToolErrorData::fixed(ToolErrorCategory::NativeError, stage, stage)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProtocolFixture {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "embedded contract field is read by tests")
    )]
    pub(super) codex_version: String,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "embedded contract field is read by tests")
    )]
    pub(super) schema_sha256: String,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "embedded contract field is read by tests")
    )]
    pub(super) successful_exemplars: BTreeMap<String, Value>,
    pub(super) error_exemplars: BTreeMap<String, Value>,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "embedded contract field is read by tests")
    )]
    pub(super) turns_newest_first: bool,
}

pub(super) fn protocol_fixture() -> &'static ProtocolFixture {
    static FIXTURE: LazyLock<ProtocolFixture> = LazyLock::new(|| {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/app-server-contract.json"
        ))
        .expect("checked-in app-server protocol fixture must deserialize")
    });
    &FIXTURE
}

pub(super) fn classify_native_error(
    method: &str,
    code: i64,
    _message: &str,
    data: Option<&Value>,
    fixture: &ProtocolFixture,
) -> ToolErrorCategory {
    for (name, category, exemplar_method) in [
        (
            "threadNotFound",
            ToolErrorCategory::TargetUnavailable,
            "thread/read",
        ),
        (
            "activeTurnMismatch",
            ToolErrorCategory::NativeConflict,
            "turn/steer",
        ),
    ] {
        let Some(exemplar) = fixture.error_exemplars.get(name) else {
            continue;
        };
        if method == exemplar_method
            && exemplar.get("code").and_then(Value::as_i64) == Some(code)
            && exemplar.get("data") == data
        {
            return category;
        }
    }
    ToolErrorCategory::NativeError
}
