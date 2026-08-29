use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::PermissionsExt,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use assert_cmd::cargo::cargo_bin;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::{
    Message, WebSocket, accept_hdr,
    handshake::server::{ErrorResponse, Request, Response},
    http::StatusCode,
};

const EXPLICIT_SOCKET: &str = "CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET";
const RUNTIME_DIR: &str = "XDG_RUNTIME_DIR";
const APP_ID: &str = "CODEX_LINUX_APP_ID";

struct NativeServer {
    upgrade_target: Receiver<String>,
    completion: Receiver<Result<(), String>>,
}

struct ToolRun {
    response: Value,
    stderr: String,
}

fn private_directory(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[allow(
    clippy::result_large_err,
    reason = "tungstenite requires this callback result type"
)]
fn start_native_server(socket_path: &Path) -> NativeServer {
    let listener = UnixListener::bind(socket_path).unwrap();
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600)).unwrap();
    let (upgrade_sender, upgrade_target) = mpsc::channel();
    let (completion_sender, completion) = mpsc::channel();

    thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let (stream, _) = listener.accept().map_err(|error| error.to_string())?;
            let mut websocket = accept_hdr(stream, |request: &Request, response: Response| {
                let target = request
                    .uri()
                    .path_and_query()
                    .map(|value| value.as_str())
                    .unwrap_or_default()
                    .to_owned();
                upgrade_sender.send(target.clone()).unwrap();
                if target == "/rpc" {
                    Ok(response)
                } else {
                    let mut error = ErrorResponse::new(Some("expected /rpc".to_owned()));
                    *error.status_mut() = StatusCode::NOT_FOUND;
                    Err(error)
                }
            })
            .map_err(|error| error.to_string())?;

            let initialize = next_native_text(&mut websocket)?;
            if initialize["method"] != "initialize" {
                return Err(format!("expected initialize, received {initialize}"));
            }
            websocket
                .send(Message::text(
                    json!({
                        "id": initialize["id"],
                        "result": {
                            "codexHome": "/tmp/desktop-owned-codex-home",
                            "userAgent": "codex-cli 0.150.0-alpha.12.2",
                        }
                    })
                    .to_string(),
                ))
                .map_err(|error| error.to_string())?;

            let initialized = next_native_text(&mut websocket)?;
            if initialized != json!({"method": "initialized"}) {
                return Err(format!("expected initialized, received {initialized}"));
            }
            let request = next_native_text(&mut websocket)?;
            if request["method"] != "thread/list" || request["params"] != json!({}) {
                return Err(format!("expected threads list, received {request}"));
            }
            websocket
                .send(Message::text(
                    json!({"id": request["id"], "result": {"data": [], "nextCursor": null}})
                        .to_string(),
                ))
                .map_err(|error| error.to_string())?;
            Ok(())
        })();
        let _ = completion_sender.send(result);
    });

    NativeServer {
        upgrade_target,
        completion,
    }
}

fn next_native_text(websocket: &mut WebSocket<UnixStream>) -> Result<Value, String> {
    match websocket.read().map_err(|error| error.to_string())? {
        Message::Text(text) => serde_json::from_str(&text).map_err(|error| error.to_string()),
        message => Err(format!("expected native text frame, received {message:?}")),
    }
}

fn send_json(writer: &mut impl Write, value: Value) {
    serde_json::to_writer(&mut *writer, &value).unwrap();
    writer.write_all(b"\n").unwrap();
    writer.flush().unwrap();
}

fn read_response_for_id(reader: &mut impl BufRead, expected_id: u64) -> Value {
    let mut transcript = String::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).unwrap();
        if read == 0 {
            panic!("MCP process closed stdout before response {expected_id}: {transcript}");
        }
        transcript.push_str(&line);
        let message: Value = serde_json::from_str(line.trim()).unwrap();
        if message["id"] == json!(expected_id) {
            return message;
        }
    }
}

fn run_threads_list(environment: &[(&str, &Path)]) -> ToolRun {
    let mut command = Command::new(cargo_bin("codex-session-control"));
    command
        .env_remove(EXPLICIT_SOCKET)
        .env_remove(RUNTIME_DIR)
        .env_remove(APP_ID)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in environment {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    send_json(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "desktop-socket-contract", "version": "1.0"},
            }
        }),
    );
    read_response_for_id(&mut reader, 1);
    send_json(
        &mut stdin,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    send_json(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "threads_list", "arguments": {}},
        }),
    );
    let response = read_response_for_id(&mut reader, 2);

    drop(reader);
    drop(stdin);
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "stderr: {stderr}");
    ToolRun { response, stderr }
}

fn assert_successful_threads_list(run: &ToolRun) {
    assert_eq!(run.response["id"], json!(2));
    assert_ne!(
        run.response["result"]["isError"],
        json!(true),
        "{:#}",
        run.response
    );
}

#[test]
fn explicit_socket_precedes_the_derived_desktop_socket() {
    let root = tempfile::tempdir().unwrap();
    let explicit_parent = root.path().join("explicit");
    private_directory(&explicit_parent);
    let explicit_socket = explicit_parent.join("app-server.sock");
    let server = start_native_server(&explicit_socket);

    let unused_runtime = root.path().join("derived-must-not-be-read");
    let run = run_threads_list(&[
        (EXPLICIT_SOCKET, explicit_socket.as_path()),
        (RUNTIME_DIR, unused_runtime.as_path()),
    ]);

    assert_successful_threads_list(&run);
    assert_eq!(
        server
            .upgrade_target
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        "/rpc"
    );
    server
        .completion
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
}

#[test]
fn derived_desktop_socket_is_used_without_an_explicit_override() {
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("runtime");
    let app = runtime.join("codex-desktop");
    let bridge = app.join("app-server-bridge");
    private_directory(&runtime);
    private_directory(&app);
    private_directory(&bridge);
    let socket = bridge.join("app-server.sock");
    let server = start_native_server(&socket);

    let run = run_threads_list(&[(RUNTIME_DIR, runtime.as_path())]);

    assert_successful_threads_list(&run);
    assert_eq!(
        server
            .upgrade_target
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        "/rpc"
    );
    server
        .completion
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
}

#[test]
fn endpoint_selection_errors_redact_environment_values() {
    let root: TempDir = tempfile::tempdir().unwrap();
    let explicit = root
        .path()
        .join("DO_NOT_LEAK_EXPLICIT")
        .join("..")
        .join("app-server.sock");
    let runtime = root.path().join("DO_NOT_LEAK_RUNTIME");

    let run = run_threads_list(&[
        (EXPLICIT_SOCKET, explicit.as_path()),
        (RUNTIME_DIR, runtime.as_path()),
    ]);

    assert_eq!(run.response["id"], json!(2));
    assert!(run.response.get("error").is_some(), "{:#}", run.response);
    let public_output = format!("{}{}", run.response, run.stderr);
    assert!(!public_output.contains("DO_NOT_LEAK_EXPLICIT"));
    assert!(!public_output.contains("DO_NOT_LEAK_RUNTIME"));
}
