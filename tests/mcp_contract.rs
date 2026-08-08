use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use assert_cmd::cargo::cargo_bin;
use serde_json::{Value, json};

const INSTRUCTIONS: &str = "These tools inspect and control Codex threads through the shared app-server used by connected Codex clients.";
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CATALOG_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const PIPE_CAPTURE_LIMIT: usize = 64 * 1024;
const CONTINUOUS_OUTPUT_FIXTURE: &str = "CSC_MCP_CONTRACT_CONTINUOUS_OUTPUT";

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

struct ChildGuard {
    child: Option<Child>,
    process_group: Option<u32>,
}

impl ChildGuard {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        command.process_group(0);
        command.spawn().map(|child| Self {
            process_group: Some(child.id()),
            child: Some(child),
        })
    }

    fn id(&self) -> u32 {
        self.child.as_ref().expect("child already reaped").id()
    }

    fn stdin_mut(&mut self) -> Option<&mut ChildStdin> {
        self.child.as_mut()?.stdin.as_mut()
    }

    fn close_stdin(&mut self) {
        drop(
            self.child
                .as_mut()
                .expect("child already reaped")
                .stdin
                .take(),
        );
    }

    fn wait_with_output(mut self, timeout: Duration) -> io::Result<Output> {
        let child = self.child.as_mut().expect("child already reaped");
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("child stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("child stderr was not piped"))?;
        let stdout_reader = read_pipe(stdout);
        let stderr_reader = read_pipe(stderr);
        let deadline = Instant::now() + timeout;

        loop {
            match self
                .child
                .as_mut()
                .expect("child already reaped")
                .try_wait()
            {
                Ok(Some(status)) => {
                    self.child.take();
                    let cleanup_result = self.terminate_process_group();
                    let output_result = collect_output(status, stdout_reader, stderr_reader);
                    cleanup_result?;
                    return output_result;
                }
                Ok(None) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        self.terminate_and_reap()?;
                        let stdout_result = join_pipe(stdout_reader, "stdout");
                        let stderr_result = join_pipe(stderr_reader, "stderr");
                        let stdout = stdout_result?;
                        let stderr = stderr_result?;
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!(
                                "child did not exit within {timeout:?}; stdout: {}; stderr: {}",
                                stdout, stderr
                            ),
                        ));
                    }
                    thread::sleep(CHILD_POLL_INTERVAL.min(remaining));
                }
                Err(error) => {
                    self.terminate_and_reap()?;
                    let stdout_result = join_pipe(stdout_reader, "stdout");
                    let stderr_result = join_pipe(stderr_reader, "stderr");
                    let _ = stdout_result?;
                    let _ = stderr_result?;
                    return Err(error);
                }
            }
        }
    }

    fn terminate_and_reap(&mut self) -> io::Result<()> {
        let group_result = self.terminate_process_group();
        let child_result = self.terminate_child();
        match (group_result, child_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(group_error), Err(child_error)) => Err(io::Error::other(format!(
                "process-group cleanup failed: {group_error}; child cleanup failed: {child_error}"
            ))),
        }
    }

    fn terminate_process_group(&mut self) -> io::Result<()> {
        let Some(process_group) = self.process_group else {
            return Ok(());
        };
        let process_group = rustix::process::Pid::from_raw(process_group as i32)
            .ok_or_else(|| io::Error::other("process group id is zero"))?;
        match rustix::process::kill_process_group(process_group, rustix::process::Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {
                self.process_group.take();
                Ok(())
            }
            Err(error) => Err(io::Error::from(error)),
        }
    }

    fn terminate_child(&mut self) -> io::Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if let Ok(Some(_)) = child.try_wait() {
            self.child.take();
            return Ok(());
        }
        if let Err(kill_error) = child.kill() {
            return match child.try_wait() {
                Ok(Some(_)) => {
                    self.child.take();
                    Ok(())
                }
                Ok(None) => Err(kill_error),
                Err(recheck_error) => Err(io::Error::other(format!(
                    "child kill failed: {kill_error}; exit recheck failed: {recheck_error}"
                ))),
            };
        }
        match child.wait() {
            Ok(_) => {
                self.child.take();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.terminate_and_reap();
    }
}

struct PipeCapture {
    bytes: Vec<u8>,
    total_bytes: usize,
}

fn write_lossy_bytes(
    formatter: &mut std::fmt::Formatter<'_>,
    mut bytes: &[u8],
) -> std::fmt::Result {
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(text) => return formatter.write_str(text),
            Err(error) => {
                let valid_len = error.valid_up_to();
                if valid_len > 0 {
                    let valid = std::str::from_utf8(&bytes[..valid_len])
                        .expect("valid_up_to must delimit valid UTF-8");
                    formatter.write_str(valid)?;
                }
                formatter.write_str("?")?;
                let invalid_len = error.error_len().unwrap_or(bytes.len() - valid_len);
                bytes = &bytes[valid_len + invalid_len..];
            }
        }
    }
    Ok(())
}

impl std::fmt::Display for PipeCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_lossy_bytes(formatter, &self.bytes)?;
        if self.total_bytes > self.bytes.len() {
            write!(
                formatter,
                " [truncated: captured first {} of {} bytes]",
                self.bytes.len(),
                self.total_bytes
            )?;
        }
        Ok(())
    }
}

fn read_pipe(mut pipe: impl Read + Send + 'static) -> JoinHandle<io::Result<PipeCapture>> {
    thread::spawn(move || {
        let mut captured = Vec::with_capacity(PIPE_CAPTURE_LIMIT);
        let mut total_bytes = 0usize;
        let mut buffer = [0u8; 8 * 1024];
        loop {
            let bytes_read = match pipe.read(&mut buffer) {
                Ok(bytes_read) => bytes_read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if bytes_read == 0 {
                break;
            }
            total_bytes = total_bytes.saturating_add(bytes_read);
            let retained = bytes_read.min(PIPE_CAPTURE_LIMIT - captured.len());
            captured.extend_from_slice(&buffer[..retained]);
        }
        Ok(PipeCapture {
            bytes: captured,
            total_bytes,
        })
    })
}

fn join_pipe(reader: JoinHandle<io::Result<PipeCapture>>, stream: &str) -> io::Result<PipeCapture> {
    reader
        .join()
        .map_err(|_| io::Error::other(format!("{stream} reader panicked")))?
}

fn collect_output(
    status: ExitStatus,
    stdout_reader: JoinHandle<io::Result<PipeCapture>>,
    stderr_reader: JoinHandle<io::Result<PipeCapture>>,
) -> io::Result<Output> {
    let stdout = join_pipe(stdout_reader, "stdout");
    let stderr = join_pipe(stderr_reader, "stderr");
    Ok(Output {
        status,
        stdout: stdout?.bytes,
        stderr: stderr?.bytes,
    })
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        thread::sleep(CHILD_POLL_INTERVAL.min(remaining));
    }
}

fn terminate_test_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    if child.kill().is_ok() {
        let _ = child.wait();
    }
}

#[test]
fn pipe_capture_bounds_invalid_utf8_diagnostics() {
    let capture = PipeCapture {
        bytes: vec![0xff; PIPE_CAPTURE_LIMIT],
        total_bytes: PIPE_CAPTURE_LIMIT * 2,
    };

    let diagnostic = format!("stdout: {capture}; stderr: {capture}");

    assert!(
        diagnostic.len() <= PIPE_CAPTURE_LIMIT * 2 + 256,
        "invalid UTF-8 expanded diagnostic to {} bytes",
        diagnostic.len()
    );
    assert!(
        diagnostic.contains('?'),
        "invalid bytes must remain visible"
    );
    assert_eq!(diagnostic.matches("[truncated:").count(), 2);
}

#[test]
fn read_pipe_retries_interrupted_reads() {
    struct InterruptedOnce<R> {
        inner: R,
        interrupted: bool,
    }

    impl<R: Read> Read for InterruptedOnce<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.inner.read(buffer)
        }
    }

    let expected = b"captured after an interrupted read".to_vec();
    let capture = join_pipe(
        read_pipe(InterruptedOnce {
            inner: io::Cursor::new(expected.clone()),
            interrupted: false,
        }),
        "test",
    )
    .unwrap();

    assert_eq!(capture.bytes, expected);
    assert_eq!(capture.total_bytes, expected.len());
}

#[test]
fn child_guard_captures_output_after_normal_exit() {
    let mut command = Command::new(cargo_bin("codex-session-control"));
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = ChildGuard::spawn(&mut command).unwrap();
    let output = child.wait_with_output(CATALOG_EXIT_TIMEOUT).unwrap();

    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("codex-session-control ")
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn child_guard_terminates_and_reaps_on_timeout() {
    let mut command = Command::new(cargo_bin("codex-session-control"));
    command
        .arg("mcp-server")
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
fn child_guard_bounds_continuously_logged_timeout_output() {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "child_guard_continuous_output_fixture",
            "--nocapture",
        ])
        .env(CONTINUOUS_OUTPUT_FIXTURE, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = ChildGuard::spawn(&mut command).unwrap();
    let child_pid = child.id();
    let error = child.wait_with_output(Duration::from_secs(1)).unwrap_err();
    let diagnostic = error.to_string();

    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(
        !std::path::Path::new(&format!("/proc/{child_pid}")).exists(),
        "continuously logging child was not reaped"
    );
    assert!(
        diagnostic.len() <= PIPE_CAPTURE_LIMIT * 2 + 1024,
        "timeout diagnostic retained {} bytes",
        diagnostic.len()
    );
    assert!(diagnostic.contains("stdout diagnostic"));
    assert!(diagnostic.contains("stderr diagnostic"));
    let capture_prefix = format!("captured first {PIPE_CAPTURE_LIMIT} of ");
    let totals = diagnostic
        .match_indices(&capture_prefix)
        .map(|(index, _)| {
            diagnostic[index + capture_prefix.len()..]
                .split_once(" bytes]")
                .unwrap()
                .0
                .parse::<usize>()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        totals.len(),
        2,
        "both streams must report the capture bound"
    );
    assert!(
        totals.iter().all(|total| *total > PIPE_CAPTURE_LIMIT * 2),
        "both readers must drain beyond the retained prefix: {totals:?}"
    );
}

#[test]
fn child_guard_continuous_output_fixture() {
    if std::env::var_os(CONTINUOUS_OUTPUT_FIXTURE).is_none() {
        return;
    }

    let stdout_chunk = "stdout diagnostic\n".repeat(4096);
    let stderr_chunk = "stderr diagnostic\n".repeat(4096);
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    loop {
        stdout.write_all(stdout_chunk.as_bytes()).unwrap();
        stderr.write_all(stderr_chunk.as_bytes()).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn child_guard_drop_terminates_and_reaps() {
    let mut command = Command::new(cargo_bin("codex-session-control"));
    command
        .arg("mcp-server")
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
        .arg("mcp-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let leader = ChildGuard::spawn(&mut leader_command).unwrap();

    let mut member_command = Command::new(cargo_bin("codex-session-control"));
    member_command
        .arg("mcp-server")
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
        .arg("mcp-server")
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
