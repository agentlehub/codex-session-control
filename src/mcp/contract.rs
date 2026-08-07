use std::{collections::HashSet, path::Path, sync::Arc, time::Duration};

use rmcp::model::{Tool, ToolAnnotations};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::{
    error::{ToolErrorCategory, ToolErrorData},
    model::{Thread, ThreadGoal, ThreadSnapshot, Turn, TurnItemsView},
};

pub(super) const INSTRUCTIONS: &str = "These tools inspect and control Codex threads through the shared app-server used by connected Codex clients.";
pub(super) const DEFAULT_WAIT_TIMEOUT_MS: u64 = 3_600_000;
pub(super) const MAX_WAIT_TIMEOUT_MS: u64 = 86_400_000;

pub(super) const TOOL_EFFECTS: [(&str, bool, bool); 14] = [
    ("thread_create", false, false),
    ("thread_fork", false, false),
    ("threads_list", true, false),
    ("thread_read", true, false),
    ("threads_wait", true, false),
    ("thread_message_send", false, false),
    ("thread_title_set", false, false),
    ("thread_pin_set", false, false),
    ("thread_goal_get", true, false),
    ("thread_goal_set", false, false),
    ("thread_goal_pause", false, false),
    ("thread_goal_resume", false, false),
    ("thread_goal_clear", false, true),
    ("thread_interrupt", false, true),
];

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadCreateInput {
    pub(super) prompt: String,
    /// Absolute working directory.
    pub(super) cwd: String,
    /// Model override. Omit for the app-server default.
    pub(super) model: Option<String>,
    /// Reasoning-effort override. Omit for the app-server default.
    pub(super) reasoning_effort: Option<String>,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadCreateResult {
    pub(super) thread_id: String,
    pub(super) turn_id: String,
    /// Effective working directory.
    pub(super) cwd: String,
    /// Effective model.
    pub(super) model: Option<String>,
    /// Effective reasoning effort.
    pub(super) reasoning_effort: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadForkInput {
    /// Thread to fork. Omit to fork the current thread.
    pub(super) thread_id: Option<String>,
    #[serde(default = "default_true")]
    /// When true, the fork does not immediately continue inherited goal work.
    pub(super) defer_goal_continuation: bool,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadForkResult {
    pub(super) thread: Thread,
    /// Effective model.
    pub(super) model: Option<String>,
    /// Effective reasoning effort.
    pub(super) reasoning_effort: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadsListInput {
    pub(super) cursor: Option<String>,
    /// Maximum threads to return. Omit for the app-server default.
    pub(super) limit: Option<u32>,
    /// Archive filter: true for archived, false or omitted for non-archived.
    pub(super) archived: Option<bool>,
    /// Exact working-directory filter.
    pub(super) cwd: Option<String>,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadsListResult {
    /// Threads on this page, newest first.
    pub(super) threads: Vec<Thread>,
    /// Use as cursor for the next page.
    pub(super) next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadReadInput {
    pub(super) thread_id: String,
    pub(super) cursor: Option<String>,
    /// Maximum turns to return. Omit for the app-server default.
    pub(super) limit: Option<u32>,
    pub(super) items_view: Option<TurnItemsView>,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadReadResult {
    pub(super) thread: Thread,
    /// Turns on this page, newest first.
    pub(super) turns: Vec<Turn>,
    /// Use as cursor for the next page.
    pub(super) next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadsWaitInput {
    /// 1-8 unique thread IDs, excluding the current thread.
    pub(super) thread_ids: Vec<String>,
    /// Timeout in milliseconds: 0 checks immediately, default 3600000, maximum 86400000.
    pub(super) timeout_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ThreadsWaitReason {
    Ready,
    Error,
    Timeout,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadsWaitResult {
    pub(super) reason: ThreadsWaitReason,
    /// Thread IDs that caused a ready result. Empty for error or timeout.
    pub(super) trigger_thread_ids: Vec<String>,
    /// Latest snapshots for successfully read targets.
    pub(super) threads: Vec<ThreadSnapshot>,
    /// Per-target read failures.
    pub(super) errors: Vec<ToolErrorData>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadMessageSendInput {
    pub(super) thread_id: String,
    pub(super) prompt: String,
    /// Model override.
    pub(super) model: Option<String>,
    /// Reasoning-effort override.
    pub(super) reasoning_effort: Option<String>,
}

#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ThreadMessageAction {
    Started,
    Steered,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadMessageSendResult {
    pub(super) action: ThreadMessageAction,
    pub(super) thread_id: String,
    pub(super) turn_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadTitleSetInput {
    /// Omit to rename the current thread.
    pub(super) thread_id: Option<String>,
    pub(super) title: String,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadTitleSetResult {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadPinSetInput {
    /// Omit to target the current thread.
    pub(super) thread_id: Option<String>,
    pub(super) pinned: bool,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadPinSetResult {
    pub(super) thread_id: String,
    pub(super) pinned: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalGetInput {
    pub(super) thread_id: String,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalGetResult {
    pub(super) goal: Option<ThreadGoal>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalSetInput {
    pub(super) thread_id: String,
    pub(super) objective: String,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalSetResult {
    pub(super) goal: ThreadGoal,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalPauseInput {
    pub(super) thread_id: String,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalPauseResult {
    pub(super) goal: ThreadGoal,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalResumeInput {
    pub(super) thread_id: String,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalResumeResult {
    pub(super) goal: ThreadGoal,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalClearInput {
    pub(super) thread_id: String,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadGoalClearResult {
    pub(super) cleared: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadInterruptInput {
    pub(super) thread_id: String,
}

#[derive(Debug, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub(super) enum ThreadInterruptResult {
    Interrupted { interrupted: bool, turn_id: String },
    NotInterrupted { interrupted: bool },
}

#[allow(dead_code)]
#[derive(Debug)]
pub(super) enum ValidatedInput {
    ThreadCreate(ThreadCreateInput),
    ThreadFork(ThreadForkInput),
    ThreadsList(ThreadsListInput),
    ThreadRead(ThreadReadInput),
    ThreadsWait {
        input: ThreadsWaitInput,
        timeout: Duration,
    },
    ThreadMessageSend(ThreadMessageSendInput),
    ThreadTitleSet(ThreadTitleSetInput),
    ThreadPinSet(ThreadPinSetInput),
    ThreadGoalGet(ThreadGoalGetInput),
    ThreadGoalSet(ThreadGoalSetInput),
    ThreadGoalPause(ThreadGoalPauseInput),
    ThreadGoalResume(ThreadGoalResumeInput),
    ThreadGoalClear(ThreadGoalClearInput),
    ThreadInterrupt(ThreadInterruptInput),
}

pub(super) fn catalog() -> Vec<Tool> {
    vec![
        catalog_tool::<ThreadCreateInput, ThreadCreateResult>(
            "thread_create",
            "Create a thread and start its initial turn.",
        ),
        catalog_tool::<ThreadForkInput, ThreadForkResult>("thread_fork", "Fork a thread."),
        catalog_tool::<ThreadsListInput, ThreadsListResult>("threads_list", "List threads."),
        catalog_tool_with_input_descriptions::<ThreadReadInput, ThreadReadResult>(
            "thread_read",
            "Read thread metadata and a page of turns.",
            &[(
                "itemsView",
                "Turn items returned: notLoaded - none, summary - first user message and final agent message, full - all persisted items. Omit for summary.",
            )],
        ),
        catalog_tool::<ThreadsWaitInput, ThreadsWaitResult>(
            "threads_wait",
            "Wait until a target is ready, a target read fails, or the timeout expires. A target is ready when idle, not loaded, awaiting approval or input, in system error, or its turn ends.",
        ),
        catalog_tool::<ThreadMessageSendInput, ThreadMessageSendResult>(
            "thread_message_send",
            "Send a message to another thread, starting a turn if idle or steering its active turn. Overrides are rejected when steering.",
        ),
        catalog_tool::<ThreadTitleSetInput, ThreadTitleSetResult>(
            "thread_title_set",
            "Set a thread title.",
        ),
        catalog_tool::<ThreadPinSetInput, ThreadPinSetResult>(
            "thread_pin_set",
            "Set whether a thread is pinned.",
        ),
        catalog_tool::<ThreadGoalGetInput, ThreadGoalGetResult>(
            "thread_goal_get",
            "Read another thread's goal.",
        ),
        catalog_tool::<ThreadGoalSetInput, ThreadGoalSetResult>(
            "thread_goal_set",
            "Set or replace another thread's goal and make it active. A running turn continues unchanged.",
        ),
        catalog_tool::<ThreadGoalPauseInput, ThreadGoalPauseResult>(
            "thread_goal_pause",
            "Pause another thread's goal without interrupting its active turn.",
        ),
        catalog_tool::<ThreadGoalResumeInput, ThreadGoalResumeResult>(
            "thread_goal_resume",
            "Resume another thread's goal.",
        ),
        catalog_tool::<ThreadGoalClearInput, ThreadGoalClearResult>(
            "thread_goal_clear",
            "Clear another thread's goal without interrupting its active turn.",
        ),
        catalog_tool_with_schema::<ThreadInterruptInput>(
            "thread_interrupt",
            "Interrupt another thread's active turn. An active goal may start another turn.",
            interrupt_output_schema(),
        ),
    ]
}

pub(super) fn catalog_tool<I: JsonSchema, O: JsonSchema>(
    name: &'static str,
    description: &'static str,
) -> Tool {
    catalog_tool_with_schema::<I>(name, description, output_schema::<O>())
}

pub(super) fn catalog_tool_with_input_descriptions<I: JsonSchema, O: JsonSchema>(
    name: &'static str,
    description: &'static str,
    descriptions: &[(&str, &str)],
) -> Tool {
    catalog_tool_with_schemas(
        name,
        description,
        describe_properties(input_schema::<I>(), descriptions),
        output_schema::<O>(),
    )
}

pub(super) fn catalog_tool_with_schema<I: JsonSchema>(
    name: &'static str,
    description: &'static str,
    output_schema: Arc<Map<String, Value>>,
) -> Tool {
    catalog_tool_with_schemas(name, description, input_schema::<I>(), output_schema)
}

fn catalog_tool_with_schemas(
    name: &'static str,
    description: &'static str,
    input_schema: Arc<Map<String, Value>>,
    output_schema: Arc<Map<String, Value>>,
) -> Tool {
    let (_, read_only, destructive) = TOOL_EFFECTS
        .iter()
        .find(|(candidate, _, _)| *candidate == name)
        .expect("catalog effects are exhaustive");
    Tool::new(name, description, input_schema)
        .with_raw_output_schema(output_schema)
        .with_annotations(annotations(*read_only, *destructive))
}

fn describe_properties(
    mut schema: Arc<Map<String, Value>>,
    descriptions: &[(&str, &str)],
) -> Arc<Map<String, Value>> {
    let properties = Arc::make_mut(&mut schema)
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("input schemas are objects with properties");
    for (name, description) in descriptions {
        properties
            .get_mut(*name)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("missing schema property {name}"))
            .insert(
                "description".to_owned(),
                Value::String((*description).to_owned()),
            );
    }
    schema
}

pub(super) fn raw_schema<T: JsonSchema>() -> Map<String, Value> {
    let schema = serde_json::to_value(schema_for!(T)).expect("schema serialization is infallible");
    let mut schema = schema
        .as_object()
        .expect("schemars root schemas are objects")
        .clone();
    if schema.get("type") == Some(&Value::String("object".to_owned())) {
        schema
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    schema
}

pub(super) fn input_schema<T: JsonSchema>() -> Arc<Map<String, Value>> {
    let mut schema = raw_schema::<T>();
    let required: HashSet<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for (name, property_schema) in properties {
            if !required.contains(name) {
                remove_null_schema(property_schema);
            }
        }
    }
    Arc::new(schema)
}

pub(super) fn output_schema<T: JsonSchema>() -> Arc<Map<String, Value>> {
    let mut schema = raw_schema::<T>();
    require_all_properties(&mut schema);
    if let Some(definitions) = schema.get_mut("$defs").and_then(Value::as_object_mut) {
        for name in ["Thread", "Turn", "ThreadGoal", "ThreadSnapshot"] {
            if let Some(definition) = definitions.get_mut(name).and_then(Value::as_object_mut) {
                require_all_properties(definition);
            }
        }
    }
    Arc::new(schema)
}

pub(super) fn require_all_properties(schema: &mut Map<String, Value>) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    schema.insert(
        "required".to_owned(),
        Value::Array(properties.keys().cloned().map(Value::String).collect()),
    );
}

pub(super) fn remove_null_schema(schema: &mut Value) {
    let replacement = {
        let Some(object) = schema.as_object_mut() else {
            return;
        };
        if object.get("default") == Some(&Value::Null) {
            object.remove("default");
        }
        let single_type = object
            .get_mut("type")
            .and_then(Value::as_array_mut)
            .and_then(|types| {
                types.retain(|value| value != "null");
                (types.len() == 1).then(|| types[0].clone())
            });
        if let Some(single_type) = single_type {
            object.insert("type".to_owned(), single_type);
        }
        object
            .get_mut("anyOf")
            .and_then(Value::as_array_mut)
            .and_then(|variants| {
                variants.retain(|variant| {
                    variant.get("type") != Some(&Value::String("null".to_owned()))
                });
                (variants.len() == 1).then(|| variants[0].clone())
            })
    };
    if let Some(replacement) = replacement {
        *schema = replacement;
    }
}

pub(super) fn interrupt_output_schema() -> Arc<Map<String, Value>> {
    let schema = serde_json::json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "interrupted": {"const": true, "type": "boolean"},
                    "turnId": {"type": "string"}
                },
                "required": ["interrupted", "turnId"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "interrupted": {"const": false, "type": "boolean"}
                },
                "required": ["interrupted"],
                "additionalProperties": false
            }
        ]
    });
    Arc::new(schema.as_object().unwrap().clone())
}

pub(super) fn annotations(read_only: bool, destructive: bool) -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(read_only)
        .destructive(destructive)
        .open_world(false)
}

pub(super) fn validate_input(
    tool: &str,
    arguments: Map<String, Value>,
    meta: &Value,
) -> Result<ValidatedInput, ToolErrorData> {
    let result = (|| {
        if arguments.values().any(Value::is_null) {
            return Err(invalid_request(tool, "input"));
        }
        match tool {
            "thread_create" => {
                let input: ThreadCreateInput = parse_input(tool, arguments)?;
                require_absolute_cwd(&input.cwd)?;
                require_reasoning_effort("thread_create", input.reasoning_effort.as_deref())?;
                Ok(ValidatedInput::ThreadCreate(input))
            }
            "thread_fork" => {
                let mut input: ThreadForkInput = parse_input(tool, arguments)?;
                match &input.thread_id {
                    Some(thread_id) => require_id("threadId", thread_id)?,
                    None => input.thread_id = Some(caller_thread_id(meta)?),
                }
                Ok(ValidatedInput::ThreadFork(input))
            }
            "threads_list" => {
                let input: ThreadsListInput = parse_input(tool, arguments)?;
                if let Some(cursor) = input.cursor.as_deref() {
                    require_id("cursor", cursor)?;
                }
                Ok(ValidatedInput::ThreadsList(input))
            }
            "thread_read" => {
                let input: ThreadReadInput = parse_input(tool, arguments)?;
                require_id("threadId", &input.thread_id)?;
                if let Some(cursor) = input.cursor.as_deref() {
                    require_id("cursor", cursor)?;
                }
                Ok(ValidatedInput::ThreadRead(input))
            }
            "threads_wait" => {
                let input: ThreadsWaitInput = parse_input(tool, arguments)?;
                let caller = caller_thread_id(meta)?;
                if !(1..=8).contains(&input.thread_ids.len()) {
                    return Err(invalid_request(tool, "threadIds"));
                }
                let mut unique = HashSet::with_capacity(input.thread_ids.len());
                for thread_id in &input.thread_ids {
                    require_id("threadIds", thread_id)?;
                    require_other_thread("threads_wait", &caller, thread_id)?;
                    if !unique.insert(thread_id) {
                        return Err(invalid_request(tool, "threadIds"));
                    }
                }
                let timeout = wait_timeout(input.timeout_ms)?;
                Ok(ValidatedInput::ThreadsWait { input, timeout })
            }
            "thread_message_send" => {
                let input: ThreadMessageSendInput = parse_input(tool, arguments)?;
                validate_explicit_other("thread_message_send", meta, &input.thread_id)?;
                require_reasoning_effort("thread_message_send", input.reasoning_effort.as_deref())?;
                Ok(ValidatedInput::ThreadMessageSend(input))
            }
            "thread_title_set" => {
                let mut input: ThreadTitleSetInput = parse_input(tool, arguments)?;
                match &input.thread_id {
                    Some(thread_id) => require_id("threadId", thread_id)?,
                    None => input.thread_id = Some(caller_thread_id(meta)?),
                }
                Ok(ValidatedInput::ThreadTitleSet(input))
            }
            "thread_pin_set" => {
                let mut input: ThreadPinSetInput = parse_input(tool, arguments)?;
                match &input.thread_id {
                    Some(thread_id) => require_id("threadId", thread_id)?,
                    None => input.thread_id = Some(caller_thread_id(meta)?),
                }
                Ok(ValidatedInput::ThreadPinSet(input))
            }
            "thread_goal_get" => {
                let input: ThreadGoalGetInput = parse_input(tool, arguments)?;
                validate_explicit_other("thread_goal_get", meta, &input.thread_id)?;
                Ok(ValidatedInput::ThreadGoalGet(input))
            }
            "thread_goal_set" => {
                let input: ThreadGoalSetInput = parse_input(tool, arguments)?;
                validate_explicit_other("thread_goal_set", meta, &input.thread_id)?;
                Ok(ValidatedInput::ThreadGoalSet(input))
            }
            "thread_goal_pause" => {
                let input: ThreadGoalPauseInput = parse_input(tool, arguments)?;
                validate_explicit_other("thread_goal_pause", meta, &input.thread_id)?;
                Ok(ValidatedInput::ThreadGoalPause(input))
            }
            "thread_goal_resume" => {
                let input: ThreadGoalResumeInput = parse_input(tool, arguments)?;
                validate_explicit_other("thread_goal_resume", meta, &input.thread_id)?;
                Ok(ValidatedInput::ThreadGoalResume(input))
            }
            "thread_goal_clear" => {
                let input: ThreadGoalClearInput = parse_input(tool, arguments)?;
                validate_explicit_other("thread_goal_clear", meta, &input.thread_id)?;
                Ok(ValidatedInput::ThreadGoalClear(input))
            }
            "thread_interrupt" => {
                let input: ThreadInterruptInput = parse_input(tool, arguments)?;
                validate_explicit_other("thread_interrupt", meta, &input.thread_id)?;
                Ok(ValidatedInput::ThreadInterrupt(input))
            }
            _ => Err(invalid_request(tool, "tool")),
        }
    })();
    result.map_err(|mut error| {
        error.tool = tool.to_owned();
        error
    })
}

pub(super) fn parse_input<T: DeserializeOwned>(
    tool: &str,
    arguments: Map<String, Value>,
) -> Result<T, ToolErrorData> {
    serde_json::from_value(Value::Object(arguments)).map_err(|_| invalid_request(tool, "input"))
}

pub(super) fn require_id(field: &'static str, value: &str) -> Result<(), ToolErrorData> {
    if !value.is_empty() && !value.chars().any(char::is_whitespace) {
        Ok(())
    } else {
        Err(invalid_request("validation", field))
    }
}

pub(super) fn caller_thread_id(meta: &Value) -> Result<String, ToolErrorData> {
    let caller = meta
        .as_object()
        .and_then(|meta| meta.get("threadId"))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_request("caller", "_meta.threadId"))?;
    require_id("_meta.threadId", caller)?;
    Ok(caller.to_owned())
}

pub(super) fn require_other_thread(
    tool: &'static str,
    caller: &str,
    target: &str,
) -> Result<(), ToolErrorData> {
    if caller == target {
        Err(ToolErrorData::fixed(
            ToolErrorCategory::PolicyRejected,
            tool,
            "self_target",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn require_absolute_cwd(cwd: &str) -> Result<(), ToolErrorData> {
    if Path::new(cwd).is_absolute() {
        Ok(())
    } else {
        Err(invalid_request("thread_create", "cwd"))
    }
}

pub(super) fn wait_timeout(value: Option<u64>) -> Result<Duration, ToolErrorData> {
    let timeout_ms = value.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    if timeout_ms <= MAX_WAIT_TIMEOUT_MS {
        Ok(Duration::from_millis(timeout_ms))
    } else {
        Err(invalid_request("threads_wait", "timeoutMs"))
    }
}

pub(super) fn validate_explicit_other(
    tool: &'static str,
    meta: &Value,
    target: &str,
) -> Result<(), ToolErrorData> {
    require_id("threadId", target)?;
    let caller = caller_thread_id(meta)?;
    require_other_thread(tool, &caller, target)
}

pub(super) fn require_reasoning_effort(
    tool: &'static str,
    reasoning_effort: Option<&str>,
) -> Result<(), ToolErrorData> {
    if reasoning_effort == Some("") {
        Err(invalid_request(tool, "reasoningEffort"))
    } else {
        Ok(())
    }
}

pub(super) fn invalid_request(tool: &str, stage: &str) -> ToolErrorData {
    ToolErrorData::fixed(ToolErrorCategory::InvalidRequest, tool, stage)
}

pub(super) const fn default_true() -> bool {
    true
}
