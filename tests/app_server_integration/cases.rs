use std::{
    error::Error,
    ffi::OsStr,
    fmt::Display,
    fs::{self, File},
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    panic::AssertUnwindSafe,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use futures_util::{FutureExt, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{net::UnixListener, sync::Mutex};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::live_harness::LiveHarness;

const LIVE_OPT_IN: &str = "CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS";
const RECOVERY_LEDGER: &str = "CODEX_SESSION_CONTROL_LIVE_RECOVER_LEDGER";
const LEDGER_FILE_NAME: &str = "owned-thread-ids.json";
type LiveRunResult = Result<Result<(), Box<dyn Error>>, Box<dyn std::any::Any + Send>>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(super) struct OwnedThreadId(String);

impl OwnedThreadId {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LedgerDocument {
    workspace: PathBuf,
    owned_thread_ids: Vec<OwnedThreadId>,
}

struct OwnedThreadLedger {
    run_dir: PathBuf,
    path: PathBuf,
    document: LedgerDocument,
}

impl OwnedThreadLedger {
    fn create(run_dir: PathBuf, workspace: PathBuf) -> io::Result<Self> {
        let path = run_dir.join(LEDGER_FILE_NAME);
        let ledger = Self {
            run_dir,
            path,
            document: LedgerDocument {
                workspace,
                owned_thread_ids: Vec::new(),
            },
        };
        ledger.persist()?;
        Ok(ledger)
    }

    fn open(path: PathBuf) -> io::Result<Self> {
        let bytes = fs::read(&path)?;
        let document: LedgerDocument = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if !document.workspace.is_absolute() {
            return Err(io::Error::other(
                "recovery ledger workspace must be absolute",
            ));
        }
        let run_dir = path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| io::Error::other("recovery ledger has no run directory"))?;
        Ok(Self {
            run_dir,
            path,
            document,
        })
    }

    fn record_created_response(&mut self, response: &Value) -> io::Result<OwnedThreadId> {
        self.record_response_id(response.pointer("/threadId"))
    }

    fn record_forked_response(&mut self, response: &Value) -> io::Result<OwnedThreadId> {
        self.record_response_id(response.pointer("/thread/id"))
    }

    fn record_recovered_id(&mut self, id: String) -> io::Result<OwnedThreadId> {
        self.record_id(id)
    }

    fn record_response_id(&mut self, id: Option<&Value>) -> io::Result<OwnedThreadId> {
        let id = id
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| io::Error::other("MCP response omitted a thread ID"))?;
        self.record_id(id.to_owned())
    }

    fn record_id(&mut self, id: String) -> io::Result<OwnedThreadId> {
        if let Some(existing) = self
            .document
            .owned_thread_ids
            .iter()
            .find(|owned| owned.as_str() == id)
        {
            return Ok(existing.clone());
        }
        let owned = OwnedThreadId(id);
        self.document.owned_thread_ids.push(owned.clone());
        self.persist()?;
        Ok(owned)
    }

    fn persist(&self) -> io::Result<()> {
        let bytes = serde_json::to_vec(&self.document).map_err(io::Error::other)?;
        atomic_write_fsync(&self.path, bytes)
    }

    fn cleanup_failure(&self, error: impl Display) -> io::Error {
        io::Error::other(format!(
            "live cleanup failed: {error}; owned={}",
            self.document.owned_thread_ids.len()
        ))
    }

    fn remove_proven_clean_run_dir(&self) -> io::Result<()> {
        let mut entries = fs::read_dir(&self.run_dir)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort();
        let mut expected = vec![self.path.clone(), self.document.workspace.clone()];
        expected.sort();
        if entries != expected {
            return Err(io::Error::other(
                "refusing to remove a run directory with unexpected entries",
            ));
        }
        if fs::read_dir(&self.document.workspace)?.next().is_some() {
            return Err(io::Error::other(
                "refusing to remove a nonempty live workspace",
            ));
        }
        fs::remove_file(&self.path)?;
        fs::remove_dir(&self.document.workspace)?;
        fs::remove_dir(&self.run_dir)
    }
}

fn atomic_write_fsync(path: &Path, bytes: Vec<u8>) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("ledger has no parent"))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged
        .as_file_mut()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    staged.write_all(&bytes)?;
    staged.as_file_mut().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()
}

fn require_live_opt_in(value: Option<&OsStr>) -> io::Result<()> {
    if value == Some(OsStr::new("1")) {
        Ok(())
    } else {
        Err(io::Error::other(
            "live all-tool test requires CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS=1",
        ))
    }
}

fn recovery_ledger(opt_in: Option<&OsStr>, ledger: Option<&OsStr>) -> io::Result<PathBuf> {
    require_live_opt_in(opt_in)?;
    let path = ledger
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .filter(|path| path.file_name() == Some(OsStr::new(LEDGER_FILE_NAME)))
        .ok_or_else(|| {
            io::Error::other("hard-kill recovery requires exact opt-in and an absolute ledger path")
        })?;
    Ok(path)
}

pub(super) fn ledger_persists_each_owned_id_with_file_and_directory_fsync() {
    let run_dir = tempfile::tempdir().unwrap();
    let workspace = run_dir.path().join("workspace");
    let mut ledger = OwnedThreadLedger::create(run_dir.path().to_path_buf(), workspace).unwrap();
    let first = ledger
        .record_created_response(&json!({"threadId": "first"}))
        .unwrap();
    let second = ledger
        .record_forked_response(&json!({"thread": {"id": "second"}}))
        .unwrap();

    assert_eq!(ledger.document.owned_thread_ids, vec![first, second]);
    let persisted: LedgerDocument =
        serde_json::from_slice(&fs::read(&ledger.path).unwrap()).unwrap();
    assert_eq!(persisted.owned_thread_ids, ledger.document.owned_thread_ids);
    assert_eq!(
        fs::metadata(&ledger.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

pub(super) fn ledger_persists_workspace_before_first_creation() {
    let run_dir = tempfile::tempdir().unwrap();
    let workspace = run_dir.path().join("workspace");
    let ledger =
        OwnedThreadLedger::create(run_dir.path().to_path_buf(), workspace.clone()).unwrap();

    let persisted: LedgerDocument =
        serde_json::from_slice(&fs::read(&ledger.path).unwrap()).unwrap();
    assert_eq!(persisted.workspace, workspace);
    assert!(persisted.owned_thread_ids.is_empty());
}

pub(super) fn live_gate_requires_exact_opt_in_before_mutation() {
    assert!(require_live_opt_in(Some(OsStr::new("1"))).is_ok());
    for rejected in [
        None,
        Some(OsStr::new("")),
        Some(OsStr::new("0")),
        Some(OsStr::new("true")),
    ] {
        assert!(require_live_opt_in(rejected).is_err());
    }
}

pub(super) fn recovery_requires_exact_opt_in_and_absolute_ledger() {
    let absolute = PathBuf::from("/tmp/owned-thread-ids.json");
    assert_eq!(
        recovery_ledger(Some(OsStr::new("1")), Some(absolute.as_os_str())).unwrap(),
        absolute
    );
    for (opt_in, ledger) in [
        (None, Some(OsStr::new("/tmp/owned-thread-ids.json"))),
        (
            Some(OsStr::new("0")),
            Some(OsStr::new("/tmp/owned-thread-ids.json")),
        ),
        (
            Some(OsStr::new("1")),
            Some(OsStr::new("owned-thread-ids.json")),
        ),
        (
            Some(OsStr::new("1")),
            Some(OsStr::new("/tmp/not-a-ledger.json")),
        ),
    ] {
        assert!(recovery_ledger(opt_in, ledger).is_err());
    }
}

pub(super) fn cleanup_retains_ledger_until_archive_proof() {
    let run_dir = tempfile::tempdir().unwrap();
    let ledger = OwnedThreadLedger::create(
        run_dir.path().to_path_buf(),
        run_dir.path().join("workspace"),
    )
    .unwrap();

    let _ = ledger.cleanup_failure("archive proof failed");
    assert!(ledger.path.exists());
    assert!(ledger.run_dir.exists());
}

pub(super) fn exact_workspace_list_is_source_complete_and_provider_unfiltered() {
    let params = crate::live_harness::exact_workspace_list_params(
        Path::new("/tmp/codex-session-control-live-test/workspace"),
        false,
        Some("next-page"),
    );

    assert_eq!(
        params,
        json!({
            "cwd": "/tmp/codex-session-control-live-test/workspace",
            "archived": false,
            "limit": 100,
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
            ],
            "modelProviders": [],
            "cursor": "next-page"
        })
    );
}

pub(super) async fn already_archived_exact_ledger_target_skips_archive_and_converges() {
    let run_dir = tempfile::tempdir().unwrap();
    let mut ledger = OwnedThreadLedger::create(
        run_dir.path().to_path_buf(),
        run_dir.path().join("workspace"),
    )
    .unwrap();
    ledger.record_id("owned-target".to_owned()).unwrap();
    let server =
        ArchiveReconciliationServer::start(vec![ExactReadReply::result(exact_thread_read(
            "owned-target",
            "/tmp/codex-home/archived_sessions/rollout.jsonl",
        ))])
        .await;
    let harness = LiveHarness::for_test_native_socket(
        ledger.document.workspace.clone(),
        server.socket.clone(),
    )
    .unwrap();

    let result = archive_and_verify(&harness, &ledger).await;
    let archive_calls = server.archive_call_count().await;

    assert!(
        result.is_ok(),
        "already archived target must converge: {result:?}"
    );
    assert_eq!(archive_calls, 0);
}

pub(super) async fn active_exact_ledger_target_archives_once_then_converges() {
    let run_dir = tempfile::tempdir().unwrap();
    let mut ledger = OwnedThreadLedger::create(
        run_dir.path().to_path_buf(),
        run_dir.path().join("workspace"),
    )
    .unwrap();
    ledger.record_id("owned-target".to_owned()).unwrap();
    let server = ArchiveReconciliationServer::start(vec![
        ExactReadReply::result(exact_thread_read(
            "owned-target",
            "/tmp/codex-home/sessions/2026/08/rollout.jsonl",
        )),
        ExactReadReply::result(exact_thread_read(
            "owned-target",
            "/tmp/codex-home/archived_sessions/rollout.jsonl",
        )),
    ])
    .await;
    let harness = LiveHarness::for_test_native_socket(
        ledger.document.workspace.clone(),
        server.socket.clone(),
    )
    .unwrap();

    let result = archive_and_verify(&harness, &ledger).await;
    let archive_calls = server.archive_call_count().await;

    assert!(
        result.is_ok(),
        "active target must converge after archive: {result:?}"
    );
    assert_eq!(archive_calls, 1);
}

pub(super) async fn invalid_exact_read_evidence_fails_closed_and_retains_ledger() {
    for (category, reply) in [
        (
            "missing",
            ExactReadReply::error(json!({"code": -32602, "message": "thread not found"})),
        ),
        (
            "mismatched_id",
            ExactReadReply::result(exact_thread_read(
                "different-target",
                "/tmp/codex-home/sessions/2026/08/rollout.jsonl",
            )),
        ),
        (
            "unclassifiable_storage",
            ExactReadReply::result(exact_thread_read(
                "owned-target",
                "/tmp/opaque-storage/rollout.jsonl",
            )),
        ),
        (
            "unclassifiable_storage",
            ExactReadReply::result(exact_thread_read(
                "owned-target",
                "/tmp/unrelated/archived_sessions/rollout.jsonl",
            )),
        ),
        (
            "unclassifiable_storage",
            ExactReadReply::result(exact_thread_read(
                "owned-target",
                "/tmp/unrelated/sessions/2026/08/rollout.jsonl",
            )),
        ),
    ] {
        let run_dir = tempfile::tempdir().unwrap();
        let mut ledger = OwnedThreadLedger::create(
            run_dir.path().to_path_buf(),
            run_dir.path().join("workspace"),
        )
        .unwrap();
        ledger.record_id("owned-target".to_owned()).unwrap();
        let server = ArchiveReconciliationServer::start(vec![reply]).await;
        let harness = LiveHarness::for_test_native_socket(
            ledger.document.workspace.clone(),
            server.socket.clone(),
        )
        .unwrap();

        let error = archive_and_verify(&harness, &ledger).await.unwrap_err();
        let archive_calls = server.archive_call_count().await;
        let message = error.to_string();

        assert_eq!(
            message,
            format!(
                "native archive reconciliation failed: stage=thread/read, category={category}, active=0, archived=0, unresolved=1"
            )
        );
        assert_eq!(archive_calls, 0);
        assert!(ledger.path.exists());
        assert!(ledger.run_dir.exists());
        assert!(!message.contains("owned-target"));
        assert!(!message.contains("opaque-storage"));
    }
}

fn exact_thread_read(id: &str, path: &str) -> Value {
    json!({"thread": {"id": id, "path": path}})
}

enum ExactReadReply {
    Result(Value),
    Error(Value),
}

impl ExactReadReply {
    fn result(result: Value) -> Self {
        Self::Result(result)
    }

    fn error(error: Value) -> Self {
        Self::Error(error)
    }
}

struct ArchiveReconciliationServer {
    socket: PathBuf,
    requests: Arc<Mutex<Vec<Value>>>,
    _directory: tempfile::TempDir,
}

impl ArchiveReconciliationServer {
    async fn start(read_replies: Vec<ExactReadReply>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("archive-reconciliation.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded_requests = Arc::clone(&requests);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_async(stream).await.unwrap();
            let mut read_replies = read_replies.into_iter();
            while let Some(Ok(frame)) = websocket.next().await {
                let Message::Text(text) = frame else {
                    continue;
                };
                let request: Value = serde_json::from_str(text.as_str()).unwrap();
                let Some(method) = request.get("method").and_then(Value::as_str) else {
                    continue;
                };
                if method == "initialized" {
                    continue;
                }
                recorded_requests.lock().await.push(request.clone());
                let response = match method {
                    "initialize" => Ok(json!({
                        "codexHome": "/tmp/codex-home",
                        "userAgent": concat!(
                            "codex-cli ",
                            env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION")
                        ),
                    })),
                    "thread/read" => match read_replies.next() {
                        Some(ExactReadReply::Result(result)) => Ok(result),
                        Some(ExactReadReply::Error(error)) => Err(error),
                        None => Err(json!({
                            "code": -32603,
                            "message": "unexpected exact read"
                        })),
                    },
                    "thread/archive" => Ok(Value::Null),
                    _ => Err(json!({"code": -32601, "message": "unexpected native method"})),
                };
                let response = match response {
                    Ok(result) => json!({"id": request["id"], "result": result}),
                    Err(error) => json!({"id": request["id"], "error": error}),
                };
                websocket
                    .send(Message::text(response.to_string()))
                    .await
                    .unwrap();
            }
        });
        Self {
            socket,
            requests,
            _directory: directory,
        }
    }

    async fn archive_call_count(&self) -> usize {
        self.requests
            .lock()
            .await
            .iter()
            .filter(|request| {
                request.get("method") == Some(&Value::String("thread/archive".to_owned()))
            })
            .count()
    }
}

pub(super) fn cleanup_failure_keeps_normal_tool_run_error() {
    let run_result: LiveRunResult = Ok(Err(io::Error::other(
        "pre-teardown discoverability failed",
    )
    .into()));

    assert_eq!(
        cleanup_failure_with_run_result("archive proof failed", &run_result).to_string(),
        "archive proof failed; tool run also failed: pre-teardown discoverability failed"
    );
}

pub(super) fn cleanup_failure_classifies_tool_run_panic_without_payload() {
    let run_result: LiveRunResult = Err(Box::new("sensitive panic payload"));
    let error = cleanup_failure_with_run_result("archive proof failed", &run_result);

    assert_eq!(
        error.to_string(),
        "archive proof failed; tool run also panicked"
    );
    assert!(!error.to_string().contains("sensitive"));
}

pub(super) async fn live_desktop_authority_all_thirteen_tools_are_disposable()
-> Result<(), Box<dyn Error>> {
    let opt_in = std::env::var_os(LIVE_OPT_IN);
    if let Some(recovery_value) = std::env::var_os(RECOVERY_LEDGER) {
        let ledger_path = recovery_ledger(opt_in.as_deref(), Some(&recovery_value))?;
        return recover_hard_kill_ledger(ledger_path).await;
    }
    require_live_opt_in(opt_in.as_deref())?;

    let mut harness = LiveHarness::prepare()?;
    let mut ledger = OwnedThreadLedger::create(
        harness.run_dir().to_path_buf(),
        harness.workspace().to_path_buf(),
    )?;
    let run_result = AssertUnwindSafe(async {
        harness.start_mcp().await?;
        harness.assert_exact_catalog().await?;
        harness.assert_empty_workspace_before_mutation().await?;
        harness.assert_supported_native_version().await?;
        run_all_thirteen_tools(&mut harness, &mut ledger).await
    })
    .catch_unwind()
    .await;
    let cleanup_result: Result<(), Box<dyn Error>> = async {
        harness.stop_and_reap_mcp_child().await?;
        recover_exact_workspace_ids(&harness, &mut ledger).await?;
        archive_and_verify(&harness, &ledger).await?;
        ledger.remove_proven_clean_run_dir()?;
        Ok(())
    }
    .await;

    match (cleanup_result, run_result) {
        (Err(cleanup_error), run_result) => Err(ledger
            .cleanup_failure(cleanup_failure_with_run_result(cleanup_error, &run_result))
            .into()),
        (Ok(()), Ok(result)) => result,
        (Ok(()), Err(payload)) => std::panic::resume_unwind(payload),
    }
}

fn cleanup_failure_with_run_result(cleanup: impl Display, run_result: &LiveRunResult) -> io::Error {
    let mut message = cleanup.to_string();
    match run_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => message.push_str(&format!("; tool run also failed: {error}")),
        Err(_) => message.push_str("; tool run also panicked"),
    }
    io::Error::other(message)
}

async fn recover_hard_kill_ledger(path: PathBuf) -> Result<(), Box<dyn Error>> {
    let mut ledger = OwnedThreadLedger::open(path)?;
    let harness = LiveHarness::from_ledger(&ledger.document.workspace)?;
    let cleanup_result = async {
        recover_exact_workspace_ids(&harness, &mut ledger).await?;
        archive_and_verify(&harness, &ledger).await
    }
    .await;
    if let Err(error) = cleanup_result {
        return Err(ledger.cleanup_failure(error).into());
    }
    if let Err(error) = ledger.remove_proven_clean_run_dir() {
        return Err(ledger.cleanup_failure(error).into());
    }
    Ok(())
}

async fn run_all_thirteen_tools(
    harness: &mut LiveHarness,
    ledger: &mut OwnedThreadLedger,
) -> Result<(), Box<dyn Error>> {
    let created = {
        let response = harness
            .mcp_mut()?
            .create_thread(ledger.document.workspace.as_path())
            .await?;
        ledger.record_created_response(&response)?
    };
    let forked = {
        let response = harness.mcp_mut()?.fork_thread(&created).await?;
        ledger.record_forked_response(&response)?
    };
    let mcp = harness.mcp_mut()?;
    mcp.list_threads(ledger.document.workspace.as_path(), false)
        .await?;
    mcp.read_thread(&created).await?;
    mcp.send_message(&created, &forked).await?;
    mcp.set_goal(&created, &forked).await?;
    mcp.get_goal(&created, &forked).await?;
    mcp.pause_goal(&created, &forked).await?;
    mcp.resume_goal(&created, &forked).await?;
    mcp.clear_goal(&created, &forked).await?;
    mcp.wait_threads(&created, &forked).await?;
    mcp.set_title(&created).await?;
    mcp.interrupt(&created, &forked).await?;
    assert_ne!(
        created, forked,
        "thread_fork must create a distinct owned thread"
    );
    Ok(())
}

async fn recover_exact_workspace_ids(
    harness: &LiveHarness,
    ledger: &mut OwnedThreadLedger,
) -> Result<(), Box<dyn Error>> {
    let mut native = harness.connect_native().await?;
    native.initialize().await?;
    for id in harness.workspace_thread_ids(&mut native, false).await? {
        ledger.record_recovered_id(id)?;
    }
    Ok(())
}

async fn archive_and_verify(
    harness: &LiveHarness,
    ledger: &OwnedThreadLedger,
) -> Result<(), Box<dyn Error>> {
    let mut native = harness.connect_native().await?;
    native.initialize().await?;
    let mut archive_dispatched = vec![false; ledger.document.owned_thread_ids.len()];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);

    loop {
        let mut counts = ExactArchiveCounts::new(ledger.document.owned_thread_ids.len());
        let mut active = Vec::new();
        for (index, owned_id) in ledger.document.owned_thread_ids.iter().enumerate() {
            match exact_thread_storage(&mut native, owned_id).await {
                Ok(ExactThreadStorage::Archived) => counts.archived += 1,
                Ok(ExactThreadStorage::Active) => {
                    counts.active += 1;
                    if !archive_dispatched[index] {
                        active.push((index, owned_id));
                    }
                }
                Err(category) => {
                    return Err(exact_archive_failure("thread/read", category, counts).into());
                }
            }
        }
        if counts.is_complete() {
            return Ok(());
        }
        for (index, owned_id) in active {
            archive_dispatched[index] = true;
            if archive_owned_thread(&mut native, owned_id).await.is_err() {
                return Err(exact_archive_failure("thread/archive", "native_error", counts).into());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(exact_archive_failure("thread/read", "not_archived", counts).into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[derive(Default)]
struct ExactArchiveCounts {
    total: usize,
    active: usize,
    archived: usize,
}

impl ExactArchiveCounts {
    fn new(total: usize) -> Self {
        Self {
            total,
            ..Self::default()
        }
    }

    fn unresolved(&self) -> usize {
        self.total - self.active - self.archived
    }

    fn is_complete(&self) -> bool {
        self.archived == self.total
    }
}

enum ExactThreadStorage {
    Active,
    Archived,
}

async fn exact_thread_storage(
    native: &mut crate::live_harness::NativeConnection,
    owned_id: &OwnedThreadId,
) -> Result<ExactThreadStorage, &'static str> {
    let codex_home = native
        .initialized_codex_home()
        .map(Path::to_path_buf)
        .ok_or("unclassifiable_storage")?;
    let response = native
        .request(
            "thread/read",
            json!({"threadId": owned_id.as_str(), "includeTurns": false}),
        )
        .await
        .map_err(|_| "missing")?;
    let thread = response
        .get("thread")
        .and_then(Value::as_object)
        .ok_or("unclassifiable_storage")?;
    if thread.get("id").and_then(Value::as_str) != Some(owned_id.as_str()) {
        return Err("mismatched_id");
    }
    let path = thread
        .get("path")
        .and_then(Value::as_str)
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or("unclassifiable_storage")?;
    let relative = path
        .strip_prefix(codex_home)
        .map_err(|_| "unclassifiable_storage")?;
    let mut components = relative.components();
    let Some(Component::Normal(storage)) = components.next() else {
        return Err("unclassifiable_storage");
    };
    let storage = match storage {
        storage if storage == OsStr::new("sessions") => ExactThreadStorage::Active,
        storage if storage == OsStr::new("archived_sessions") => ExactThreadStorage::Archived,
        _ => return Err("unclassifiable_storage"),
    };
    let mut has_rollout_component = false;
    for component in components {
        let Component::Normal(component) = component else {
            return Err("unclassifiable_storage");
        };
        if component == OsStr::new("sessions") || component == OsStr::new("archived_sessions") {
            return Err("unclassifiable_storage");
        }
        has_rollout_component = true;
    }
    if has_rollout_component {
        Ok(storage)
    } else {
        Err("unclassifiable_storage")
    }
}

fn exact_archive_failure(stage: &str, category: &str, counts: ExactArchiveCounts) -> io::Error {
    io::Error::other(format!(
        "native archive reconciliation failed: stage={stage}, category={category}, active={}, archived={}, unresolved={}",
        counts.active,
        counts.archived,
        counts.unresolved()
    ))
}

async fn archive_owned_thread(
    native: &mut crate::live_harness::NativeConnection,
    owned_id: &OwnedThreadId,
) -> Result<(), Box<dyn Error>> {
    native
        .request("thread/archive", json!({"threadId": owned_id.as_str()}))
        .await?;
    Ok(())
}
