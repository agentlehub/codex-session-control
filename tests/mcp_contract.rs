#[path = "support/process_guard.rs"]
mod process_guard;

use process_guard::{ChildGuard, PIPE_CAPTURE_LIMIT, terminate_test_child, wait_for_child_exit};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use assert_cmd::cargo::cargo_bin;
use serde_json::{Value, json};

const CATALOG_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

const TOOL_EFFECTS: [(&str, bool, bool); 13] = [
    ("thread_create", false, false),
    ("thread_fork", false, false),
    ("threads_list", true, false),
    ("thread_read", true, false),
    ("threads_wait", true, false),
    ("thread_message_send", false, false),
    ("thread_title_set", false, false),
    ("thread_goal_get", true, false),
    ("thread_goal_set", false, false),
    ("thread_goal_pause", false, false),
    ("thread_goal_resume", false, false),
    ("thread_goal_clear", false, true),
    ("thread_interrupt", false, true),
];

#[test]
fn child_guard_captures_runtime_error_after_stdin_eof() {
    let mut command = Command::new(cargo_bin("codex-session-control"));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = ChildGuard::spawn(&mut command).unwrap();
    let output = child.wait_with_output(CATALOG_EXIT_TIMEOUT).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn binary_is_direct_stdio_and_accepts_no_commands() {
    let mut command = Command::new(cargo_bin("codex-session-control"));
    command
        .arg("mcp-server")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = ChildGuard::spawn(&mut command).unwrap();
    let child_pid = child.id();
    let output = child.wait_with_output(CATALOG_EXIT_TIMEOUT).unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    assert!(
        !std::path::Path::new(&format!("/proc/{child_pid}")).exists(),
        "argument-rejection child was not reaped"
    );
}

#[test]
fn child_guard_terminates_and_reaps_on_timeout() {
    let mut command = Command::new(cargo_bin("codex-session-control"));
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = ChildGuard::spawn(&mut command).unwrap();
    let child_pid = child.id();
    let error = child
        .wait_with_output(Duration::from_millis(100))
        .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(
        !std::path::Path::new(&format!("/proc/{child_pid}")).exists(),
        "timed-out child was not reaped"
    );
}

#[test]
fn child_guard_rejects_successful_output_that_exceeds_the_capture_limit() {
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            &format!("head -c {} /dev/zero", PIPE_CAPTURE_LIMIT + 1),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = ChildGuard::spawn(&mut command).unwrap();
    let error = child
        .wait_with_output(CATALOG_EXIT_TIMEOUT)
        .expect_err("successful oversized output must fail closed");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn child_guard_times_out_when_detached_writer_holds_pipes() {
    struct DetachedPipeHolder {
        pid: Option<i32>,
    }

    impl DetachedPipeHolder {
        fn await_pid(pid_file: &std::path::Path) -> Self {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                if let Ok(value) = std::fs::read_to_string(pid_file)
                    && let Ok(pid) = value.trim().parse::<i32>()
                {
                    return Self { pid: Some(pid) };
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "detached pipe holder did not record a pid"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn terminate_and_await_disappearance(&mut self) -> io::Result<()> {
            let pid = self
                .pid
                .ok_or_else(|| io::Error::other("detached pid was already gone"))?;
            let detached = rustix::process::Pid::from_raw(pid)
                .ok_or_else(|| io::Error::other("detached pid is zero"))?;
            match rustix::process::kill_process(detached, rustix::process::Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                Err(error) => return Err(io::Error::from(error)),
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::path::Path::new(&format!("/proc/{pid}")).exists() {
                if std::time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "detached pipe holder did not disappear",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
            self.pid = None;
            Ok(())
        }
    }

    impl Drop for DetachedPipeHolder {
        fn drop(&mut self) {
            if self.pid.is_some() {
                let _ = self.terminate_and_await_disappearance();
            }
        }
    }

    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("detached-pipe-holder.pid");
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("setsid sh -c 'printf \"%s\\n\" \"$$\" > \"$1\"; exec sleep 60' detached \"$1\" &")
        .arg("guard-fixture")
        .arg(&pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = ChildGuard::spawn(&mut command).unwrap();
    let mut detached = DetachedPipeHolder::await_pid(&pid_file);

    let started = std::time::Instant::now();
    let error = child
        .wait_with_output(Duration::from_millis(100))
        .expect_err("detached pipe holder must not block the guard");
    let elapsed = started.elapsed();
    let detached_pid = detached.pid.expect("detached holder pid remains available");
    detached
        .terminate_and_await_disappearance()
        .expect("detached pipe holder must be explicitly terminated");

    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(
        elapsed < Duration::from_millis(200),
        "detached pipe holder delayed deadline completion for {elapsed:?}"
    );
    assert!(
        !std::path::Path::new(&format!("/proc/{detached_pid}")).exists(),
        "detached pipe holder must be explicitly cleaned up"
    );
}

#[test]
fn child_guard_rejects_unthrottled_single_stream_output_promptly() {
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            "i=0; while [ \"$i\" -lt 32 ]; do yes stdout & i=$((i + 1)); done; wait",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = ChildGuard::spawn(&mut command).unwrap();
    let child_pid = child.id();
    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    let watchdog = thread::spawn(
        move || match completed_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(()) => false,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let process_group = rustix::process::Pid::from_raw(child_pid as i32).unwrap();
                let _ = rustix::process::kill_process_group(
                    process_group,
                    rustix::process::Signal::KILL,
                );
                true
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => false,
        },
    );

    let started = std::time::Instant::now();
    let error = child
        .wait_with_output(Duration::from_millis(100))
        .unwrap_err();
    let elapsed = started.elapsed();
    let _ = completed_tx.send(());
    let watchdog_fired = watchdog.join().unwrap();
    let diagnostic = error.to_string();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        !watchdog_fired,
        "independent watchdog had to stop an unbounded output drain"
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "capture limit was not enforced promptly: {elapsed:?}"
    );
    assert!(
        !std::path::Path::new(&format!("/proc/{child_pid}")).exists(),
        "unthrottled writer leader was not reaped"
    );
    assert!(
        diagnostic.len() <= 1024,
        "capture-limit diagnostic retained {} bytes",
        diagnostic.len()
    );
}

#[test]
fn child_guard_drop_terminates_and_reaps() {
    let mut command = Command::new(cargo_bin("codex-session-control"));
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = ChildGuard::spawn(&mut command).unwrap();
    let child_pid = child.id();
    drop(child);

    assert!(
        !std::path::Path::new(&format!("/proc/{child_pid}")).exists(),
        "dropped child was not reaped"
    );
}

#[test]
fn child_guard_drop_terminates_process_group() {
    let mut leader_command = Command::new(cargo_bin("codex-session-control"));
    leader_command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let leader = ChildGuard::spawn(&mut leader_command).unwrap();

    let mut member_command = Command::new(cargo_bin("codex-session-control"));
    member_command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(leader.id() as i32);
    let mut member = member_command.spawn().unwrap();

    drop(leader);
    let member_status = match wait_for_child_exit(&mut member, CATALOG_EXIT_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) => {
            terminate_test_child(&mut member);
            panic!("dropped child left a process-group member running");
        }
        Err(error) => {
            terminate_test_child(&mut member);
            panic!("failed while waiting for process-group member: {error}");
        }
    };
    assert_eq!(
        member_status.signal(),
        Some(rustix::process::Signal::KILL.as_raw()),
        "process-group member did not receive SIGKILL"
    );
}

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

    let mut command = Command::new(cargo_bin("codex-session-control"));
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ChildGuard::spawn(&mut command).unwrap();
    let child_pid = child.id();
    {
        let stdin = child.stdin_mut().unwrap();
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
    child.close_stdin();
    let output = child.wait_with_output(CATALOG_EXIT_TIMEOUT).unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !std::path::Path::new(&format!("/proc/{child_pid}")).exists(),
        "mcp-server remained after stdin EOF"
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let responses: Vec<Value> = stdout
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
        assert_object_schema(
            &tool["inputSchema"],
            contract.input_properties,
            contract.input_required,
        );
        if name == "thread_interrupt" {
            assert!(
                !permits_null(&tool["inputSchema"]["properties"]["threadId"]),
                "thread_interrupt.threadId must reject explicit null"
            );
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
                input_properties: &["includeDescendants", "threadId"],
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
    assert_closed_object_schema(schema, properties, required);
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
    assert_local_references_resolve(schema);
    assert_output_objects_closed(schema);
    assert_interrupt_result_variants(schema, true);
}

fn assert_interrupt_result_variants(schema: &Value, descendants: bool) {
    assert_eq!(variants(schema).len(), 2, "{schema}");
    for interrupted in [false, true] {
        let variant = semantic_variant(schema, "interrupted", Some(interrupted));
        let mut properties = vec!["interrupted"];
        let mut required = vec!["interrupted"];
        if interrupted {
            properties.push("turnId");
            required.push("turnId");
        }
        if descendants {
            properties.push("descendants");
        }
        assert_object_schema(variant, &properties, &required);
        if interrupted {
            assert_eq!(variant["properties"]["turnId"]["type"], "string");
        }
        if descendants {
            assert_descendants_schema(&variant["properties"]["descendants"]);
        }
    }
}

fn assert_descendants_schema(schema: &Value) {
    assert_eq!(variants(schema).len(), 3, "{schema}");
    let warning = semantic_variant(schema, "warning", None);
    assert_object_schema(warning, &["warning"], &["warning"]);
    let warning = &warning["properties"]["warning"];
    assert_object_schema(
        warning,
        &["activeCount", "activeThreadIds", "code"],
        &["activeCount", "activeThreadIds", "code"],
    );
    assert_eq!(
        warning["properties"]["code"]["const"],
        "active_descendants_not_interrupted"
    );
    assert_eq!(warning["properties"]["activeCount"]["type"], "integer");
    assert_eq!(warning["properties"]["activeCount"]["minimum"], 0);
    assert_eq!(warning["properties"]["activeThreadIds"]["type"], "array");
    assert_eq!(
        warning["properties"]["activeThreadIds"]["items"]["type"],
        "string"
    );

    let results = semantic_variant(schema, "results", None);
    assert_object_schema(results, &["results"], &["results"]);
    let results = &results["properties"]["results"];
    assert_eq!(results["type"], "array");
    assert_eq!(
        results
            .get("minItems")
            .map(|value| value.as_u64().expect("minItems must be an integer"))
            .unwrap_or_default(),
        0,
        "descendant results must allow an empty array: {results}"
    );
    assert_eq!(variants(&results["items"]).len(), 2, "{results}");
    let result = semantic_variant(&results["items"], "result", None);
    assert_object_schema(result, &["result", "threadId"], &["result", "threadId"]);
    assert_eq!(result["properties"]["threadId"]["type"], "string");
    assert_exact_interrupt_result_schema(&result["properties"]["result"]);
    let target_error = semantic_variant(&results["items"], "error", None);
    assert_object_schema(target_error, &["error", "threadId"], &["error", "threadId"]);
    assert_eq!(target_error["properties"]["threadId"]["type"], "string");
    assert_tool_error_schema(&target_error["properties"]["error"], None);

    let discovery_error = semantic_variant(schema, "error", None);
    assert_object_schema(discovery_error, &["error"], &["error"]);
    assert_tool_error_schema(
        &discovery_error["properties"]["error"],
        Some("descendant_discovery_failed"),
    );
}

fn assert_exact_interrupt_result_schema(schema: &Value) {
    assert_interrupt_result_variants(schema, false);
}

fn assert_tool_error_schema(schema: &Value, code: Option<&str>) {
    let mut properties = vec![
        "category",
        "dispatch",
        "message",
        "native",
        "observation",
        "reconciliationError",
        "stage",
        "threadId",
        "tool",
        "turnId",
    ];
    let mut required = vec!["category", "message", "stage", "tool"];
    if code.is_some() {
        properties.push("code");
        required.push("code");
    }
    assert_closed_object_schema(schema, &properties, &required);
    if let Some(code) = code {
        assert_eq!(schema["properties"]["code"]["const"], code);
    }
}

fn assert_closed_object_schema(schema: &Value, properties: &[&str], required: &[&str]) {
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
}

fn semantic_variant<'a>(
    schema: &'a Value,
    property: &str,
    boolean_const: Option<bool>,
) -> &'a Value {
    let matches = variants(schema)
        .iter()
        .filter(|variant| {
            variant["properties"].get(property).is_some()
                && boolean_const.is_none_or(|value| {
                    variant["properties"][property]["const"].as_bool() == Some(value)
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "expected one semantic variant: {schema}");
    matches[0]
}

fn variants(schema: &Value) -> &[Value] {
    let one_of = schema.get("oneOf").and_then(Value::as_array);
    let any_of = schema.get("anyOf").and_then(Value::as_array);
    assert!(
        matches!((one_of, any_of), (Some(_), None) | (None, Some(_))),
        "expected exactly one schema union: {schema}"
    );
    let variants = one_of.or(any_of).unwrap();
    assert!(
        !variants.is_empty(),
        "schema union must not be empty: {schema}"
    );
    variants
}

fn assert_local_references_resolve(root: &Value) {
    fn visit(value: &Value, root: &Value) {
        if let Some(reference) = value.get("$ref") {
            let reference = reference.as_str().expect("$ref must be a string");
            assert!(
                reference.starts_with("#/"),
                "schema reference must be local: {reference}"
            );
            assert!(
                root.pointer(&reference[1..]).is_some(),
                "unresolved local schema reference: {reference}"
            );
        }
        match value {
            Value::Object(object) => object.values().for_each(|value| visit(value, root)),
            Value::Array(values) => values.iter().for_each(|value| visit(value, root)),
            _ => {}
        }
    }

    visit(root, root);
}

fn assert_output_objects_closed(schema: &Value) {
    if schema["type"] == "object" {
        assert_eq!(schema["additionalProperties"], false, "{schema}");
    }
    match schema {
        Value::Object(object) => object.values().for_each(assert_output_objects_closed),
        Value::Array(values) => values.iter().for_each(assert_output_objects_closed),
        _ => {}
    }
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
