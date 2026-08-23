use super::*;

use std::{any::Any, panic::AssertUnwindSafe};

use futures_util::FutureExt;

pub(super) fn arguments(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

pub(super) fn meta(thread_id: &str) -> Value {
    json!({"threadId": thread_id})
}

pub(super) fn assert_category(error: ToolErrorData, category: ToolErrorCategory) {
    assert_eq!(error.category, category, "{error:?}");
}

#[derive(Clone, Debug)]
pub(super) enum FakeResponse {
    Result(Value),
    Error(Value),
    Pending,
    Disconnect,
    Controlled {
        response: Box<FakeResponse>,
        release: Option<Arc<tokio::sync::Notify>>,
        sent: Option<Arc<tokio::sync::Notify>>,
    },
}

#[derive(Clone, Debug)]
pub(super) struct FakeStep {
    pub(super) method: &'static str,
    pub(super) params: Value,
    pub(super) response: FakeResponse,
    pub(super) notify_after: bool,
    pub(super) delay: Duration,
}

impl FakeStep {
    pub(super) fn result(method: &'static str, params: Value, result: Value) -> Self {
        Self {
            method,
            params,
            response: FakeResponse::Result(result),
            notify_after: false,
            delay: Duration::ZERO,
        }
    }

    pub(super) fn error(method: &'static str, params: Value, error: Value) -> Self {
        Self {
            method,
            params,
            response: FakeResponse::Error(error),
            notify_after: false,
            delay: Duration::ZERO,
        }
    }

    pub(super) fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub(super) fn controlled(
        mut self,
        release: Option<Arc<tokio::sync::Notify>>,
        sent: Option<Arc<tokio::sync::Notify>>,
    ) -> Self {
        self.response = FakeResponse::Controlled {
            response: Box::new(self.response),
            release,
            sent,
        };
        self
    }
}

pub(super) struct FakeAppServer {
    pub(super) _root: TempDir,
    pub(super) config: ProductConfig,
    pub(super) log: Arc<Mutex<Vec<Value>>>,
    pub(super) connections: Arc<AtomicUsize>,
    request_received: Arc<tokio::sync::Notify>,
    failures: Arc<Mutex<Vec<String>>>,
    pub(super) task: tokio::task::JoinHandle<()>,
}

impl FakeAppServer {
    pub(super) async fn start(steps: Vec<FakeStep>) -> Self {
        Self::start_connections(vec![steps]).await
    }

    pub(super) async fn start_connections(scripts: Vec<Vec<FakeStep>>) -> Self {
        Self::start_with_scripts(scripts, false).await
    }

    pub(super) async fn start_with_initialization_disconnect(root_steps: Vec<FakeStep>) -> Self {
        Self::start_with_scripts(vec![root_steps], true).await
    }

    async fn start_with_scripts(scripts: Vec<Vec<FakeStep>>, disconnect_after_root: bool) -> Self {
        assert!(
            matches!(
                tokio::runtime::Handle::current().runtime_flavor(),
                tokio::runtime::RuntimeFlavor::CurrentThread
            ),
            "FakeAppServer requires a current-thread Tokio runtime so background failures are recorded before client tasks resume"
        );
        let root = crate::test_support::private_tempdir();
        let socket_parent = root.path().join("socket");
        let codex_home = root.path().join(".codex");
        fs::create_dir(&socket_parent).unwrap();
        fs::create_dir(&codex_home).unwrap();
        fs::set_permissions(&socket_parent, fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = socket_parent.join("app-server.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).unwrap();

        let config = ProductConfig {
            schema_version: 2,
            codex_executable: "/usr/bin/codex".into(),
            codex_home: codex_home.clone(),
            socket_path,
        };
        let log = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let request_received = Arc::new(tokio::sync::Notify::new());
        let failures = Arc::new(Mutex::new(Vec::new()));
        let task_log = Arc::clone(&log);
        let task_connections = Arc::clone(&connections);
        let task_request_received = Arc::clone(&request_received);
        let task_failures = Arc::clone(&failures);
        let task = tokio::spawn(async move {
            let server_failures = Arc::clone(&task_failures);
            let result = AssertUnwindSafe(async move {
                let mut handlers = tokio::task::JoinSet::new();
                let mut scripts = scripts;
                let normal_connection_count = scripts.len();
                for connection_index in
                    0..(normal_connection_count + usize::from(disconnect_after_root))
                {
                    let (stream, _) = listener.accept().await.unwrap();
                    task_connections.fetch_add(1, Ordering::SeqCst);
                    let mut websocket = accept_async(stream).await.unwrap();
                    let initialize = next_text(&mut websocket).await;
                    assert_eq!(initialize["method"], "initialize");
                    if disconnect_after_root && connection_index == normal_connection_count {
                        let _ = websocket.close(None).await;
                        continue;
                    }

                    let initialize_id = initialize["id"].clone();
                    websocket
                        .send(Message::text(
                            json!({
                                "id": initialize_id,
                                "result": {
                                    "codexHome": codex_home,
                                    "userAgent": TESTED_CODEX_CLI_VERSION
                                }
                            })
                            .to_string(),
                        ))
                        .await
                        .unwrap();
                    let initialized = next_text(&mut websocket).await;
                    assert_eq!(initialized, json!({"method": "initialized"}));
                    let first_request = next_text(&mut websocket).await;
                    let matching_indices = scripts
                        .iter()
                        .enumerate()
                        .filter_map(|(index, steps)| {
                            let first_step =
                                steps.first().expect("connection script must not be empty");
                            (first_step.method == first_request["method"]
                                && first_step.params == first_request["params"])
                                .then_some(index)
                        })
                        .collect::<Vec<_>>();
                    let script_index = match matching_indices.as_slice() {
                        [index] => *index,
                        [] => panic!("no connection script matches {first_request}"),
                        _ => panic!("multiple connection scripts match {first_request}"),
                    };
                    let steps = scripts.swap_remove(script_index);
                    let handler_failures = Arc::clone(&server_failures);
                    let handler_log = Arc::clone(&task_log);
                    let handler_request_received = Arc::clone(&task_request_received);
                    let _ = handlers.spawn(async move {
                        if let Err(payload) = AssertUnwindSafe(handle_connection(
                            websocket,
                            first_request,
                            steps,
                            handler_log,
                            handler_request_received,
                        ))
                        .catch_unwind()
                        .await
                        {
                            record_failure(&handler_failures, "connection handler", payload);
                        }
                    });
                }
                while let Some(result) = handlers.join_next().await {
                    result.unwrap();
                }
            })
            .catch_unwind()
            .await;
            if let Err(payload) = result {
                record_failure(&task_failures, "server", payload);
            }
        });
        Self {
            _root: root,
            config,
            log,
            connections,
            request_received,
            failures,
            task,
        }
    }

    pub(super) fn log(&self) -> Vec<Value> {
        self.log.lock().unwrap().clone()
    }

    pub(super) fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub(super) async fn wait_for_requests(&self, count: usize) {
        loop {
            let request_received = self.request_received.notified();
            if self.log.lock().unwrap().len() >= count {
                return;
            }
            request_received.await;
        }
    }

    pub(super) async fn wait_for_requests_matching(&self, expected: &[(&str, Value)]) {
        loop {
            let request_received = self.request_received.notified();
            let matched = {
                let log = self.log.lock().unwrap();
                expected.iter().all(|(method, params)| {
                    log.iter()
                        .any(|request| request["method"] == *method && request["params"] == *params)
                })
            };
            if matched {
                return;
            }
            request_received.await;
        }
    }
}

impl Drop for FakeAppServer {
    fn drop(&mut self) {
        let failures = self.failures.lock().unwrap().clone();
        self.task.abort();
        if !std::thread::panicking() && !failures.is_empty() {
            panic!("FakeAppServer background failure:\n{}", failures.join("\n"));
        }
    }
}

fn record_failure(failures: &Arc<Mutex<Vec<String>>>, source: &str, payload: Box<dyn Any + Send>) {
    failures
        .lock()
        .unwrap()
        .push(format!("{source}: {}", panic_payload_text(payload)));
}

fn panic_payload_text(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

async fn next_text(
    websocket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Value {
    let message = websocket.next().await.unwrap().unwrap();
    let Message::Text(text) = message else {
        panic!("expected text message")
    };
    serde_json::from_str(&text).unwrap()
}

async fn handle_connection(
    mut websocket: tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    first_request: Value,
    steps: Vec<FakeStep>,
    log: Arc<Mutex<Vec<Value>>>,
    request_received: Arc<tokio::sync::Notify>,
) {
    let mut disconnected = false;
    let mut first_request = Some(first_request);
    for step in steps {
        let FakeStep {
            method,
            params,
            response,
            notify_after,
            delay,
        } = step;
        let request = match first_request.take() {
            Some(request) => request,
            None => next_text(&mut websocket).await,
        };
        assert_eq!(request["method"], method, "{request}");
        assert_eq!(request["params"], params, "{request}");
        log.lock().unwrap().push(json!({
            "method": method,
            "params": request["params"].clone(),
        }));
        request_received.notify_one();

        let (response, release, sent) = match response {
            FakeResponse::Controlled {
                response,
                release,
                sent,
            } => {
                if matches!(response.as_ref(), FakeResponse::Controlled { .. }) {
                    panic!("nested controlled fake responses are unsupported");
                }
                (*response, release, sent)
            }
            response => (response, None, None),
        };
        if let Some(release) = release {
            release.notified().await;
        }

        tokio::time::sleep(delay).await;
        let id = request["id"].clone();
        let disconnect = match response {
            FakeResponse::Result(result) => {
                websocket
                    .send(Message::text(
                        json!({"id": id, "result": result}).to_string(),
                    ))
                    .await
                    .unwrap();
                false
            }
            FakeResponse::Error(error) => {
                websocket
                    .send(Message::text(json!({"id": id, "error": error}).to_string()))
                    .await
                    .unwrap();
                false
            }
            FakeResponse::Pending => false,
            FakeResponse::Disconnect => true,
            FakeResponse::Controlled { .. } => {
                panic!("nested controlled fake responses are unsupported");
            }
        };
        if !disconnect && notify_after {
            websocket
                .send(Message::text(
                    json!({"method": "thread/status/changed", "params": {}}).to_string(),
                ))
                .await
                .unwrap();
        }
        if let Some(sent) = sent {
            sent.notify_one();
        }
        if disconnect {
            disconnected = true;
            break;
        }
    }
    if !disconnected {
        std::future::pending::<()>().await;
    }
    let _ = websocket.close(None).await;
}

pub(super) fn native_thread(id: &str, status: Value, updated_at: i64) -> Value {
    json!({
        "id": id,
        "name": null,
        "preview": "safe preview",
        "cwd": "/workspace",
        "status": status,
        "createdAt": 10,
        "updatedAt": updated_at,
        "forkedFromId": null,
    })
}

pub(super) fn native_turn(id: &str, status: &str) -> Value {
    json!({
        "id": id,
        "status": status,
        "items": [],
        "itemsView": "notLoaded",
        "startedAt": null,
        "completedAt": null,
        "durationMs": null,
        "error": null,
    })
}

pub(super) fn native_goal(thread_id: &str, status: &str) -> Value {
    json!({
        "threadId": thread_id,
        "objective": "objective",
        "status": status,
        "tokenBudget": null,
        "tokensUsed": 2,
        "timeUsedSeconds": 3,
        "createdAt": 4,
        "updatedAt": 5,
    })
}

pub(super) fn snapshot_steps(
    thread_id: &'static str,
    status: Value,
    updated_at: i64,
    latest_turn: Option<Value>,
    notify_after: bool,
) -> Vec<FakeStep> {
    let mut latest = FakeStep::result(
        "thread/turns/list",
        json!({
            "threadId": thread_id,
            "limit": 1,
            "itemsView": "notLoaded",
        }),
        json!({"data": latest_turn.into_iter().collect::<Vec<_>>(), "nextCursor": null}),
    );
    latest.notify_after = notify_after;
    vec![
        FakeStep::result(
            "thread/read",
            json!({"threadId": thread_id, "includeTurns": false}),
            json!({"thread": native_thread(thread_id, status, updated_at)}),
        ),
        latest,
    ]
}
