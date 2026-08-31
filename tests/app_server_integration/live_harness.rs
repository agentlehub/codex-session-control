use std::{
    collections::BTreeSet,
    error::Error,
    ffi::OsString,
    fmt, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use assert_cmd::cargo::cargo_bin;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    net::UnixStream,
    process::{Child, ChildStdin, ChildStdout, Command},
};
use tokio_tungstenite::{WebSocketStream, client_async, tungstenite::Message};

use crate::cases::OwnedThreadId;

const EXPECTED_CODEX_VERSION: &str = env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION");
const ALL_THREAD_SOURCE_KINDS: [&str; 10] = [
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];
const THREAD_LIST_PAGE_SIZE: u32 = 100;
const MAX_WORKSPACE_PAGES: usize = 64;
const MAX_WORKSPACE_ROWS: usize = 6_400;
const TOOLS_CALL_METHOD: &str = "tools/call";
const UNKNOWN_ERROR_CATEGORY: &str = "unknown";
const SESSION_CONTROL_TOOLS: [&str; 13] = [
    "thread_create",
    "thread_fork",
    "threads_list",
    "thread_read",
    "threads_wait",
    "thread_message_send",
    "thread_title_set",
    "thread_goal_get",
    "thread_goal_set",
    "thread_goal_pause",
    "thread_goal_resume",
    "thread_goal_clear",
    "thread_interrupt",
];

pub(super) struct LiveHarness {
    workspace: PathBuf,
    socket: PathBuf,
    mcp: Option<McpClient>,
}

impl LiveHarness {
    pub(super) fn from_ledger(workspace: &Path) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            workspace: workspace.to_path_buf(),
            socket: desktop_socket_path()?,
            mcp: None,
        })
    }

    pub(super) fn for_test_native_socket(workspace: PathBuf, socket: PathBuf) -> io::Result<Self> {
        Ok(Self {
            workspace,
            socket,
            mcp: None,
        })
    }

    pub(super) async fn start_mcp(&mut self) -> Result<(), Box<dyn Error>> {
        self.mcp = Some(McpClient::start().await?);
        Ok(())
    }

    pub(super) fn mcp_mut(&mut self) -> Result<&mut McpClient, Box<dyn Error>> {
        self.mcp
            .as_mut()
            .ok_or_else(|| io::Error::other("MCP child is not running").into())
    }

    pub(super) async fn assert_exact_catalog(&mut self) -> Result<(), Box<dyn Error>> {
        let listed = self.mcp_mut()?.list_tools().await?;
        let names = listed
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| io::Error::other("tools/list omitted tools"))?
            .iter()
            .map(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| io::Error::other("tools/list emitted unnamed tool"))
            })
            .collect::<io::Result<Vec<_>>>()?;
        assert_eq!(
            names,
            SESSION_CONTROL_TOOLS
                .into_iter()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    pub(super) async fn assert_empty_workspace_before_mutation(
        &mut self,
    ) -> Result<(), Box<dyn Error>> {
        let workspace = self.workspace.clone();
        for archived in [false, true] {
            let listed = self.mcp_mut()?.list_threads(&workspace, archived).await?;
            let threads = listed
                .get("threads")
                .and_then(Value::as_array)
                .ok_or_else(|| io::Error::other("threads_list omitted threads"))?;
            if !threads.is_empty() {
                return Err(io::Error::other("unique live workspace was not empty").into());
            }
        }
        Ok(())
    }

    pub(super) async fn assert_supported_native_version(&self) -> Result<(), Box<dyn Error>> {
        let mut native = self.connect_native().await?;
        native.initialize().await
    }

    pub(super) async fn connect_native(&self) -> Result<NativeConnection, Box<dyn Error>> {
        NativeConnection::connect(&self.socket).await
    }

    pub(super) async fn workspace_thread_ids(
        &self,
        native: &mut NativeConnection,
        archived: bool,
    ) -> Result<Vec<String>, Box<dyn Error>> {
        let mut pages = WorkspacePages::new(&self.workspace)?;
        let mut cursor = None;
        loop {
            let page = native
                .request(
                    "thread/list",
                    exact_workspace_list_params(&self.workspace, archived, cursor.as_deref()),
                )
                .await?;
            let next = pages.add(&page)?;
            match next {
                Some(next) if cursor.as_deref() != Some(next.as_str()) => cursor = Some(next),
                Some(_) => return Err(io::Error::other("thread/list repeated a cursor").into()),
                None => return Ok(pages.into_ids()),
            }
        }
    }

    pub(super) async fn stop_and_reap_mcp_child(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(mut mcp) = self.mcp.take() else {
            return Ok(());
        };
        mcp.stdin.take();
        if tokio::time::timeout(Duration::from_secs(10), mcp.child.wait())
            .await
            .is_err()
        {
            mcp.child.start_kill()?;
            mcp.child.wait().await?;
        }
        Ok(())
    }
}

struct WorkspacePages<'a> {
    workspace: &'a str,
    ids: BTreeSet<String>,
    cursors: BTreeSet<String>,
    pages: usize,
    rows: usize,
}

impl<'a> WorkspacePages<'a> {
    fn new(workspace: &'a Path) -> io::Result<Self> {
        let workspace = workspace
            .to_str()
            .filter(|workspace| Path::new(workspace).is_absolute())
            .ok_or_else(|| io::Error::other("live workspace is not a UTF-8 absolute path"))?;
        Ok(Self {
            workspace,
            ids: BTreeSet::new(),
            cursors: BTreeSet::new(),
            pages: 0,
            rows: 0,
        })
    }

    fn add(&mut self, page: &Value) -> io::Result<Option<String>> {
        self.pages += 1;
        if self.pages > MAX_WORKSPACE_PAGES {
            return Err(io::Error::other(
                "thread/list exhausted the live page limit",
            ));
        }
        let data = page
            .get("data")
            .and_then(Value::as_array)
            .filter(|data| !data.is_empty())
            .ok_or_else(|| io::Error::other("thread/list emitted an empty or malformed page"))?;
        self.rows += data.len();
        if self.rows > MAX_WORKSPACE_ROWS {
            return Err(io::Error::other("thread/list exhausted the live row limit"));
        }
        for thread in data {
            let id = thread
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty() && id.len() <= 512)
                .filter(|id| !id.bytes().any(|byte| byte.is_ascii_control()))
                .ok_or_else(|| io::Error::other("thread/list emitted an invalid ID"))?;
            if thread.get("cwd").and_then(Value::as_str) != Some(self.workspace) {
                return Err(io::Error::other("thread/list returned a foreign workspace"));
            }
            if !self.ids.insert(id.to_owned()) {
                return Err(io::Error::other("thread/list returned a duplicate ID"));
            }
        }
        let next = page
            .get("nextCursor")
            .map(|cursor| {
                cursor
                    .as_str()
                    .filter(|cursor| !cursor.is_empty() && cursor.len() <= 512)
                    .filter(|cursor| !cursor.bytes().any(|byte| byte.is_ascii_control()))
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| io::Error::other("thread/list emitted an invalid cursor"))
            })
            .transpose()?;
        if let Some(cursor) = &next {
            if !self.cursors.insert(cursor.clone()) {
                return Err(io::Error::other("thread/list repeated a cursor"));
            }
        }
        Ok(next)
    }

    fn into_ids(self) -> Vec<String> {
        self.ids.into_iter().collect()
    }
}

pub(super) fn collect_workspace_page_ids(
    workspace: &Path,
    pages: &[Value],
) -> io::Result<Vec<String>> {
    let mut collected = WorkspacePages::new(workspace)?;
    for (index, page) in pages.iter().enumerate() {
        let next = collected.add(page)?;
        if next.is_none() {
            if index + 1 != pages.len() {
                return Err(io::Error::other(
                    "thread/list returned pages after completion",
                ));
            }
            return Ok(collected.into_ids());
        }
    }
    Err(io::Error::other("thread/list pagination was exhausted"))
}

pub(super) fn exact_workspace_list_params(
    workspace: &Path,
    archived: bool,
    cursor: Option<&str>,
) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("cwd".to_owned(), json!(workspace));
    params.insert("archived".to_owned(), json!(archived));
    params.insert("limit".to_owned(), json!(THREAD_LIST_PAGE_SIZE));
    params.insert("sourceKinds".to_owned(), json!(ALL_THREAD_SOURCE_KINDS));
    params.insert("modelProviders".to_owned(), json!([]));
    if let Some(cursor) = cursor {
        params.insert("cursor".to_owned(), json!(cursor));
    }
    Value::Object(params)
}

type NativeWebSocket = WebSocketStream<UnixStream>;

pub(super) struct NativeConnection {
    websocket: NativeWebSocket,
    next_id: u64,
    codex_home: Option<PathBuf>,
}

impl NativeConnection {
    async fn connect(socket: &Path) -> Result<Self, Box<dyn Error>> {
        let stream = UnixStream::connect(socket).await?;
        let (websocket, _) = client_async("ws://localhost/rpc", stream).await?;
        Ok(Self {
            websocket,
            next_id: 1,
            codex_home: None,
        })
    }

    pub(super) async fn initialize(&mut self) -> Result<(), Box<dyn Error>> {
        let initialized = self
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "codex_session_control_live_test",
                        "title": "Codex Session Control Live Test",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": {
                        "experimentalApi": true,
                        "mcpServerOpenaiFormElicitation": false,
                        "requestAttestation": false,
                        "optOutNotificationMethods": [],
                    },
                }),
            )
            .await?;
        let codex_home = initialized
            .get("codexHome")
            .and_then(Value::as_str)
            .filter(|home| Path::new(home).is_absolute())
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("initialize omitted an absolute Codex home"))?;
        let user_agent = initialized
            .get("userAgent")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other("initialize omitted a user agent"))?;
        if !has_supported_codex_version(user_agent) {
            return Err(
                io::Error::other("Desktop authority is not on the supported version").into(),
            );
        }
        self.codex_home = Some(codex_home);
        self.websocket
            .send(Message::text(json!({"method": "initialized"}).to_string()))
            .await?;
        Ok(())
    }

    pub(super) fn initialized_codex_home(&self) -> Option<&Path> {
        self.codex_home.as_deref()
    }

    pub(super) async fn request(
        &mut self,
        method: &str,
        params: impl serde::Serialize,
    ) -> Result<Value, Box<dyn Error>> {
        let id = self.next_id;
        self.next_id += 1;
        self.websocket
            .send(Message::text(
                json!({"id": id, "method": method, "params": params}).to_string(),
            ))
            .await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let frame = self
                    .websocket
                    .next()
                    .await
                    .ok_or_else(|| io::Error::other("app-server disconnected"))?
                    .map_err(io::Error::other)?;
                let Message::Text(text) = frame else {
                    continue;
                };
                let value: Value = serde_json::from_str(text.as_str())?;
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = value.get("error") {
                    return Err(io::Error::other(format!("{method} failed: {error}")));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| io::Error::other(format!("{method} omitted result")));
            }
        })
        .await
        .map_err(|_| io::Error::other(format!("{method} timed out")))?
        .map_err(Into::into)
    }
}

pub(super) struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

#[derive(Debug)]
struct McpRequestFailure {
    method: String,
    stage: String,
    category: &'static str,
}

impl McpRequestFailure {
    fn rpc(method: &str, error: &Value) -> Self {
        Self::from_error_data(method, error.get("data").unwrap_or(&Value::Null))
    }

    fn tool_result(method: &str, result: &Value) -> Self {
        Self::from_error_data(
            method,
            result.get("structuredContent").unwrap_or(&Value::Null),
        )
    }

    fn from_error_data(method: &str, error_data: &Value) -> Self {
        Self {
            method: method.to_owned(),
            stage: safe_error_stage(error_data).unwrap_or("mcp_rpc").to_owned(),
            category: allowlisted_error_category(error_data),
        }
    }

    fn transport(method: &str, stage: &'static str) -> Self {
        Self {
            method: method.to_owned(),
            stage: stage.to_owned(),
            category: UNKNOWN_ERROR_CATEGORY,
        }
    }

    fn for_tool(&self, tool: &str) -> io::Error {
        io::Error::other(format!("tool={tool}; {self}"))
    }
}

impl fmt::Display for McpRequestFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "method={}; stage={}; category={}",
            self.method, self.stage, self.category
        )
    }
}

impl Error for McpRequestFailure {}

fn safe_error_stage(error: &Value) -> Option<&'static str> {
    match error.get("stage").and_then(Value::as_str) {
        Some("connect") => Some("connect"),
        Some("initialize") => Some("initialize"),
        Some("socket_validation") => Some("socket_validation"),
        Some("observe") => Some("observe"),
        Some("serialize") => Some("serialize"),
        Some("input") => Some("input"),
        Some("threadId") => Some("threadId"),
        Some("threadIds") => Some("threadIds"),
        Some("cursor") => Some("cursor"),
        Some("limit") => Some("limit"),
        Some("archived") => Some("archived"),
        Some("cwd") => Some("cwd"),
        Some("timeoutMs") => Some("timeoutMs"),
        Some("reasoningEffort") => Some("reasoningEffort"),
        Some("_meta.threadId") => Some("_meta.threadId"),
        Some("tool") => Some("tool"),
        Some("validation") => Some("validation"),
        Some("self_target") => Some("self_target"),
        Some("active_override") => Some("active_override"),
        Some("active_turn") => Some("active_turn"),
        Some("thread/list") => Some("thread/list"),
        Some("thread/read") => Some("thread/read"),
        Some("thread/turns/list") => Some("thread/turns/list"),
        Some("thread/resume") => Some("thread/resume"),
        Some("thread/start") => Some("thread/start"),
        Some("thread/fork") => Some("thread/fork"),
        Some("thread/name/set") => Some("thread/name/set"),
        Some("thread/goal/get") => Some("thread/goal/get"),
        Some("thread/goal/set") => Some("thread/goal/set"),
        Some("thread/goal/clear") => Some("thread/goal/clear"),
        Some("turn/start") => Some("turn/start"),
        Some("turn/steer") => Some("turn/steer"),
        Some("turn/interrupt") => Some("turn/interrupt"),
        _ => None,
    }
}

fn allowlisted_error_category(error: &Value) -> &'static str {
    match error.get("category").and_then(Value::as_str) {
        Some("invalid_request") => "invalid_request",
        Some("policy_rejected") => "policy_rejected",
        Some("target_unavailable") => "target_unavailable",
        Some("authority_transport_failure") => "authority_transport_failure",
        Some("stage_timeout") => "stage_timeout",
        Some("native_conflict") => "native_conflict",
        Some("native_error") => "native_error",
        Some("outcome_unknown") => "outcome_unknown",
        _ => UNKNOWN_ERROR_CATEGORY,
    }
}

fn project_tool_call_result(tool: &str, result: &Value) -> Result<Value, io::Error> {
    if result.get("isError") == Some(&Value::Bool(true)) {
        return Err(McpRequestFailure::tool_result(TOOLS_CALL_METHOD, result).for_tool(tool));
    }
    result
        .get("structuredContent")
        .cloned()
        .ok_or_else(|| McpRequestFailure::transport(TOOLS_CALL_METHOD, "mcp_rpc").for_tool(tool))
}

fn tool_call_params(name: &str, arguments: Value, caller: Option<&str>) -> Value {
    let mut params = json!({"name": name, "arguments": arguments});
    if let Some(caller) = caller {
        params
            .as_object_mut()
            .expect("tool call parameters are always an object")
            .insert("_meta".to_owned(), json!({"threadId": caller}));
    }
    params
}

pub(super) fn mcp_json_rpc_tool_error_preserves_allowlisted_context_without_sensitive_data() {
    let error = McpRequestFailure::rpc(
        TOOLS_CALL_METHOD,
        &json!({
            "message": "message-sentinel",
            "data": {
                "tool": "response-tool-sentinel",
                "stage": "turn/start",
                "category": "native_conflict",
                "threadId": "thread-id-sentinel",
                "turnId": "turn-id-sentinel",
                "native": {"message": "native-message-sentinel"},
                "observation": {"prompt": "prompt-sentinel"},
                "reconciliationError": "reconciliation-sentinel"
            }
        }),
    )
    .for_tool("thread_message_send");

    assert_eq!(
        error.to_string(),
        "tool=thread_message_send; method=tools/call; stage=turn/start; category=native_conflict"
    );
    for sentinel in [
        "message-sentinel",
        "response-tool-sentinel",
        "thread-id-sentinel",
        "turn-id-sentinel",
        "native-message-sentinel",
        "prompt-sentinel",
        "reconciliation-sentinel",
    ] {
        assert!(!error.to_string().contains(sentinel));
    }
}

pub(super) fn mcp_tool_result_error_preserves_allowlisted_context_and_fixed_fallbacks() {
    assert_eq!(
        project_tool_call_result(
            "thread_read",
            &json!({"structuredContent": {"threads": []}})
        )
        .unwrap(),
        json!({"threads": []})
    );

    let error = project_tool_call_result(
        "thread_message_send",
        &json!({
            "isError": true,
            "content": [{"type": "text", "text": "content-sentinel"}],
            "structuredContent": {
                "tool": "response-tool-sentinel",
                "stage": "turn/start",
                "category": "native_conflict",
                "message": "message-sentinel",
                "threadId": "thread-id-sentinel",
                "turnId": "turn-id-sentinel",
                "native": {"message": "native-message-sentinel"},
                "observation": {"prompt": "prompt-sentinel"},
                "reconciliationError": "reconciliation-sentinel"
            }
        }),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "tool=thread_message_send; method=tools/call; stage=turn/start; category=native_conflict"
    );
    for sentinel in [
        "content-sentinel",
        "response-tool-sentinel",
        "message-sentinel",
        "thread-id-sentinel",
        "turn-id-sentinel",
        "native-message-sentinel",
        "prompt-sentinel",
        "reconciliation-sentinel",
    ] {
        assert!(!error.to_string().contains(sentinel));
    }

    for result in [
        json!({"isError": true, "content": [{"type": "text", "text": "missing-sentinel"}]}),
        json!({
            "isError": true,
            "content": [{"type": "text", "text": "malformed-sentinel"}],
            "structuredContent": {"stage": ["stage-sentinel"], "category": {"value": "native_error"}}
        }),
        json!({
            "isError": true,
            "content": [{"type": "text", "text": "unallowlisted-sentinel"}],
            "structuredContent": {"stage": "unsafe stage sentinel", "category": "unallowlisted-sentinel"}
        }),
    ] {
        let error = project_tool_call_result("thread_read", &result).unwrap_err();
        assert_eq!(
            error.to_string(),
            "tool=thread_read; method=tools/call; stage=mcp_rpc; category=unknown"
        );
        for sentinel in [
            "missing-sentinel",
            "malformed-sentinel",
            "stage-sentinel",
            "unallowlisted-sentinel",
            "unsafe stage sentinel",
        ] {
            assert!(!error.to_string().contains(sentinel));
        }
    }

    let sensitive_stages = [
        "/tmp/path-sentinel",
        "environment-sentinel",
        "socket-path-sentinel",
        "credential-sentinel",
        "panic-payload-sentinel",
    ];
    for stage in sensitive_stages {
        let error = project_tool_call_result(
            "thread_read",
            &json!({
                "isError": true,
                "content": [{"type": "text", "text": "content-sentinel"}],
                "structuredContent": {
                    "stage": stage,
                    "category": "native_error",
                    "message": "message-sentinel",
                    "native": {"message": "native-message-sentinel"}
                }
            }),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "tool=thread_read; method=tools/call; stage=mcp_rpc; category=native_error"
        );
        for sentinel in sensitive_stages {
            assert!(!error.to_string().contains(sentinel));
        }
        for sentinel in [
            "content-sentinel",
            "message-sentinel",
            "native-message-sentinel",
        ] {
            assert!(!error.to_string().contains(sentinel));
        }
    }
}

pub(super) fn caller_bound_tool_request_keeps_metadata_outside_public_arguments() {
    let params = tool_call_params(
        "thread_message_send",
        json!({"threadId": "target-sentinel", "prompt": "prompt-sentinel"}),
        Some("caller-sentinel"),
    );

    assert_eq!(params["name"], "thread_message_send");
    assert_eq!(
        params["arguments"],
        json!({"threadId": "target-sentinel", "prompt": "prompt-sentinel"})
    );
    assert_eq!(params["_meta"], json!({"threadId": "caller-sentinel"}));
    assert!(params["arguments"].get("_meta").is_none());
    assert!(params["arguments"].get("callerThreadId").is_none());
    assert!(params.get("callerThreadId").is_none());
}

impl McpClient {
    async fn start() -> Result<Self, Box<dyn Error>> {
        let binary = cargo_bin("codex-session-control");
        if !binary.is_file() {
            return Err(io::Error::other("the built CSC binary is unavailable").into());
        }
        let mut command = Command::new(binary);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("CSC child stdin is unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("CSC child stdout is unavailable"))?;
        let mut client = Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
        };
        client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "live-all-tools", "version": "1.0.0"},
                }),
            )
            .await?;
        client
            .notification("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }

    pub(super) async fn list_tools(&mut self) -> Result<Value, Box<dyn Error>> {
        Ok(self.request("tools/list", json!({})).await?)
    }

    pub(super) async fn list_threads(
        &mut self,
        workspace: &Path,
        archived: bool,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool(
            "threads_list",
            json!({"cwd": workspace, "archived": archived}),
        )
        .await
    }

    pub(super) async fn create_thread(
        &mut self,
        workspace: &Path,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool(
            "thread_create",
            json!({
                "cwd": workspace,
                "prompt": "Remain available for the live session-control validation.",
            }),
        )
        .await
    }

    pub(super) async fn fork_thread(
        &mut self,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool(
            "thread_fork",
            json!({"threadId": owned_id.as_str(), "deferGoalContinuation": false}),
        )
        .await
    }

    pub(super) async fn read_thread(
        &mut self,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool("thread_read", json!({"threadId": owned_id.as_str()}))
            .await
    }

    pub(super) async fn wait_threads(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool_as(
            caller,
            "threads_wait",
            json!({"threadIds": [owned_id.as_str()], "timeoutMs": 120_000}),
        )
        .await
    }

    pub(super) async fn send_message(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool_as(
            caller,
            "thread_message_send",
            json!({
                "threadId": owned_id.as_str(),
                "prompt": "Reply exactly READY and take no other action.",
            }),
        )
        .await
    }

    pub(super) async fn set_title(
        &mut self,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool(
            "thread_title_set",
            json!({"threadId": owned_id.as_str(), "title": "Disposable live validation"}),
        )
        .await
    }

    pub(super) async fn get_goal(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool_as(
            caller,
            "thread_goal_get",
            json!({"threadId": owned_id.as_str()}),
        )
        .await
    }

    pub(super) async fn set_goal(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool_as(
            caller,
            "thread_goal_set",
            json!({"threadId": owned_id.as_str(), "objective": "Complete live validation."}),
        )
        .await
    }

    pub(super) async fn pause_goal(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool_as(
            caller,
            "thread_goal_pause",
            json!({"threadId": owned_id.as_str()}),
        )
        .await
    }

    pub(super) async fn resume_goal(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool_as(
            caller,
            "thread_goal_resume",
            json!({"threadId": owned_id.as_str()}),
        )
        .await
    }

    pub(super) async fn clear_goal(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool_as(
            caller,
            "thread_goal_clear",
            json!({"threadId": owned_id.as_str()}),
        )
        .await
    }

    pub(super) async fn interrupt(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool_as(
            caller,
            "thread_interrupt",
            json!({"threadId": owned_id.as_str(), "includeDescendants": false}),
        )
        .await
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, Box<dyn Error>> {
        self.call_tool_request(name, tool_call_params(name, arguments, None))
            .await
    }

    async fn call_tool_as(
        &mut self,
        caller: &OwnedThreadId,
        name: &str,
        arguments: Value,
    ) -> Result<Value, Box<dyn Error>> {
        self.call_tool_request(
            name,
            tool_call_params(name, arguments, Some(caller.as_str())),
        )
        .await
    }

    async fn call_tool_request(
        &mut self,
        name: &str,
        params: Value,
    ) -> Result<Value, Box<dyn Error>> {
        let response = self
            .request(TOOLS_CALL_METHOD, params)
            .await
            .map_err(|failure| failure.for_tool(name))?;
        project_tool_call_result(name, &response).map_err(Into::into)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, McpRequestFailure> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| McpRequestFailure::transport(method, "mcp_rpc"))?;
        stdin
            .write_all(message.to_string().as_bytes())
            .await
            .map_err(|_| McpRequestFailure::transport(method, "mcp_write"))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|_| McpRequestFailure::transport(method, "mcp_write"))?;
        stdin
            .flush()
            .await
            .map_err(|_| McpRequestFailure::transport(method, "mcp_write"))?;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let line = self
                    .stdout
                    .next_line()
                    .await
                    .map_err(|_| McpRequestFailure::transport(method, "mcp_read"))?
                    .ok_or_else(|| McpRequestFailure::transport(method, "mcp_read"))?;
                let value: Value = serde_json::from_str(&line)
                    .map_err(|_| McpRequestFailure::transport(method, "mcp_decode"))?;
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = value.get("error") {
                    return Err(McpRequestFailure::rpc(method, error));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| McpRequestFailure::transport(method, "mcp_rpc"));
            }
        })
        .await
        .map_err(|_| McpRequestFailure::transport(method, "mcp_timeout"))?
    }

    async fn notification(&mut self, method: &str, params: Value) -> Result<(), Box<dyn Error>> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("CSC child stdin is closed"))?;
        stdin.write_all(message.to_string().as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }
}

fn desktop_socket_path() -> Result<PathBuf, Box<dyn Error>> {
    let explicit = std::env::var_os("CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET");
    if let Some(path) = explicit.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Desktop runtime directory is unavailable"))?;
    let app_id = std::env::var_os("CODEX_LINUX_APP_ID")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("codex-desktop"));
    Ok(runtime
        .join(app_id)
        .join("app-server-bridge/app-server.sock"))
}

fn has_supported_codex_version(user_agent: &str) -> bool {
    user_agent
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+'))
        })
        .filter_map(|candidate| semver::Version::parse(candidate).ok())
        .any(|version| version.to_string() == EXPECTED_CODEX_VERSION)
}
