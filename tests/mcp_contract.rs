use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    process::{Command, Stdio},
};

use assert_cmd::cargo::cargo_bin;
use serde_json::{Value, json};

const INSTRUCTIONS: &str = "These tools inspect and control Codex threads through the shared app-server used by connected Codex clients.";

const TOOL_EFFECTS: [(&str, bool, bool); 14] = [
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

#[test]
fn public_catalog_is_exact() {
    let messages = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "contract-test", "version": "1.0.0"}
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    ];

    let mut child = Command::new(cargo_bin("codex-session-control"))
        .arg("mcp-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let child_pid = child.id();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for message in messages {
            writeln!(stdin, "{message}").unwrap();
        }
    }
    let children =
        std::fs::read_to_string(format!("/proc/{child_pid}/task/{child_pid}/children")).unwrap();
    assert!(
        children.trim().is_empty(),
        "mcp-server spawned a child: {children}"
    );
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !std::path::Path::new(&format!("/proc/{child_pid}")).exists(),
        "mcp-server remained after stdin EOF"
    );

    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(!responses.is_empty());
    assert!(
        responses
            .iter()
            .all(|response| response["jsonrpc"] == "2.0" && response.is_object())
    );
    for line in String::from_utf8(output.stderr).unwrap().lines() {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            assert!(
                value.get("result").is_none() && value.get("error").is_none(),
                "stderr contained MCP result framing: {line}"
            );
        }
    }
    let initialize = response(&responses, 1);
    assert_eq!(initialize["result"]["instructions"], INSTRUCTIONS);

    let list = response(&responses, 2);
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), TOOL_EFFECTS.len());
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        TOOL_EFFECTS
            .iter()
            .map(|(name, _, _)| *name)
            .collect::<Vec<_>>()
    );

    let expected = schema_contracts();
    let expected_descriptions = description_contracts();
    for (tool, (name, read_only, destructive)) in tools.iter().zip(TOOL_EFFECTS) {
        assert_eq!(tool["name"], name);
        assert_eq!(tool["annotations"]["readOnlyHint"], read_only);
        assert_eq!(tool["annotations"]["destructiveHint"], destructive);
        assert_eq!(tool["annotations"]["openWorldHint"], false);
        assert!(
            tool["inputSchema"]["properties"]
                .as_object()
                .is_some_and(|properties| !properties.contains_key("callerThreadId"))
        );

        let contract = expected.get(name).unwrap();
        assert_eq!(
            json!({
                "tool": tool["description"],
                "input": property_descriptions(&tool["inputSchema"]),
                "output": property_descriptions(&tool["outputSchema"]),
            }),
            expected_descriptions[name],
            "model-facing text drifted for {name}"
        );
        assert_object_schema(
            &tool["inputSchema"],
            contract.input_properties,
            contract.input_required,
        );
        if name == "thread_interrupt" {
            assert_interrupt_output_schema(&tool["outputSchema"]);
        } else {
            assert_object_schema(
                &tool["outputSchema"],
                contract.output_properties,
                contract.output_required,
            );
            for property in contract.nullable_output {
                assert!(
                    permits_null(&tool["outputSchema"]["properties"][property]),
                    "{name}.{property} must permit null"
                );
            }
        }
    }
    assert_stable_output_definitions(tools);
    assert_nested_output_definitions_have_no_descriptions(tools);
}

fn description_contracts() -> BTreeMap<&'static str, Value> {
    [
        (
            "thread_create",
            json!({
                "tool": "Create a thread and start its initial turn.",
                "input": {
                    "cwd": "Absolute working directory.",
                    "model": "Model override. Omit for the app-server default.",
                    "reasoningEffort": "Reasoning-effort override. Omit for the app-server default."
                },
                "output": {
                    "cwd": "Effective working directory.",
                    "model": "Effective model.",
                    "reasoningEffort": "Effective reasoning effort."
                }
            }),
        ),
        (
            "thread_fork",
            json!({
                "tool": "Fork a thread.",
                "input": {
                    "deferGoalContinuation": "When true, the fork does not immediately continue inherited goal work.",
                    "threadId": "Thread to fork. Omit to fork the current thread."
                },
                "output": {
                    "model": "Effective model.",
                    "reasoningEffort": "Effective reasoning effort."
                }
            }),
        ),
        (
            "threads_list",
            json!({
                "tool": "List threads.",
                "input": {
                    "archived": "Archive filter: true for archived, false or omitted for non-archived.",
                    "cwd": "Exact working-directory filter.",
                    "limit": "Maximum threads to return. Omit for the app-server default."
                },
                "output": {
                    "nextCursor": "Use as cursor for the next page.",
                    "threads": "Threads on this page, newest first."
                }
            }),
        ),
        (
            "thread_read",
            json!({
                "tool": "Read thread metadata and a page of turns.",
                "input": {
                    "itemsView": "Turn items returned: notLoaded - none, summary - first user message and final agent message, full - all persisted items. Omit for summary.",
                    "limit": "Maximum turns to return. Omit for the app-server default."
                },
                "output": {
                    "nextCursor": "Use as cursor for the next page.",
                    "turns": "Turns on this page, newest first."
                }
            }),
        ),
        (
            "threads_wait",
            json!({
                "tool": "Wait until a target is ready, a target read fails, or the timeout expires. A target is ready when idle, not loaded, awaiting approval or input, in system error, or its turn ends.",
                "input": {
                    "threadIds": "1-8 unique thread IDs, excluding the current thread.",
                    "timeoutMs": "Timeout in milliseconds: 0 checks immediately, default 3600000, maximum 86400000."
                },
                "output": {
                    "errors": "Per-target read failures.",
                    "threads": "Latest snapshots for successfully read targets.",
                    "triggerThreadIds": "Thread IDs that caused a ready result. Empty for error or timeout."
                }
            }),
        ),
        (
            "thread_message_send",
            json!({
                "tool": "Send a message to another thread, starting a turn if idle or steering its active turn. Overrides are rejected when steering.",
                "input": {
                    "model": "Model override.",
                    "reasoningEffort": "Reasoning-effort override."
                },
                "output": {}
            }),
        ),
        (
            "thread_title_set",
            json!({
                "tool": "Set a thread title.",
                "input": {"threadId": "Omit to rename the current thread."},
                "output": {}
            }),
        ),
        (
            "thread_pin_set",
            json!({
                "tool": "Set whether a thread is pinned.",
                "input": {"threadId": "Omit to target the current thread."},
                "output": {}
            }),
        ),
        (
            "thread_goal_get",
            json!({
                "tool": "Read another thread's goal.",
                "input": {},
                "output": {}
            }),
        ),
        (
            "thread_goal_set",
            json!({
                "tool": "Set or replace another thread's goal and make it active. A running turn continues unchanged.",
                "input": {},
                "output": {}
            }),
        ),
        (
            "thread_goal_pause",
            json!({
                "tool": "Pause another thread's goal without interrupting its active turn.",
                "input": {},
                "output": {}
            }),
        ),
        (
            "thread_goal_resume",
            json!({
                "tool": "Resume another thread's goal.",
                "input": {},
                "output": {}
            }),
        ),
        (
            "thread_goal_clear",
            json!({
                "tool": "Clear another thread's goal without interrupting its active turn.",
                "input": {},
                "output": {}
            }),
        ),
        (
            "thread_interrupt",
            json!({
                "tool": "Interrupt another thread's active turn. An active goal may start another turn.",
                "input": {},
                "output": {}
            }),
        ),
    ]
    .into_iter()
    .collect()
}

fn response(responses: &[Value], id: u64) -> &Value {
    responses
        .iter()
        .find(|response| response["id"] == id)
        .unwrap_or_else(|| panic!("missing response {id}: {responses:?}"))
}

#[derive(Clone, Copy)]
struct SchemaContract {
    input_properties: &'static [&'static str],
    input_required: &'static [&'static str],
    output_properties: &'static [&'static str],
    output_required: &'static [&'static str],
    nullable_output: &'static [&'static str],
}

fn schema_contracts() -> BTreeMap<&'static str, SchemaContract> {
    [
        (
            "thread_create",
            SchemaContract {
                input_properties: &["cwd", "model", "prompt", "reasoningEffort"],
                input_required: &["cwd", "prompt"],
                output_properties: &["cwd", "model", "reasoningEffort", "threadId", "turnId"],
                output_required: &["cwd", "model", "reasoningEffort", "threadId", "turnId"],
                nullable_output: &["model", "reasoningEffort"],
            },
        ),
        (
            "thread_fork",
            SchemaContract {
                input_properties: &["deferGoalContinuation", "threadId"],
                input_required: &[],
                output_properties: &["model", "reasoningEffort", "thread"],
                output_required: &["model", "reasoningEffort", "thread"],
                nullable_output: &["model", "reasoningEffort"],
            },
        ),
        (
            "threads_list",
            SchemaContract {
                input_properties: &["archived", "cursor", "cwd", "limit"],
                input_required: &[],
                output_properties: &["nextCursor", "threads"],
                output_required: &["nextCursor", "threads"],
                nullable_output: &["nextCursor"],
            },
        ),
        (
            "thread_read",
            SchemaContract {
                input_properties: &["cursor", "itemsView", "limit", "threadId"],
                input_required: &["threadId"],
                output_properties: &["nextCursor", "thread", "turns"],
                output_required: &["nextCursor", "thread", "turns"],
                nullable_output: &["nextCursor"],
            },
        ),
        (
            "threads_wait",
            SchemaContract {
                input_properties: &["threadIds", "timeoutMs"],
                input_required: &["threadIds"],
                output_properties: &["errors", "reason", "threads", "triggerThreadIds"],
                output_required: &["errors", "reason", "threads", "triggerThreadIds"],
                nullable_output: &[],
            },
        ),
        (
            "thread_message_send",
            SchemaContract {
                input_properties: &["model", "prompt", "reasoningEffort", "threadId"],
                input_required: &["prompt", "threadId"],
                output_properties: &["action", "threadId", "turnId"],
                output_required: &["action", "threadId", "turnId"],
                nullable_output: &[],
            },
        ),
        (
            "thread_title_set",
            SchemaContract {
                input_properties: &["threadId", "title"],
                input_required: &["title"],
                output_properties: &[],
                output_required: &[],
                nullable_output: &[],
            },
        ),
        (
            "thread_pin_set",
            SchemaContract {
                input_properties: &["pinned", "threadId"],
                input_required: &["pinned"],
                output_properties: &["pinned", "threadId"],
                output_required: &["pinned", "threadId"],
                nullable_output: &[],
            },
        ),
        (
            "thread_goal_get",
            SchemaContract {
                input_properties: &["threadId"],
                input_required: &["threadId"],
                output_properties: &["goal"],
                output_required: &["goal"],
                nullable_output: &["goal"],
            },
        ),
        (
            "thread_goal_set",
            SchemaContract {
                input_properties: &["objective", "threadId"],
                input_required: &["objective", "threadId"],
                output_properties: &["goal"],
                output_required: &["goal"],
                nullable_output: &[],
            },
        ),
        (
            "thread_goal_pause",
            SchemaContract {
                input_properties: &["threadId"],
                input_required: &["threadId"],
                output_properties: &["goal"],
                output_required: &["goal"],
                nullable_output: &[],
            },
        ),
        (
            "thread_goal_resume",
            SchemaContract {
                input_properties: &["threadId"],
                input_required: &["threadId"],
                output_properties: &["goal"],
                output_required: &["goal"],
                nullable_output: &[],
            },
        ),
        (
            "thread_goal_clear",
            SchemaContract {
                input_properties: &["threadId"],
                input_required: &["threadId"],
                output_properties: &["cleared"],
                output_required: &["cleared"],
                nullable_output: &[],
            },
        ),
        (
            "thread_interrupt",
            SchemaContract {
                input_properties: &["threadId"],
                input_required: &["threadId"],
                output_properties: &[],
                output_required: &[],
                nullable_output: &[],
            },
        ),
    ]
    .into_iter()
    .collect()
}

fn assert_object_schema(schema: &Value, properties: &[&str], required: &[&str]) {
    assert_eq!(schema["type"], "object", "{schema}");
    assert_eq!(schema["additionalProperties"], false, "{schema}");
    assert_eq!(
        keys(&schema["properties"]),
        properties.iter().copied().collect(),
        "{schema}"
    );
    assert_eq!(
        string_set(&schema["required"]),
        required.iter().copied().collect(),
        "{schema}"
    );
    let required: BTreeSet<_> = required.iter().copied().collect();
    for property in properties {
        if !required.contains(property) {
            assert!(
                !permits_null(&schema["properties"][property]),
                "optional property {property} must reject explicit null: {schema}"
            );
        }
    }
}

fn assert_stable_output_definitions(tools: &[Value]) {
    let expected: BTreeMap<&str, (&[&str], &[&str])> = [
        (
            "Thread",
            (
                &[
                    "createdAt",
                    "cwd",
                    "forkedFromId",
                    "id",
                    "name",
                    "preview",
                    "status",
                    "updatedAt",
                ][..],
                &["forkedFromId", "name"][..],
            ),
        ),
        (
            "Turn",
            (
                &[
                    "completedAt",
                    "durationMs",
                    "error",
                    "id",
                    "items",
                    "itemsView",
                    "startedAt",
                    "status",
                ][..],
                &["completedAt", "durationMs", "error", "startedAt"][..],
            ),
        ),
        (
            "ThreadGoal",
            (
                &[
                    "createdAt",
                    "objective",
                    "status",
                    "threadId",
                    "timeUsedSeconds",
                    "tokenBudget",
                    "tokensUsed",
                    "updatedAt",
                ][..],
                &["tokenBudget"][..],
            ),
        ),
        (
            "ThreadSnapshot",
            (
                &[
                    "activeTurnId",
                    "activeTurnStatus",
                    "name",
                    "status",
                    "threadId",
                    "updatedAt",
                ][..],
                &["activeTurnId", "activeTurnStatus", "name"][..],
            ),
        ),
    ]
    .into_iter()
    .collect();

    for (definition_name, (properties, nullable)) in expected {
        let definition = tools
            .iter()
            .find_map(|tool| tool["outputSchema"]["$defs"].get(definition_name))
            .unwrap_or_else(|| panic!("missing output definition {definition_name}"));
        assert_object_schema(definition, properties, properties);
        for property in nullable {
            assert!(
                permits_null(&definition["properties"][property]),
                "{definition_name}.{property} must permit null"
            );
        }
        if definition_name == "Turn" {
            assert_eq!(definition["properties"]["items"]["items"]["type"], "object");
        }
    }
}

fn property_descriptions(schema: &Value) -> BTreeMap<&str, &str> {
    schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, property)| {
            property["description"]
                .as_str()
                .map(|description| (name.as_str(), description))
        })
        .collect()
}

fn assert_nested_output_definitions_have_no_descriptions(tools: &[Value]) {
    for tool in tools {
        let Some(definitions) = tool["outputSchema"]["$defs"].as_object() else {
            continue;
        };
        for (name, definition) in definitions {
            assert!(
                !contains_description(definition),
                "nested output definition {name} unexpectedly contains model-facing text"
            );
        }
    }
}

fn contains_description(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key("description") || object.values().any(contains_description)
        }
        Value::Array(values) => values.iter().any(contains_description),
        _ => false,
    }
}

fn permits_null(schema: &Value) -> bool {
    schema["type"] == "null"
        || schema["type"]
            .as_array()
            .is_some_and(|types| types.iter().any(|value| value == "null"))
        || schema["anyOf"]
            .as_array()
            .is_some_and(|variants| variants.iter().any(permits_null))
        || schema["oneOf"]
            .as_array()
            .is_some_and(|variants| variants.iter().any(permits_null))
}

fn assert_interrupt_output_schema(schema: &Value) {
    let variants = schema["oneOf"].as_array().unwrap();
    assert_eq!(variants.len(), 2);
    let by_interrupted: BTreeMap<bool, &Value> = variants
        .iter()
        .map(|variant| {
            (
                variant["properties"]["interrupted"]["const"]
                    .as_bool()
                    .unwrap(),
                variant,
            )
        })
        .collect();
    assert_object_schema(
        by_interrupted.get(&true).unwrap(),
        &["interrupted", "turnId"],
        &["interrupted", "turnId"],
    );
    assert_object_schema(
        by_interrupted.get(&false).unwrap(),
        &["interrupted"],
        &["interrupted"],
    );
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn string_set(value: &Value) -> BTreeSet<&str> {
    value
        .as_array()
        .map(|values| values.iter().map(|value| value.as_str().unwrap()).collect())
        .unwrap_or_default()
}
