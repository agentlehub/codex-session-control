use super::*;

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

#[derive(Clone, Debug)]
pub(super) enum FakeInitialize {
    Success,
    Disconnect,
}

#[derive(Clone, Debug)]
pub(super) struct FakeConnectionScript {
    pub(super) initialize: FakeInitialize,
    pub(super) steps: Vec<FakeStep>,
}

impl FakeConnectionScript {
    pub(super) fn initialized(steps: Vec<FakeStep>) -> Self {
        Self {
            initialize: FakeInitialize::Success,
            steps,
        }
    }

    pub(super) fn disconnect_on_initialize() -> Self {
        Self {
            initialize: FakeInitialize::Disconnect,
            steps: Vec::new(),
        }
    }
}

pub(super) struct FakeAppServer {
    pub(super) _root: TempDir,
    pub(super) config: ProductConfig,
    pub(super) log: Arc<Mutex<Vec<Value>>>,
    pub(super) connections: Arc<AtomicUsize>,
    request_received: Arc<tokio::sync::Notify>,
    pub(super) task: tokio::task::JoinHandle<()>,
}

impl FakeAppServer {
    pub(super) async fn start(steps: Vec<FakeStep>) -> Self {
        Self::start_connections(vec![steps]).await
    }

    pub(super) async fn start_connections(scripts: Vec<Vec<FakeStep>>) -> Self {
        Self::start_scripted_connections(
            scripts
                .into_iter()
                .map(FakeConnectionScript::initialized)
                .collect(),
        )
        .await
    }

    pub(super) async fn start_scripted_connections(scripts: Vec<FakeConnectionScript>) -> Self {
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
        let task_log = Arc::clone(&log);
        let task_connections = Arc::clone(&connections);
        let task_request_received = Arc::clone(&request_received);
        let task = tokio::spawn(async move {
            let mut handlers = tokio::task::JoinSet::new();
            for script in scripts {
                let (stream, _) = listener.accept().await.unwrap();
                task_connections.fetch_add(1, Ordering::SeqCst);
                let mut websocket = accept_async(stream).await.unwrap();
                let initialize = next_text(&mut websocket).await;
                assert_eq!(initialize["method"], "initialize");
                match script.initialize {
                    FakeInitialize::Success => {
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
                    }
                    FakeInitialize::Disconnect => {
                        let _ = websocket.close(None).await;
                        continue;
                    }
                }
                let _ = handlers.spawn(handle_connection(
                    websocket,
                    script.steps,
                    Arc::clone(&task_log),
                    Arc::clone(&task_request_received),
                ));
            }
            while let Some(result) = handlers.join_next().await {
                result.unwrap();
            }
        });
        Self {
            _root: root,
            config,
            log,
            connections,
            request_received,
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
}

impl Drop for FakeAppServer {
    fn drop(&mut self) {
        self.task.abort();
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
    steps: Vec<FakeStep>,
    log: Arc<Mutex<Vec<Value>>>,
    request_received: Arc<tokio::sync::Notify>,
) {
    let mut disconnected = false;
    for step in steps {
        let FakeStep {
            method,
            params,
            response,
            notify_after,
            delay,
        } = step;
        let request = next_text(&mut websocket).await;
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

#[test]
fn controlled_response_preserves_the_wrapped_response() {
    let release = Arc::new(tokio::sync::Notify::new());
    let sent = Arc::new(tokio::sync::Notify::new());
    let step = FakeStep::result("thread/read", json!({}), json!({"thread": {}}))
        .controlled(Some(Arc::clone(&release)), Some(Arc::clone(&sent)));

    let FakeResponse::Controlled {
        response,
        release: actual_release,
        sent: actual_sent,
    } = step.response
    else {
        panic!("expected controlled response")
    };
    assert!(matches!(*response, FakeResponse::Result(_)));
    assert!(Arc::ptr_eq(actual_release.as_ref().unwrap(), &release));
    assert!(Arc::ptr_eq(actual_sent.as_ref().unwrap(), &sent));
}

#[test]
fn connection_scripts_describe_initialization_outcomes() {
    let initialized = FakeConnectionScript::initialized(Vec::new());
    assert!(matches!(initialized.initialize, FakeInitialize::Success));
    assert!(initialized.steps.is_empty());

    let disconnected = FakeConnectionScript::disconnect_on_initialize();
    assert!(matches!(
        disconnected.initialize,
        FakeInitialize::Disconnect
    ));
    assert!(disconnected.steps.is_empty());
}

#[tokio::test]
async fn scripted_connections_initialize_multiple_clients_concurrently() {
    let harness = FakeAppServer::start_scripted_connections(vec![
        FakeConnectionScript::initialized(Vec::new()),
        FakeConnectionScript::initialized(Vec::new()),
    ])
    .await;
    let first_client = AppServerClient::from_config(&harness.config);
    let second_client = AppServerClient::from_config(&harness.config);

    let (first, second) = tokio::join!(
        first_client.connect_initialized(),
        second_client.connect_initialized(),
    );

    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_eq!(harness.connection_count(), 2);
}

#[tokio::test]
async fn scripted_connection_can_disconnect_during_initialization() {
    let harness = FakeAppServer::start_scripted_connections(vec![
        FakeConnectionScript::disconnect_on_initialize(),
        FakeConnectionScript::initialized(Vec::new()),
    ])
    .await;
    let client = AppServerClient::from_config(&harness.config);

    assert!(client.connect_initialized().await.is_err());
    assert!(client.connect_initialized().await.is_ok());
    assert_eq!(harness.connection_count(), 2);
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
