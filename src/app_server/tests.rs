use std::{
    collections::HashMap,
    future::pending,
    io,
    os::unix::fs::{PermissionsExt, symlink},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UnixListener},
    process::{Child, Command},
    sync::{Mutex, Notify},
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        http::StatusCode,
    },
};

use super::protocol::ProtocolFixture;
use super::*;
use crate::error::ToolErrorCategory;
#[derive(Clone, Debug)]
enum ServerFrame {
    Notification(serde_json::Value),
    Request(serde_json::Value),
    Response(serde_json::Value),
    Disconnect,
}

#[derive(Clone, Copy, Debug)]
enum TransportBarrier {
    BeforeInitializeResponse,
    BeforeNativeResponse,
}

#[derive(Clone, Copy, Debug)]
enum SocketViolation {
    ParentMode,
    SocketMode,
    SocketSymlink,
    NotSocket,
}

#[derive(Clone, Debug)]
struct FakeScript {
    codex_version: String,
    initialize_codex_home: Option<Option<String>>,
    native_frames: Vec<ServerFrame>,
    barrier: Option<TransportBarrier>,
    failure_point: FailurePoint,
}

#[derive(Clone)]
struct FakeServerState {
    codex_home: PathBuf,
    frames: Arc<Mutex<Vec<Value>>>,
    upgrade_targets: Arc<StdMutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
    barrier: Arc<Notify>,
    closed: Arc<Notify>,
    frame_recorded: Arc<Notify>,
    native_state_changed: Arc<AtomicBool>,
}

impl FakeScript {
    fn happy() -> Self {
        Self {
            codex_version: TESTED_CODEX_VERSION.to_owned(),
            initialize_codex_home: None,
            native_frames: Vec::new(),
            barrier: None,
            failure_point: FailurePoint::Never,
        }
    }

    fn with_native_frames(mut self, frames: Vec<ServerFrame>) -> Self {
        self.native_frames = frames;
        self
    }

    fn with_codex_version(mut self, version: &str) -> Self {
        self.codex_version = version.to_owned();
        self
    }

    fn with_initialize_codex_home(mut self, codex_home: Option<&str>) -> Self {
        self.initialize_codex_home = Some(codex_home.map(ToOwned::to_owned));
        self
    }

    fn blocked_at(barrier: TransportBarrier) -> Self {
        Self {
            barrier: Some(barrier),
            ..Self::happy()
        }
    }

    fn at_failure(failure_point: FailurePoint) -> Self {
        let native_frames = if matches!(
            failure_point,
            FailurePoint::AfterFullWrite
                | FailurePoint::BeforeResponse
                | FailurePoint::AfterNativeStateChange
        ) {
            vec![ServerFrame::Disconnect]
        } else {
            Vec::new()
        };
        Self {
            native_frames,
            failure_point,
            ..Self::happy()
        }
    }
}

struct FakeAppServer {
    _temporary: TempDir,
    _listener_guard: Option<UnixListener>,
    listener_task: Option<tokio::task::JoinHandle<()>>,
    socket_path: PathBuf,
    codex_home: PathBuf,
    frames: Arc<Mutex<Vec<Value>>>,
    upgrade_targets: Arc<StdMutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
    barrier: Arc<Notify>,
    closed: Arc<Notify>,
    frame_recorded: Arc<Notify>,
    failure_point: FailurePoint,
    native_state_changed: Arc<AtomicBool>,
}

impl FakeAppServer {
    async fn start(script: FakeScript) -> Self {
        let temporary = crate::test_support::private_tempdir();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = temporary.path().join("app-server.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let codex_home = temporary.path().join("codex-home");
        let frames = Arc::new(Mutex::new(Vec::new()));
        let upgrade_targets = Arc::new(StdMutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Notify::new());
        let closed = Arc::new(Notify::new());
        let frame_recorded = Arc::new(Notify::new());
        let failure_point = script.failure_point;
        let native_state_changed = Arc::new(AtomicBool::new(false));

        let state = FakeServerState {
            codex_home: codex_home.clone(),
            frames: Arc::clone(&frames),
            upgrade_targets: Arc::clone(&upgrade_targets),
            connections: Arc::clone(&connections),
            barrier: Arc::clone(&barrier),
            closed: Arc::clone(&closed),
            frame_recorded: Arc::clone(&frame_recorded),
            native_state_changed: Arc::clone(&native_state_changed),
        };

        let listener_task = tokio::spawn(serve_fake(listener, script, state));

        Self {
            _temporary: temporary,
            _listener_guard: None,
            listener_task: Some(listener_task),
            socket_path,
            codex_home,
            frames,
            upgrade_targets,
            connections,
            barrier,
            closed,
            frame_recorded,
            failure_point,
            native_state_changed,
        }
    }

    async fn with_socket_violation(violation: SocketViolation) -> Self {
        let temporary = crate::test_support::private_tempdir();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = temporary.path().join("app-server.sock");
        let mut listener_guard = None;

        match violation {
            SocketViolation::ParentMode => {
                std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o755))
                    .unwrap();
                let listener = UnixListener::bind(&socket_path).unwrap();
                std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
                    .unwrap();
                listener_guard = Some(listener);
            }
            SocketViolation::SocketMode => {
                let listener = UnixListener::bind(&socket_path).unwrap();
                std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))
                    .unwrap();
                listener_guard = Some(listener);
            }
            SocketViolation::SocketSymlink => {
                let target = temporary.path().join("actual.sock");
                let listener = UnixListener::bind(&target).unwrap();
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
                symlink(&target, &socket_path).unwrap();
                listener_guard = Some(listener);
            }
            SocketViolation::NotSocket => std::fs::write(&socket_path, b"not a socket").unwrap(),
        }

        Self {
            codex_home: temporary.path().join("codex-home"),
            _temporary: temporary,
            _listener_guard: listener_guard,
            listener_task: None,
            socket_path,
            frames: Arc::new(Mutex::new(Vec::new())),
            upgrade_targets: Arc::new(StdMutex::new(Vec::new())),
            connections: Arc::new(AtomicUsize::new(0)),
            barrier: Arc::new(Notify::new()),
            closed: Arc::new(Notify::new()),
            frame_recorded: Arc::new(Notify::new()),
            failure_point: FailurePoint::Never,
            native_state_changed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn client(&self, tested_version: &str) -> AppServerClient {
        let mut client = AppServerClient::for_test(self.socket_path.clone(), tested_version);
        client.failure_point = self.failure_point;
        client
    }

    fn codex_home(&self) -> &Path {
        &self.codex_home
    }

    async fn recorded_frames(&self) -> Vec<Value> {
        self.frames.lock().await.clone()
    }

    fn recorded_upgrade_targets(&self) -> Vec<String> {
        self.upgrade_targets.lock().unwrap().clone()
    }

    async fn wait_for_frames(&self, count: usize) {
        loop {
            let notified = self.frame_recorded.notified();
            if self.frames.lock().await.len() >= count {
                return;
            }
            notified.await;
        }
    }

    fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn native_state_changed(&self) -> bool {
        self.native_state_changed.load(Ordering::SeqCst)
    }

    async fn wait_until_barrier(&self) {
        self.barrier.notified().await;
    }

    async fn wait_for_close(&self) {
        self.closed.notified().await;
    }

    async fn replace_socket(&mut self, script: FakeScript) {
        let listener_task = self
            .listener_task
            .take()
            .expect("replacement harness owns its listener task");
        listener_task.abort();
        let join_error = listener_task.await.unwrap_err();
        assert!(join_error.is_cancelled());
        std::fs::remove_file(&self.socket_path).unwrap();

        let listener = UnixListener::bind(&self.socket_path).unwrap();
        std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o600))
            .unwrap();
        let state = FakeServerState {
            codex_home: self.codex_home.clone(),
            frames: Arc::clone(&self.frames),
            upgrade_targets: Arc::clone(&self.upgrade_targets),
            connections: Arc::clone(&self.connections),
            barrier: Arc::clone(&self.barrier),
            closed: Arc::clone(&self.closed),
            frame_recorded: Arc::clone(&self.frame_recorded),
            native_state_changed: Arc::clone(&self.native_state_changed),
        };
        self.listener_task = Some(tokio::spawn(serve_fake(listener, script, state)));
    }
}

async fn serve_fake(listener: UnixListener, script: FakeScript, state: FakeServerState) {
    let FakeServerState {
        codex_home,
        frames,
        upgrade_targets,
        connections,
        barrier,
        closed,
        frame_recorded,
        native_state_changed,
    } = state;

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        connections.fetch_add(1, Ordering::SeqCst);
        let script = script.clone();
        let codex_home = codex_home.clone();
        let frames = Arc::clone(&frames);
        let upgrade_targets = Arc::clone(&upgrade_targets);
        let barrier = Arc::clone(&barrier);
        let closed = Arc::clone(&closed);
        let frame_recorded = Arc::clone(&frame_recorded);
        let native_state_changed = Arc::clone(&native_state_changed);
        tokio::spawn(async move {
            let Ok(mut websocket) =
                accept_hdr_async(stream, move |request: &Request, response: Response| {
                    let target = request
                        .uri()
                        .path_and_query()
                        .map(|value| value.as_str())
                        .unwrap_or_default()
                        .to_owned();
                    upgrade_targets.lock().unwrap().push(target.clone());
                    if target == "/rpc" {
                        Ok(response)
                    } else {
                        let mut error = ErrorResponse::new(Some("expected /rpc".to_owned()));
                        *error.status_mut() = StatusCode::NOT_FOUND;
                        Err(error)
                    }
                })
                .await
            else {
                return;
            };
            let Some(Ok(Message::Text(initialize))) = websocket.next().await else {
                return;
            };
            let initialize: Value = serde_json::from_str(initialize.as_str()).unwrap();
            frames.lock().await.push(initialize.clone());
            frame_recorded.notify_one();

            if matches!(
                script.barrier,
                Some(TransportBarrier::BeforeInitializeResponse)
            ) {
                barrier.notify_one();
                pending::<()>().await;
            }

            let mut initialize_response = json!({
                "id": initialize["id"],
                "result": {
                    "codexHome": codex_home,
                    "platformFamily": "unix",
                    "platformOs": "linux",
                    "userAgent": format!("codex-cli {}", script.codex_version),
                }
            });
            match &script.initialize_codex_home {
                Some(Some(codex_home)) => {
                    initialize_response["result"]["codexHome"] = json!(codex_home);
                }
                Some(None) => {
                    initialize_response["result"]
                        .as_object_mut()
                        .unwrap()
                        .remove("codexHome");
                }
                None => {}
            }
            websocket
                .send(Message::text(initialize_response.to_string()))
                .await
                .unwrap();
            let Some(Ok(Message::Text(initialized))) = websocket.next().await else {
                return;
            };
            frames
                .lock()
                .await
                .push(serde_json::from_str(initialized.as_str()).unwrap());
            frame_recorded.notify_one();

            while let Some(message) = websocket.next().await {
                match message {
                    Ok(Message::Text(request)) => {
                        let request: Value = serde_json::from_str(request.as_str()).unwrap();
                        frames.lock().await.push(request.clone());
                        frame_recorded.notify_one();
                        if script.failure_point == FailurePoint::AfterNativeStateChange {
                            native_state_changed.store(true, Ordering::SeqCst);
                        }
                        if matches!(script.barrier, Some(TransportBarrier::BeforeNativeResponse)) {
                            barrier.notify_one();
                            pending::<()>().await;
                        }
                        let native_frames = if script.native_frames.is_empty() {
                            vec![ServerFrame::Response(default_native_response(&request))]
                        } else {
                            script.native_frames.clone()
                        };
                        for frame in native_frames {
                            let send_result = match frame {
                                ServerFrame::Notification(value)
                                | ServerFrame::Request(value)
                                | ServerFrame::Response(value) => {
                                    websocket.send(Message::text(value.to_string())).await
                                }
                                ServerFrame::Disconnect => {
                                    let _ = websocket.close(None).await;
                                    closed.notify_one();
                                    return;
                                }
                            };
                            if send_result.is_err() {
                                closed.notify_one();
                                return;
                            }
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
            closed.notify_one();
        });
    }
}

fn default_native_response(request: &Value) -> Value {
    let result = match request.get("method").and_then(Value::as_str) {
        Some("thread/list") => json!({"data": []}),
        Some("thread/read") => json!({
            "thread": {
                "id": "thread-1",
                "name": null,
                "status": {"type": "notLoaded"},
                "updatedAt": 0,
            }
        }),
        Some("thread/turns/list") => json!({"data": [], "nextCursor": null}),
        _ => json!({}),
    };
    json!({"id": request["id"], "result": result})
}

fn initialize_frame(id: u64, _codex_home: &Path) -> Value {
    json!({
        "id": id,
        "method": "initialize",
        "params": {
            "clientInfo": {
                        "name": "codex_session_control",
                        "title": "Codex Session Control",
                        "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {
                "experimentalApi": true,
                "mcpServerOpenaiFormElicitation": false,
                "requestAttestation": false,
                "optOutNotificationMethods": [],
            }
        }
    })
}

async fn run_to_barrier(
    client: AppServerClient,
    barrier: TransportBarrier,
) -> Result<(), ToolErrorData> {
    let mut connection = client.connect_initialized().await?;
    if matches!(barrier, TransportBarrier::BeforeNativeResponse) {
        let _: Value = connection
            .request("thread/read", json!({"threadId": "thread-1"}))
            .await?;
    }
    Ok(())
}

fn classify_fixture_error(
    method: &str,
    exemplar: &Value,
    fixture: &ProtocolFixture,
) -> ToolErrorCategory {
    classify_native_error(
        method,
        exemplar["code"].as_i64().unwrap(),
        exemplar["message"].as_str().unwrap(),
        exemplar.get("data"),
        fixture,
    )
}

mod compact_snapshot;
mod live_capture;
mod native_error;
mod transport;
