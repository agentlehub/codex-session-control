use std::collections::BTreeMap;

use super::*;

#[test]
fn schema_digest_canonicalizes_objects_but_preserves_array_order() {
    let first = crate::test_support::private_tempdir();
    let second = crate::test_support::private_tempdir();
    std::fs::write(
        first.path().join("schema.json"),
        br#"{"z":{"second":2,"first":1},"a":[1,2],"scalar":"same"}"#,
    )
    .unwrap();
    std::fs::write(
        second.path().join("schema.json"),
        br#"{"scalar":"same","a":[1,2],"z":{"first":1,"second":2}}"#,
    )
    .unwrap();

    assert_eq!(
        aggregate_schema_digest(first.path()).unwrap(),
        aggregate_schema_digest(second.path()).unwrap()
    );

    std::fs::write(
        second.path().join("schema.json"),
        br#"{"scalar":"same","a":[2,1],"z":{"first":1,"second":2}}"#,
    )
    .unwrap();
    assert_ne!(
        aggregate_schema_digest(first.path()).unwrap(),
        aggregate_schema_digest(second.path()).unwrap()
    );
}

#[tokio::test]
#[ignore = "writes a fixture only when explicitly selected"]
async fn capture_protocol_fixture() {
    capture_fixture_from_live_codex().await.unwrap();
}

#[test]
fn capture_protocol_fixture_uses_disposable_normal_home_shape() {
    let temporary = Path::new("/capture-root");
    let fixture = normalize_fixture(
        json!({"codexHome": fixture_codex_home(temporary)}),
        &HashMap::new(),
        temporary.to_str().unwrap(),
    );

    assert_eq!(
        fixture["codexHome"],
        "/tmp/codex-session-control-fixture/home/.codex"
    );
}

#[test]
fn fixture_normalization_zeroes_section_entry_time() {
    let fixture = normalize_fixture(
        json!({"thread": {"sectionEnteredAt": 123}}),
        &HashMap::new(),
        "/capture-root",
    );

    assert_eq!(fixture["thread"]["sectionEnteredAt"], 0);
}

fn fixture_codex_home(temporary: &Path) -> PathBuf {
    temporary.join("home/.codex")
}

struct DisposableCodex {
    child: Child,
}

impl Drop for DisposableCodex {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

struct ResponsesEndpoint {
    address: std::net::SocketAddr,
    requests: Arc<AtomicUsize>,
    held_started: Arc<Notify>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

async fn capture_fixture_from_live_codex() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::var_os("CODEX_SESSION_CONTROL_FIXTURE_OUT")
        .ok_or("CODEX_SESSION_CONTROL_FIXTURE_OUT is required")?;
    let output = PathBuf::from(output);
    if !output.is_absolute() {
        return Err("CODEX_SESSION_CONTROL_FIXTURE_OUT must be absolute".into());
    }

    let version_output = Command::new("codex").arg("--version").output().await?;
    let expected_version = format!("codex-cli {TESTED_CODEX_VERSION}");
    if !version_output.status.success()
        || String::from_utf8(version_output.stdout)?.trim() != expected_version
    {
        return Err(
            format!("fixture capture requires exact codex-cli {TESTED_CODEX_VERSION}").into(),
        );
    }

    validate_responses_fixture()?;
    let temporary = crate::test_support::private_tempdir();
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))?;
    let schema_dir = temporary.path().join("schema");
    let home = temporary.path().join("home");
    let codex_home = fixture_codex_home(temporary.path());
    let runtime_dir = temporary.path().join("runtime");
    let workspace = temporary.path().join("workspace");
    for directory in [&schema_dir, &home, &codex_home, &runtime_dir, &workspace] {
        std::fs::create_dir(directory)?;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }

    let schema_status = Command::new("codex")
        .args([
            "app-server",
            "generate-json-schema",
            "--experimental",
            "--out",
        ])
        .arg(&schema_dir)
        .env("CODEX_HOME", &codex_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    if !schema_status.success() {
        return Err("experimental JSON schema generation failed".into());
    }
    let schema_sha256 = aggregate_schema_digest(&schema_dir)?;

    let endpoint = start_responses_endpoint().await?;
    let config = format!(
        r#"model = "session-control-test"
model_provider = "session-control-local"

[model_providers.session-control-local]
name = "Session control local test"
base_url = "http://{}/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = 10000

[analytics]
enabled = false
"#,
        endpoint.address
    );
    std::fs::write(codex_home.join("config.toml"), config)?;

    let socket_path = runtime_dir.join("app-server.sock");
    let mut codex = DisposableCodex {
        child: Command::new("codex")
            .args(["app-server", "--listen"])
            .arg(format!("unix://{}", socket_path.display()))
            .env("CODEX_HOME", &codex_home)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    };
    wait_for_socket(&mut codex.child, &socket_path).await?;

    let client = AppServerClient::new(
        socket_path,
        codex_home.clone(),
        env!("CARGO_PKG_VERSION").to_owned(),
        TESTED_CODEX_VERSION.to_owned(),
    );
    let mut connection = client
        .connect_initialized()
        .await
        .map_err(fixture_tool_error)?;
    let initialize = connection
        .initialize_result
        .clone()
        .ok_or("initialize result was not captured")?;

    let thread_start: Value = connection
        .request(
            "thread/start",
            json!({
                "model": "session-control-test",
                "modelProvider": "session-control-local",
                "cwd": workspace,
                "approvalPolicy": "never",
                "sandbox": "read-only"
            }),
        )
        .await
        .map_err(fixture_tool_error)?;
    let thread_id = thread_start["thread"]["id"]
        .as_str()
        .ok_or("thread/start did not return a thread id")?
        .to_owned();
    let safe_thread_start = safe_thread_exemplar(&thread_start)?;

    let thread_read: Value = connection
        .request(
            "thread/read",
            json!({"threadId": thread_id, "includeTurns": false}),
        )
        .await
        .map_err(fixture_tool_error)?;
    let safe_thread_read = safe_thread_exemplar(&thread_read)?;

    let first_turn = start_fixture_turn(&mut connection, &thread_id).await?;
    wait_for_responses_requests(&endpoint, 1).await?;
    wait_for_turn_status(&mut connection, &thread_id, &first_turn, "completed").await?;

    let thread_section_move: Value = connection
        .mutate(
            "thread_pin_set",
            "thread/section/move",
            json!({
                "threadId": thread_id,
                "sectionId": "01984de2-8f74-7c91-a3b2-5c5e937cf318",
            }),
            Some(&thread_id),
            None,
        )
        .await
        .map_err(fixture_tool_error)?;
    if !thread_section_move.is_object() {
        return Err("thread/section/move returned a malformed acknowledgement".into());
    }
    let pinned: Value = connection
        .request(
            "thread/read",
            json!({"threadId": thread_id, "includeTurns": false}),
        )
        .await
        .map_err(fixture_tool_error)?;
    if pinned["thread"]["id"] != thread_id
        || pinned["thread"]["section"]["id"] != "01984de2-8f74-7c91-a3b2-5c5e937cf318"
    {
        return Err("thread/section/move did not pin the captured thread".into());
    }
    let mut safe_pinned_thread = safe_thread_exemplar(&pinned)?;
    safe_pinned_thread["thread"]["preview"] = json!("");
    let unpinned: Value = connection
        .mutate(
            "thread_pin_set",
            "thread/section/move",
            json!({"threadId": thread_id, "sectionId": null}),
            Some(&thread_id),
            None,
        )
        .await
        .map_err(fixture_tool_error)?;
    if !unpinned.is_object() {
        return Err("thread/section/move returned a malformed acknowledgement".into());
    }
    let unpinned_read: Value = connection
        .request(
            "thread/read",
            json!({"threadId": thread_id, "includeTurns": false}),
        )
        .await
        .map_err(fixture_tool_error)?;
    if !unpinned_read["thread"]["section"].is_null() {
        return Err("thread/section/move did not unpin the captured thread".into());
    }

    let second_turn = start_fixture_turn(&mut connection, &thread_id).await?;
    wait_for_responses_requests(&endpoint, 2).await?;
    let turns =
        wait_for_turn_status(&mut connection, &thread_id, &second_turn, "completed").await?;
    let returned_ids = turns["data"]
        .as_array()
        .ok_or("thread/turns/list data was not an array")?
        .iter()
        .filter_map(|turn| turn["id"].as_str())
        .collect::<Vec<_>>();
    if returned_ids.first() != Some(&second_turn.as_str())
        || returned_ids.get(1) != Some(&first_turn.as_str())
    {
        return Err("thread/turns/list did not return completed turns newest-first".into());
    }
    let thread_list = wait_for_thread_list(&mut connection, &thread_id).await?;
    let safe_thread_list = safe_thread_list_exemplar(&thread_list, &thread_id)?;

    let goal_set: Value = connection
        .mutate(
            "thread_goal_set",
            "thread/goal/set",
            json!({
                "threadId": thread_id,
                "objective": "fixture objective",
                "status": "paused",
                "tokenBudget": 1000
            }),
            Some(&thread_id),
            None,
        )
        .await
        .map_err(fixture_tool_error)?;
    let goal_get: Value = connection
        .request("thread/goal/get", json!({"threadId": thread_id}))
        .await
        .map_err(fixture_tool_error)?;
    let goal_clear: Value = connection
        .mutate(
            "thread_goal_clear",
            "thread/goal/clear",
            json!({"threadId": thread_id}),
            Some(&thread_id),
            None,
        )
        .await
        .map_err(fixture_tool_error)?;

    let thread_not_found = capture_native_error(
        &mut connection,
        "thread/read",
        json!({"threadId": "00000000-0000-7000-8000-000000000000"}),
    )
    .await?;

    let held_turn: Value = connection
        .request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{"type": "text", "text": "FIXTURE_PROMPT_SENTINEL"}]
            }),
        )
        .await
        .map_err(fixture_tool_error)?;
    let held_turn_id = held_turn["turn"]["id"]
        .as_str()
        .ok_or("held turn did not return an id")?
        .to_owned();
    let safe_held_turn = safe_turn_exemplar(&held_turn)?;
    endpoint.held_started.notified().await;
    if held_turn["turn"]["status"] != "inProgress" {
        return Err("held turn/start result was not inProgress".into());
    }
    let active_turn = safe_held_turn.clone();
    let active_turn_mismatch = capture_native_error(
        &mut connection,
        "turn/steer",
        json!({
            "threadId": thread_id,
            "expectedTurnId": "00000000-0000-7000-8000-000000000001",
            "input": [{"type": "text", "text": "fixture-conflict"}]
        }),
    )
    .await?;
    let interrupt: Value = connection
        .mutate(
            "turn_interrupt",
            "turn/interrupt",
            json!({"threadId": thread_id, "turnId": held_turn_id}),
            Some(&thread_id),
            Some(&held_turn_id),
        )
        .await
        .map_err(fixture_tool_error)?;
    let interrupted_turns =
        wait_for_turn_status(&mut connection, &thread_id, &held_turn_id, "interrupted").await?;
    if endpoint.requests.load(Ordering::SeqCst) != 3 {
        return Err("Responses endpoint did not receive exactly three requests".into());
    }
    tokio::time::timeout(Duration::from_secs(10), endpoint.task)
        .await
        .map_err(|_| "Responses endpoint did not finish after interruption")??
        .map_err(io::Error::other)?;

    let ids = HashMap::from([
        (thread_id.clone(), "thread_session_control_1"),
        (first_turn.clone(), "turn_session_control_1"),
        (second_turn.clone(), "turn_session_control_2"),
        (held_turn_id.clone(), "turn_session_control_held"),
    ]);
    let temporary_path = temporary.path().to_string_lossy().into_owned();
    let successful_exemplars = BTreeMap::from([
        ("initialize".to_owned(), initialize),
        ("threadStart".to_owned(), safe_thread_start),
        ("threadRead".to_owned(), safe_thread_read),
        ("threadReadPinned".to_owned(), safe_pinned_thread),
        ("threadSectionMove".to_owned(), thread_section_move),
        ("threadList".to_owned(), safe_thread_list),
        ("turnStart".to_owned(), safe_held_turn),
        ("turnsList".to_owned(), interrupted_turns),
        ("activeTurn".to_owned(), active_turn),
        ("goalSet".to_owned(), goal_set),
        ("goalGet".to_owned(), goal_get),
        ("goalClear".to_owned(), goal_clear),
        ("turnInterrupt".to_owned(), interrupt),
    ]);
    let error_exemplars = BTreeMap::from([
        ("threadNotFound".to_owned(), thread_not_found),
        ("activeTurnMismatch".to_owned(), active_turn_mismatch),
    ]);
    let fixture = json!({
        "codexVersion": TESTED_CODEX_VERSION,
        "schemaSha256": schema_sha256,
        "successfulExemplars": successful_exemplars,
        "errorExemplars": error_exemplars,
        "turnsNewestFirst": true
    });
    let fixture = normalize_fixture(fixture, &ids, &temporary_path);
    reject_sensitive_fixture(&fixture, &temporary_path)?;
    let mut serialized = serde_json::to_string_pretty(&fixture)?;
    serialized.push('\n');
    std::fs::write(output, serialized)?;
    Ok(())
}

fn validate_responses_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let transcript = include_str!("../../../tests/fixtures/responses-completed.sse");
    let blocks = transcript
        .trim_end_matches('\n')
        .split("\n\n")
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>();
    if blocks.len() != 9 || blocks[8] != "data: [DONE]" {
        return Err("Responses fixture has the wrong event count or terminator".into());
    }
    let expected_types = [
        "response.created",
        "response.output_item.added",
        "response.content_part.added",
        "response.output_text.delta",
        "response.output_text.done",
        "response.content_part.done",
        "response.output_item.done",
        "response.completed",
    ];
    let expected_keys: [&[&str]; 8] = [
        &["response", "sequence_number", "type"],
        &["item", "output_index", "sequence_number", "type"],
        &[
            "content_index",
            "item_id",
            "output_index",
            "part",
            "sequence_number",
            "type",
        ],
        &[
            "content_index",
            "delta",
            "item_id",
            "output_index",
            "sequence_number",
            "type",
        ],
        &[
            "content_index",
            "item_id",
            "output_index",
            "sequence_number",
            "text",
            "type",
        ],
        &[
            "content_index",
            "item_id",
            "output_index",
            "part",
            "sequence_number",
            "type",
        ],
        &["item", "output_index", "sequence_number", "type"],
        &["response", "sequence_number", "type"],
    ];
    for (index, block) in blocks[..8].iter().enumerate() {
        let data = block
            .strip_prefix("data: ")
            .ok_or("Responses fixture block lacks a data prefix")?;
        let event: Value = serde_json::from_str(data)?;
        if event["type"] != expected_types[index] || event["sequence_number"] != index {
            return Err(format!("Responses fixture event {index} has wrong type/sequence").into());
        }
        let mut keys = event
            .as_object()
            .ok_or("Responses fixture event was not an object")?
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        if keys != expected_keys[index] {
            return Err(format!("Responses fixture event {index} has wrong keys").into());
        }
    }

    let events = blocks[..8]
        .iter()
        .map(|block| serde_json::from_str::<Value>(block.strip_prefix("data: ").unwrap()).unwrap())
        .collect::<Vec<_>>();
    if events[0]["response"]["id"] != "resp_session_control_1"
        || events[0]["response"]["created_at"] != 0
        || events[0]["response"]["status"] != "in_progress"
        || events[1]["output_index"] != 0
        || events[1]["item"]["id"] != "msg_session_control_1"
        || events[1]["item"]["type"] != "message"
        || events[1]["item"]["content"] != json!([])
        || events[2]["content_index"] != 0
        || events[2]["part"]["type"] != "output_text"
        || events[3]["delta"] != "fixture-complete"
        || events[4]["text"] != "fixture-complete"
        || events[5]["part"]["text"] != "fixture-complete"
        || events[6]["item"]["status"] != "completed"
        || events[6]["item"]["content"][0]["text"] != "fixture-complete"
        || events[7]["response"]["id"] != "resp_session_control_1"
        || events[7]["response"]["created_at"] != 0
        || events[7]["response"]["status"] != "completed"
        || events[7]["response"]["output"][0]["status"] != "completed"
        || events[7]["response"]["output"][0]["content"][0]["text"] != "fixture-complete"
    {
        return Err("Responses fixture content contract is invalid".into());
    }
    Ok(())
}

fn fixture_tool_error(error: ToolErrorData) -> io::Error {
    io::Error::other(
        serde_json::to_string(&error)
            .unwrap_or_else(|_| "unserializable session-control tool error".to_owned()),
    )
}

fn aggregate_schema_digest(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    fn canonical_schema_bytes(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        fn order_objects(value: Value) -> Value {
            match value {
                Value::Object(object) => {
                    let mut entries = object.into_iter().collect::<Vec<_>>();
                    entries.sort_by(|left, right| left.0.cmp(&right.0));
                    Value::Object(
                        entries
                            .into_iter()
                            .map(|(key, value)| (key, order_objects(value)))
                            .collect(),
                    )
                }
                Value::Array(values) => {
                    Value::Array(values.into_iter().map(order_objects).collect())
                }
                scalar => scalar,
            }
        }

        let schema: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        Ok(serde_json::to_vec(&order_objects(schema))?)
    }

    fn collect(
        root: &Path,
        directory: &Path,
        output: &mut Vec<(String, PathBuf)>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect(root, &path, output)?;
            } else if entry.file_type()?.is_file() {
                let relative = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                output.push((relative, path));
            } else {
                return Err("schema bundle contains a non-file entry".into());
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, path) in files {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(canonical_schema_bytes(&path)?);
        hasher.update([0]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn start_responses_endpoint() -> Result<ResponsesEndpoint, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let requests = Arc::new(AtomicUsize::new(0));
    let held_started = Arc::new(Notify::new());
    let task_requests = Arc::clone(&requests);
    let task_held_started = Arc::clone(&held_started);
    let task = tokio::spawn(async move {
        for index in 0..3 {
            let (mut stream, peer) = listener.accept().await.map_err(|error| error.to_string())?;
            if !peer.ip().is_loopback() {
                return Err("Responses endpoint accepted a non-loopback peer".to_owned());
            }
            let request = read_http_request(&mut stream).await?;
            let request_line = request
                .lines()
                .next()
                .ok_or_else(|| "Responses endpoint request was empty".to_owned())?;
            if request_line != "POST /v1/responses HTTP/1.1" {
                return Err(format!("unexpected Responses request: {request_line}"));
            }
            let observed = task_requests.fetch_add(1, Ordering::SeqCst);
            if observed != index {
                return Err("Responses request FIFO order changed".to_owned());
            }

            let body = if index < 2 {
                include_str!("../../../tests/fixtures/responses-completed.sse").to_owned()
            } else {
                let first = include_str!("../../../tests/fixtures/responses-completed.sse")
                    .split("\n\n")
                    .next()
                    .ok_or_else(|| "missing response.created fixture event".to_owned())?;
                let mut created: Value = serde_json::from_str(
                    first
                        .strip_prefix("data: ")
                        .ok_or_else(|| "invalid response.created fixture event".to_owned())?,
                )
                .map_err(|error| error.to_string())?;
                created["response"]["id"] = json!("resp_session_control_held");
                format!("data: {}\n\n", created)
            };
            let headers = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: text/event-stream\r\n",
                "Cache-Control: no-cache\r\n",
                "Connection: close\r\n",
                "\r\n"
            );
            stream
                .write_all(headers.as_bytes())
                .await
                .map_err(|error| error.to_string())?;
            stream
                .write_all(body.as_bytes())
                .await
                .map_err(|error| error.to_string())?;
            stream.flush().await.map_err(|error| error.to_string())?;

            if index == 2 {
                task_held_started.notify_one();
                let mut buffer = [0_u8; 1];
                match stream.read(&mut buffer).await {
                    Ok(0) => {}
                    Ok(_) => {
                        return Err(
                            "held Responses connection received unexpected bytes".to_owned()
                        );
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::ConnectionReset
                                | io::ErrorKind::BrokenPipe
                                | io::ErrorKind::UnexpectedEof
                        ) => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
        }
        Ok(())
    });
    Ok(ResponsesEndpoint {
        address,
        requests,
        held_started,
        task,
    })
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Result<String, String> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if bytes.len() > 1_048_576 {
            return Err("Responses endpoint request exceeded 1 MiB".to_owned());
        }
        let mut chunk = [0_u8; 4096];
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("Responses endpoint request ended before headers".to_owned());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).map_err(|error| error.to_string())?;
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>())
            })
        })
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 4096];
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("Responses endpoint request body ended early".to_owned());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes[..header_end + content_length].to_vec())
        .map_err(|error| error.to_string())
}

async fn wait_for_socket(
    child: &mut Child,
    socket_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if socket_path.exists() {
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                return Err(io::Error::other(format!(
                    "Codex app-server exited before socket creation: {status}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| "Codex app-server socket creation timed out")??;
    Ok(())
}

fn safe_thread_exemplar(value: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let thread = value
        .get("thread")
        .and_then(Value::as_object)
        .ok_or("thread exemplar lacks a thread object")?;
    let mut safe = serde_json::Map::new();
    for key in [
        "id",
        "name",
        "preview",
        "cwd",
        "status",
        "createdAt",
        "updatedAt",
        "forkedFromId",
        "section",
        "sectionEnteredAt",
    ] {
        if let Some(value) = thread.get(key) {
            safe.insert(key.to_owned(), value.clone());
        }
    }
    Ok(json!({"thread": safe}))
}

fn safe_thread_list_exemplar(
    value: &Value,
    thread_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let thread = value["data"]
        .as_array()
        .and_then(|threads| {
            threads
                .iter()
                .find(|thread| thread["id"].as_str() == Some(thread_id))
        })
        .ok_or("thread/list omitted the captured thread")?;
    let mut safe = safe_thread_exemplar(&json!({"thread": thread}))?;
    safe["thread"]
        .as_object_mut()
        .ok_or("safe thread/list exemplar was not an object")?
        .remove("preview");
    Ok(json!({
        "data": [safe["thread"].clone()],
        "nextCursor": value.get("nextCursor").cloned().unwrap_or(Value::Null)
    }))
}

fn safe_turn_exemplar(value: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let turn = value
        .get("turn")
        .and_then(Value::as_object)
        .ok_or("turn exemplar lacks a turn object")?;
    let mut safe = serde_json::Map::new();
    for key in [
        "id",
        "status",
        "itemsView",
        "startedAt",
        "completedAt",
        "durationMs",
        "error",
    ] {
        if let Some(value) = turn.get(key) {
            safe.insert(key.to_owned(), value.clone());
        }
    }
    safe.insert("items".to_owned(), json!([]));
    Ok(json!({"turn": safe}))
}

async fn start_fixture_turn(
    connection: &mut AppServerConnection,
    thread_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let result: Value = connection
        .request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{"type": "text", "text": "FIXTURE_PROMPT_SENTINEL"}]
            }),
        )
        .await
        .map_err(fixture_tool_error)?;
    Ok(result["turn"]["id"]
        .as_str()
        .ok_or("turn/start did not return an id")?
        .to_owned())
}

async fn wait_for_turn_status(
    connection: &mut AppServerConnection,
    thread_id: &str,
    turn_id: &str,
    expected_status: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let turns: Value = connection
                .request(
                    "thread/turns/list",
                    json!({
                        "threadId": thread_id,
                        "itemsView": "notLoaded",
                        "sortDirection": "desc",
                        "limit": 10
                    }),
                )
                .await
                .map_err(fixture_tool_error)?;
            if turns["data"].as_array().is_some_and(|items| {
                items.iter().any(|turn| {
                    turn["id"].as_str() == Some(turn_id)
                        && turn["status"].as_str() == Some(expected_status)
                })
            }) {
                return Ok::<Value, io::Error>(turns);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| format!("turn {turn_id} did not reach {expected_status}"))?
    .map_err(Into::into)
}

async fn wait_for_thread_list(
    connection: &mut AppServerConnection,
    thread_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let threads: Value = connection
                .request(
                    "thread/list",
                    json!({
                        "limit": 10,
                        "sourceKinds": [
                            "cli",
                            "vscode",
                            "exec",
                            "appServer",
                            "subAgent",
                            "subAgentReview",
                            "subAgentCompact",
                            "subAgentThreadSpawn",
                            "subAgentOther",
                            "unknown"
                        ]
                    }),
                )
                .await
                .map_err(fixture_tool_error)?;
            if threads["data"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|thread| thread["id"].as_str() == Some(thread_id))
            }) {
                return Ok::<Value, io::Error>(threads);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| format!("thread/list did not converge for {thread_id}"))?
    .map_err(Into::into)
}

async fn wait_for_responses_requests(
    endpoint: &ResponsesEndpoint,
    expected: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while endpoint.requests.load(Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| format!("Responses endpoint did not receive request {expected}"))?;
    Ok(())
}

async fn capture_native_error(
    connection: &mut AppServerConnection,
    method: &'static str,
    params: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let request_id = connection.next_request_id;
    connection.next_request_id += 1;
    connection
        .websocket
        .send(Message::text(
            json!({"id": request_id, "method": method, "params": params}).to_string(),
        ))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = connection
                .websocket
                .next()
                .await
                .ok_or_else(|| io::Error::other("app-server disconnected"))?
                .map_err(io::Error::other)?;
            let Message::Text(text) = frame else {
                continue;
            };
            let value: Value = serde_json::from_str(text.as_str())?;
            if value.get("id").and_then(Value::as_u64) != Some(request_id) {
                continue;
            }
            return value
                .get("error")
                .cloned()
                .ok_or_else(|| io::Error::other("native request unexpectedly succeeded"));
        }
    })
    .await
    .map_err(|_| format!("{method} error capture timed out"))?
    .map_err(Into::into)
}

fn normalize_fixture(
    value: Value,
    ids: &HashMap<String, &'static str>,
    temporary_path: &str,
) -> Value {
    fn normalize(value: Value, ids: &HashMap<String, &'static str>, temporary_path: &str) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .into_iter()
                    .map(|(key, value)| {
                        let value = if matches!(
                            key.as_str(),
                            "createdAt"
                                | "updatedAt"
                                | "startedAt"
                                | "completedAt"
                                | "durationMs"
                                | "sectionEnteredAt"
                        ) {
                            match value {
                                Value::Null => Value::Null,
                                _ => json!(0),
                            }
                        } else {
                            normalize(value, ids, temporary_path)
                        };
                        (key, value)
                    })
                    .collect(),
            ),
            Value::Array(values) => Value::Array(
                values
                    .into_iter()
                    .map(|value| normalize(value, ids, temporary_path))
                    .collect(),
            ),
            Value::String(mut string) => {
                for (actual, normalized) in ids {
                    string = string.replace(actual, normalized);
                }
                string = string.replace(temporary_path, "/tmp/codex-session-control-fixture");
                Value::String(string)
            }
            other => other,
        }
    }
    normalize(value, ids, temporary_path)
}

fn reject_sensitive_fixture(
    fixture: &Value,
    temporary_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    fn inspect_keys(value: &Value) -> Result<(), &'static str> {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    if matches!(
                        key.as_str(),
                        "accessToken"
                            | "refreshToken"
                            | "authorization"
                            | "apiKey"
                            | "credentials"
                            | "environment"
                    ) {
                        return Err("fixture contains a credential-bearing key");
                    }
                    inspect_keys(value)?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    inspect_keys(value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    inspect_keys(fixture)?;
    let rendered = serde_json::to_string(fixture)?;
    for sentinel in [
        "FIXTURE_PROMPT_SENTINEL",
        "FIXTURE_CREDENTIAL_SENTINEL",
        temporary_path,
    ] {
        if rendered.contains(sentinel) {
            return Err(format!("fixture contains forbidden sentinel: {sentinel}").into());
        }
    }
    Ok(())
}
