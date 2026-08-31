use std::{
    collections::BTreeSet,
    error::Error,
    ffi::OsStr,
    fmt::Display,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::Arc,
};

use futures_util::{FutureExt, SinkExt, StreamExt};
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{net::UnixListener, sync::Mutex};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::endpoint_policy::{ThreadStorage, classify_thread_storage};
use crate::live_harness::LiveHarness;

const LIVE_OPT_IN: &str = "CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS";
const LIVE_HARD_KILL: &str = "CODEX_SESSION_CONTROL_LIVE_HARD_KILL";
const LIVE_RECOVERY: &str = "CODEX_SESSION_CONTROL_LIVE_RECOVER";
const LIVE_ROOT_NAME: &str = "live-test";
const JOURNAL_NAME: &str = "current.json";
const STAGING_NAME: &str = "current.next";
const LOCK_NAME: &str = "lock";
const RUNS_NAME: &str = "runs";
type LiveRunResult = Result<Result<(), Box<dyn Error>>, Box<dyn std::any::Any + Send>>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum JournalState {
    Idle,
    Active {
        generation: String,
        run_device: u64,
        run_inode: u64,
        workspace_device: u64,
        workspace_inode: u64,
        owned_thread_ids: Vec<OwnedThreadId>,
    },
    CleanupComplete {
        generation: String,
        run_device: u64,
        run_inode: u64,
        workspace_device: u64,
        workspace_inode: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalDocument {
    state: JournalState,
}

struct FixedJournal {
    root: PathBuf,
    runs: PathBuf,
    path: PathBuf,
    staging: PathBuf,
    _lock: File,
    document: JournalDocument,
    writes: usize,
}

impl FixedJournal {
    fn begin(root: PathBuf, generation: &str) -> io::Result<Self> {
        if root.exists() || !valid_generation(generation) {
            return Err(io::Error::other("live authority is not wholly absent"));
        }
        fs::create_dir(&root)?;
        set_mode(&root, 0o700)?;
        let runs = root.join(RUNS_NAME);
        fs::create_dir(&runs)?;
        set_mode(&runs, 0o700)?;
        let lock_path = root.join(LOCK_NAME);
        let lock = create_private_file(&lock_path)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(io::Error::other)?;
        let path = root.join(JOURNAL_NAME);
        let staging = root.join(STAGING_NAME);
        let mut journal = Self {
            root,
            runs,
            path,
            staging,
            _lock: lock,
            document: JournalDocument {
                state: JournalState::Idle,
            },
            writes: 0,
        };
        journal.persist()?;
        journal.activate(generation)?;
        Ok(journal)
    }

    fn open(root: PathBuf) -> io::Result<Self> {
        validate_private_dir(&root)?;
        let runs = root.join(RUNS_NAME);
        validate_private_dir(&runs)?;
        let lock_path = root.join(LOCK_NAME);
        validate_private_file(&lock_path)?;
        let lock = OpenOptions::new().read(true).write(true).open(&lock_path)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(io::Error::other)?;
        let path = root.join(JOURNAL_NAME);
        validate_private_file(&path)?;
        let staging = root.join(STAGING_NAME);
        if staging.exists() {
            return Err(io::Error::other("live journal staging file is present"));
        }
        let document = serde_json::from_slice(&fs::read(&path)?).map_err(io::Error::other)?;
        let journal = Self {
            root,
            runs,
            path,
            staging,
            _lock: lock,
            document,
            writes: 0,
        };
        journal.validate_layout()?;
        journal.validate_state()?;
        Ok(journal)
    }

    fn recover(root: PathBuf) -> io::Result<Self> {
        let mut journal = Self::open(root)?;
        if !matches!(journal.document.state, JournalState::Active { .. }) {
            return Err(io::Error::other("live recovery requires an active journal"));
        }
        journal.persist()?;
        Ok(journal)
    }

    fn activate(&mut self, generation: &str) -> io::Result<()> {
        let JournalState::Idle = self.document.state else {
            return Err(io::Error::other(
                "normal live mode requires an idle journal",
            ));
        };
        let run_dir = self.runs.join(generation);
        fs::create_dir(&run_dir)?;
        set_mode(&run_dir, 0o700)?;
        let workspace = run_dir.join("workspace");
        fs::create_dir(&workspace)?;
        set_mode(&workspace, 0o700)?;
        let run = private_dir_identity(&run_dir)?;
        let workspace = private_dir_identity(&workspace)?;
        self.persist_document(JournalDocument {
            state: JournalState::Active {
                generation: generation.to_owned(),
                run_device: run.0,
                run_inode: run.1,
                workspace_device: workspace.0,
                workspace_inode: workspace.1,
                owned_thread_ids: Vec::new(),
            },
        })
    }

    fn record_id(&mut self, id: String) -> io::Result<OwnedThreadId> {
        let owned = valid_owned_id(&id)?;
        let JournalState::Active {
            owned_thread_ids, ..
        } = &self.document.state
        else {
            return Err(io::Error::other("live journal is not active"));
        };
        if let Some(existing) = owned_thread_ids.iter().find(|existing| *existing == &owned) {
            return Ok(existing.clone());
        }
        let mut next = self.document.clone();
        let JournalState::Active {
            owned_thread_ids, ..
        } = &mut next.state
        else {
            unreachable!("checked active journal state")
        };
        owned_thread_ids.push(owned.clone());
        self.persist_document(next)?;
        Ok(owned)
    }

    fn record_created_response(&mut self, response: &Value) -> io::Result<OwnedThreadId> {
        self.record_response_id(response.pointer("/threadId"))
    }

    fn record_forked_response(&mut self, response: &Value) -> io::Result<OwnedThreadId> {
        self.record_response_id(response.pointer("/thread/id"))
    }

    fn record_response_id(&mut self, id: Option<&Value>) -> io::Result<OwnedThreadId> {
        let id = id
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other("MCP response omitted a thread ID"))?;
        self.record_id(id.to_owned())
    }

    fn owned_thread_ids(&self) -> io::Result<Vec<OwnedThreadId>> {
        let JournalState::Active {
            owned_thread_ids, ..
        } = &self.document.state
        else {
            return Err(io::Error::other("live journal is not active"));
        };
        Ok(owned_thread_ids.clone())
    }

    fn cleanup_failure(&self, error: impl Display) -> io::Error {
        let owned = match &self.document.state {
            JournalState::Active {
                owned_thread_ids, ..
            } => owned_thread_ids.len(),
            JournalState::Idle | JournalState::CleanupComplete { .. } => 0,
        };
        io::Error::other(format!("live cleanup failed: {error}; owned={owned}"))
    }

    fn record_recovered_ids(&mut self, ids: Vec<String>) -> io::Result<()> {
        let mut next = self.document.clone();
        let JournalState::Active {
            owned_thread_ids, ..
        } = &mut next.state
        else {
            return Err(io::Error::other("live journal is not active"));
        };
        let mut all = owned_thread_ids
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let mut recovered = BTreeSet::new();
        for id in ids {
            let owned = valid_owned_id(&id)?;
            if !recovered.insert(id.clone()) {
                return Err(io::Error::other("live recovery discovered a duplicate ID"));
            }
            if all.insert(id) {
                owned_thread_ids.push(owned);
            }
        }
        self.persist_document(next)
    }

    fn cleanup_complete(&mut self) -> io::Result<()> {
        let JournalState::Active {
            generation,
            run_device,
            run_inode,
            workspace_device,
            workspace_inode,
            ..
        } = &self.document.state
        else {
            return Err(io::Error::other("live journal is not active"));
        };
        self.persist_document(JournalDocument {
            state: JournalState::CleanupComplete {
                generation: generation.clone(),
                run_device: *run_device,
                run_inode: *run_inode,
                workspace_device: *workspace_device,
                workspace_inode: *workspace_inode,
            },
        })
    }

    fn delete_local_idempotently(&mut self) -> io::Result<()> {
        let JournalState::CleanupComplete {
            generation,
            run_device,
            run_inode,
            workspace_device,
            workspace_inode,
        } = &self.document.state
        else {
            return Err(io::Error::other("live journal cleanup is not authorized"));
        };
        let run_dir = self.runs.join(generation);
        let workspace = run_dir.join("workspace");
        if !run_dir.exists() {
            if fs::read_dir(&self.runs)?.next().is_some() {
                return Err(io::Error::other("live journal cleanup authority drifted"));
            }
            return self.persist_document(JournalDocument {
                state: JournalState::Idle,
            });
        }
        if private_dir_identity(&run_dir)? != (*run_device, *run_inode) {
            return Err(io::Error::other("live journal cleanup authority drifted"));
        }
        if workspace.exists() {
            if private_dir_identity(&workspace)? != (*workspace_device, *workspace_inode)
                || fs::read_dir(&workspace)?.next().is_some()
            {
                return Err(io::Error::other("live journal cleanup authority drifted"));
            }
            fs::remove_dir(&workspace)?;
        }
        if fs::read_dir(&run_dir)?.next().is_some() {
            return Err(io::Error::other("live journal cleanup authority drifted"));
        }
        fs::remove_dir(&run_dir)?;
        if fs::read_dir(&self.runs)?.next().is_some() {
            return Err(io::Error::other("live journal cleanup authority drifted"));
        }
        self.persist_document(JournalDocument {
            state: JournalState::Idle,
        })
    }

    fn workspace(&self) -> io::Result<PathBuf> {
        let (JournalState::Active { generation, .. }
        | JournalState::CleanupComplete { generation, .. }) = &self.document.state
        else {
            return Err(io::Error::other("idle journal has no workspace"));
        };
        Ok(self.runs.join(generation).join("workspace"))
    }

    fn persist_document(&mut self, document: JournalDocument) -> io::Result<()> {
        let bytes = serde_json::to_vec(&document).map_err(io::Error::other)?;
        atomic_write_fixed(&self.path, &self.staging, bytes)?;
        self.document = document;
        self.writes += 1;
        Ok(())
    }

    fn persist(&mut self) -> io::Result<()> {
        self.persist_document(self.document.clone())
    }

    fn validate_state(&self) -> io::Result<()> {
        match &self.document.state {
            JournalState::Idle => Ok(()),
            JournalState::Active {
                generation,
                run_device,
                run_inode,
                workspace_device,
                workspace_inode,
                owned_thread_ids,
            } => validate_active_state(
                &self.runs,
                generation,
                *run_device,
                *run_inode,
                *workspace_device,
                *workspace_inode,
                owned_thread_ids,
            ),
            JournalState::CleanupComplete {
                generation,
                run_device,
                run_inode,
                workspace_device,
                workspace_inode,
            } => validate_active_state(
                &self.runs,
                generation,
                *run_device,
                *run_inode,
                *workspace_device,
                *workspace_inode,
                &[],
            ),
        }
    }

    fn validate_layout(&self) -> io::Result<()> {
        let mut root_entries = fs::read_dir(&self.root)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        root_entries.sort();
        let expected = vec![
            std::ffi::OsString::from(JOURNAL_NAME),
            std::ffi::OsString::from(LOCK_NAME),
            std::ffi::OsString::from(RUNS_NAME),
        ];
        if root_entries != expected {
            return Err(io::Error::other("live authority has unexpected entries"));
        }
        let expected_generation = match &self.document.state {
            JournalState::Idle => None,
            JournalState::Active { generation, .. }
            | JournalState::CleanupComplete { generation, .. } => Some(generation.as_str()),
        };
        let mut run_entries = fs::read_dir(&self.runs)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        run_entries.sort();
        let expected = expected_generation
            .map(|generation| vec![std::ffi::OsString::from(generation)])
            .unwrap_or_default();
        if run_entries != expected {
            return Err(io::Error::other("live authority runs are unexpected"));
        }
        Ok(())
    }
}

fn validate_active_state(
    runs: &Path,
    generation: &str,
    run_device: u64,
    run_inode: u64,
    workspace_device: u64,
    workspace_inode: u64,
    owned_thread_ids: &[OwnedThreadId],
) -> io::Result<()> {
    if !valid_generation(generation) {
        return Err(io::Error::other("live journal has an invalid generation"));
    }
    let run = runs.join(generation);
    let workspace = run.join("workspace");
    if private_dir_identity(&run)? != (run_device, run_inode)
        || private_dir_identity(&workspace)? != (workspace_device, workspace_inode)
    {
        return Err(io::Error::other("live journal identity binding drifted"));
    }
    let mut unique = BTreeSet::new();
    for id in owned_thread_ids {
        if !unique.insert(id.as_str()) || valid_owned_id(id.as_str()).is_err() {
            return Err(io::Error::other("live journal has an invalid owned ID"));
        }
    }
    Ok(())
}

fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

fn validate_private_dir(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != uzers::get_current_uid()
        || metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(io::Error::other("live authority directory is unsafe"));
    }
    Ok(())
}

fn private_dir_identity(path: &Path) -> io::Result<(u64, u64)> {
    validate_private_dir(path)?;
    let metadata = fs::metadata(path)?;
    Ok((metadata.dev(), metadata.ino()))
}

fn validate_private_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != uzers::get_current_uid()
        || metadata.permissions().mode() & 0o7777 != 0o600
    {
        return Err(io::Error::other("live authority file is unsafe"));
    }
    Ok(())
}

fn valid_generation(generation: &str) -> bool {
    generation.len() == 32 && generation.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_owned_id(id: &str) -> io::Result<OwnedThreadId> {
    if id.is_empty() || id.len() > 512 || id.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(io::Error::other("live journal has an invalid owned ID"));
    }
    Ok(OwnedThreadId(id.to_owned()))
}

fn atomic_write_fixed(path: &Path, staging: &Path, bytes: Vec<u8>) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| Some(*parent) == staging.parent())
        .ok_or_else(|| io::Error::other("live journal has inconsistent parents"))?;
    let mut staged = create_private_file(staging)?;
    staged.write_all(&bytes)?;
    staged.sync_all()?;
    fs::rename(staging, path)?;
    File::open(parent)?.sync_all()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiveMode {
    Normal,
    HardKill,
    Recovery,
}

fn select_live_mode(
    opt_in: Option<&OsStr>,
    hard_kill: Option<&OsStr>,
    recovery: Option<&OsStr>,
) -> io::Result<LiveMode> {
    match (opt_in, hard_kill, recovery) {
        (Some(value), None, None) if value == OsStr::new("1") => Ok(LiveMode::Normal),
        (Some(value), Some(hard_kill), None)
            if value == OsStr::new("1") && hard_kill == OsStr::new("1") =>
        {
            Ok(LiveMode::HardKill)
        }
        (Some(value), None, Some(recovery))
            if value == OsStr::new("1") && recovery == OsStr::new("1") =>
        {
            Ok(LiveMode::Recovery)
        }
        _ => Err(io::Error::other("live mode opt-in combination is rejected")),
    }
}

pub(super) fn journal_grants_authority_only_after_durable_replace() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let mut journal = FixedJournal::begin(root, "00000000000000000000000000000001").unwrap();
    let before = fs::read(&journal.path).unwrap();
    journal.path = journal.root.clone();

    assert!(journal.record_id("prospective".to_owned()).is_err());
    assert!(matches!(
        &journal.document.state,
        JournalState::Active { owned_thread_ids, .. } if owned_thread_ids.is_empty()
    ));
    assert_eq!(fs::read(journal.root.join(JOURNAL_NAME)).unwrap(), before);

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let mut journal =
        FixedJournal::begin(root.clone(), "00000000000000000000000000000005").unwrap();
    journal.cleanup_complete().unwrap();
    journal.delete_local_idempotently().unwrap();
    assert_eq!(journal.document.state, JournalState::Idle);
    drop(journal);
    assert_eq!(
        FixedJournal::open(root).unwrap().document.state,
        JournalState::Idle
    );
}

pub(super) fn journal_rejects_unsafe_or_mismatched_authority() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let journal = FixedJournal::begin(root.clone(), "00000000000000000000000000000002").unwrap();
    drop(journal);
    set_mode(&root.join(LOCK_NAME), 0o644).unwrap();
    assert!(FixedJournal::open(root).is_err());

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let mut journal =
        FixedJournal::begin(root.clone(), "00000000000000000000000000000003").unwrap();
    let JournalState::Active { run_inode, .. } = &mut journal.document.state else {
        panic!("new journal must be active")
    };
    *run_inode += 1;
    journal.persist().unwrap();
    drop(journal);
    assert!(FixedJournal::open(root).is_err());
}

pub(super) fn live_mode_matrix_is_total_and_recovery_is_fixed_authority() {
    assert_eq!(
        select_live_mode(Some(OsStr::new("1")), None, None).unwrap(),
        LiveMode::Normal
    );
    assert_eq!(
        select_live_mode(Some(OsStr::new("1")), Some(OsStr::new("1")), None).unwrap(),
        LiveMode::HardKill
    );
    assert_eq!(
        select_live_mode(Some(OsStr::new("1")), None, Some(OsStr::new("1"))).unwrap(),
        LiveMode::Recovery
    );
    for (opt_in, hard_kill, recovery) in [
        (None, None, None),
        (Some("0"), None, None),
        (Some("1"), Some("0"), None),
        (Some("1"), None, Some("0")),
        (Some("1"), Some("1"), Some("1")),
    ] {
        assert!(
            select_live_mode(
                opt_in.map(OsStr::new),
                hard_kill.map(OsStr::new),
                recovery.map(OsStr::new),
            )
            .is_err()
        );
    }
    assert!(
        recovery_ledger(
            Some(OsStr::new("1")),
            Some(OsStr::new("/tmp/owned-thread-ids.json")),
        )
        .is_err()
    );
}

pub(super) fn workspace_recovery_validates_all_pages_before_one_journal_write() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let mut journal = FixedJournal::begin(root, "00000000000000000000000000000004").unwrap();
    let workspace = journal.workspace().unwrap();
    let before = fs::read(&journal.path).unwrap();
    let writes = journal.writes;
    let invalid_later_page = vec![
        json!({"data": [{"id": "one", "cwd": workspace}], "nextCursor": "next"}),
        json!({"data": [{"id": "one", "cwd": workspace}]}),
    ];
    assert!(
        crate::live_harness::collect_workspace_page_ids(&workspace, &invalid_later_page).is_err()
    );
    assert_eq!(fs::read(&journal.path).unwrap(), before);
    assert_eq!(journal.writes, writes);

    let pages = vec![
        json!({"data": [{"id": "one", "cwd": workspace}], "nextCursor": "next"}),
        json!({"data": [{"id": "two", "cwd": workspace}]}),
    ];
    let ids = crate::live_harness::collect_workspace_page_ids(&workspace, &pages).unwrap();
    journal.record_recovered_ids(ids).unwrap();
    assert_eq!(journal.writes, writes + 1);
    assert!(matches!(
        &journal.document.state,
        JournalState::Active { owned_thread_ids, .. }
            if owned_thread_ids.iter().map(OwnedThreadId::as_str).collect::<Vec<_>>() == ["one", "two"]
    ));
}

pub(super) fn workspace_pagination_rejects_cycles_and_exhaustion() {
    let workspace = Path::new("/tmp/codex-session-control-live-test/workspace");
    let cycle = vec![
        json!({"data": [{"id": "one", "cwd": workspace}], "nextCursor": "again"}),
        json!({"data": [{"id": "two", "cwd": workspace}], "nextCursor": "again"}),
    ];
    assert!(crate::live_harness::collect_workspace_page_ids(workspace, &cycle).is_err());

    let exhausted = (0..65)
        .map(|index| {
            let mut page = json!({
                "data": [{"id": format!("owned-{index}"), "cwd": workspace}],
            });
            if index < 64 {
                page["nextCursor"] = json!(format!("cursor-{index}"));
            }
            page
        })
        .collect::<Vec<_>>();
    assert!(crate::live_harness::collect_workspace_page_ids(workspace, &exhausted).is_err());
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(super) struct OwnedThreadId(String);

impl OwnedThreadId {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
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
    if ledger.is_some() {
        return Err(io::Error::other(
            "hard-kill recovery never accepts a caller-selected journal path",
        ));
    }
    Ok(fixed_live_root()?.join(JOURNAL_NAME))
}

fn fixed_live_root() -> io::Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("live recovery has no fixed runtime authority"))?;
    validate_private_dir(&runtime)?;
    Ok(runtime.join("codex-session-control").join(LIVE_ROOT_NAME))
}

fn normal_fixed_live_root() -> io::Result<PathBuf> {
    let root = fixed_live_root()?;
    let control = root
        .parent()
        .ok_or_else(|| io::Error::other("live authority has no control parent"))?;
    if control.exists() {
        validate_private_dir(control)?;
    } else {
        fs::create_dir(control)?;
        set_mode(control, 0o700)?;
    }
    Ok(root)
}

fn start_normal_journal() -> io::Result<FixedJournal> {
    let root = normal_fixed_live_root()?;
    if !root.exists() {
        return FixedJournal::begin(root, "00000000000000000000000000000000");
    }
    let mut journal = FixedJournal::open(root)?;
    journal.activate("00000000000000000000000000000000")?;
    Ok(journal)
}

pub(super) fn archive_classifier_accepts_only_exact_identity_and_storage() {
    use crate::endpoint_policy::{InitializedIdentity, ThreadStorage, classify_thread_storage};

    let expected = env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION");
    for (home, user_agent, expected_result) in [
        (
            Some("relative-home"),
            Some(concat!(
                "codex-cli ",
                env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION")
            )),
            Err("identity_unverified"),
        ),
        (
            Some("relative-home"),
            Some("codex-cli 0.0.1"),
            Err("identity_unverified"),
        ),
        (Some("relative-home"), None, Err("identity_unverified")),
        (
            Some("/tmp//codex-home"),
            Some(concat!(
                "codex-cli ",
                env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION")
            )),
            Err("identity_unverified"),
        ),
        (
            Some("/tmp//codex-home"),
            Some("codex-cli 0.0.1"),
            Err("identity_unverified"),
        ),
        (Some("/tmp//codex-home"), None, Err("identity_unverified")),
        (
            Some("/tmp/codex-home"),
            Some(concat!(
                "codex-cli ",
                env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION")
            )),
            Ok("/tmp/codex-home"),
        ),
        (
            Some("/tmp/codex-home"),
            Some("codex-cli 0.0.1"),
            Err("version_unsupported"),
        ),
        (Some("/tmp/codex-home"), None, Err("identity_unverified")),
    ] {
        let identity = InitializedIdentity::from_initialize(home, user_agent);
        assert_eq!(
            identity.ordinary_codex_home().map(Path::to_path_buf),
            home.filter(|value| !value.is_empty() && Path::new(value).is_absolute())
                .map(PathBuf::from),
            "ordinary initialize handling; home={home:?}"
        );
        assert_eq!(
            identity
                .recovery_home(expected)
                .map(|path| path.to_str().unwrap()),
            expected_result,
            "home={home:?}; user_agent={user_agent:?}"
        );
    }

    let home = Path::new("/tmp/codex-home");
    for (reported_id, path, expected_result) in [
        (
            Some("owned-target"),
            "/tmp/codex-home/sessions/2026/08/rollout.jsonl",
            Ok(ThreadStorage::Active),
        ),
        (
            Some("owned-target"),
            "/tmp/codex-home/archived_sessions/rollout.jsonl",
            Ok(ThreadStorage::Archived),
        ),
        (
            Some("owned-target"),
            "/tmp/other-home/sessions/rollout.jsonl",
            Err("unclassifiable_storage"),
        ),
        (
            Some("owned-target"),
            "relative/sessions/rollout.jsonl",
            Err("unclassifiable_storage"),
        ),
        (
            Some("owned-target"),
            "/tmp/codex-home/sessions/../archived_sessions/rollout.jsonl",
            Err("unclassifiable_storage"),
        ),
        (
            Some("owned-target"),
            "/tmp/codex-home/sessions",
            Err("unclassifiable_storage"),
        ),
        (
            Some("owned-target"),
            "/tmp/codex-home/sessions/archived_sessions/rollout.jsonl",
            Err("unclassifiable_storage"),
        ),
        (
            None,
            "/tmp/codex-home/sessions/rollout.jsonl",
            Err("mismatched_id"),
        ),
        (
            Some("other-target"),
            "/tmp/codex-home/sessions/rollout.jsonl",
            Err("mismatched_id"),
        ),
    ] {
        assert_eq!(
            classify_thread_storage(home, "owned-target", reported_id, Path::new(path)),
            expected_result,
            "reported_id={reported_id:?}; path={path}"
        );
    }
}

pub(super) async fn archive_reconciliation_dispatches_at_most_once_after_exact_active_read() {
    let run_dir = tempfile::tempdir().unwrap();
    let mut ledger = FixedJournal::begin(
        run_dir.path().join(LIVE_ROOT_NAME),
        "00000000000000000000000000000007",
    )
    .unwrap();
    let workspace = ledger.workspace().unwrap();
    let server = ArchiveReconciliationServer::start(
        exact_initialized_identity(),
        vec![json!({"data": [{"id": "owned-target", "cwd": workspace}]})],
        vec![
            ExactReadReply::result(exact_thread_read(
                "owned-target",
                "/tmp/codex-home/sessions/2026/08/rollout.jsonl",
            )),
            ExactReadReply::result(exact_thread_read(
                "owned-target",
                "/tmp/codex-home/sessions/2026/08/rollout.jsonl",
            )),
            ExactReadReply::result(exact_thread_read(
                "owned-target",
                "/tmp/codex-home/archived_sessions/rollout.jsonl",
            )),
        ],
    )
    .await;
    let harness =
        LiveHarness::for_test_native_socket(ledger.workspace().unwrap(), server.socket.clone())
            .unwrap();

    let result = reconcile_direct_cleanup(&harness, &mut ledger).await;
    let methods = server.method_names().await;

    assert!(
        result.is_ok(),
        "active target must converge after archive: {result:?}"
    );
    assert_eq!(server.connection_count().await, 1);
    assert_eq!(server.method_call_count("initialize").await, 1);
    assert_eq!(server.method_call_count("thread/list").await, 1);
    assert_eq!(server.method_call_count("thread/read").await, 3);
    assert_eq!(server.method_call_count("thread/archive").await, 1);
    let first_read = methods
        .iter()
        .position(|method| method == "thread/read")
        .unwrap();
    let archive = methods
        .iter()
        .position(|method| method == "thread/archive")
        .unwrap();
    assert!(
        archive > first_read,
        "archive must follow an exact active read"
    );
    assert_eq!(ledger.owned_thread_ids().unwrap().len(), 1);

    let mut archived_ledger = FixedJournal::begin(
        run_dir.path().join("already-archived"),
        "00000000000000000000000000000009",
    )
    .unwrap();
    let archived_workspace = archived_ledger.workspace().unwrap();
    let archived_server = ArchiveReconciliationServer::start(
        exact_initialized_identity(),
        vec![json!({"data": [{"id": "owned-target", "cwd": archived_workspace}]})],
        vec![ExactReadReply::result(exact_thread_read(
            "owned-target",
            "/tmp/codex-home/archived_sessions/rollout.jsonl",
        ))],
    )
    .await;
    let archived_harness = LiveHarness::for_test_native_socket(
        archived_ledger.workspace().unwrap(),
        archived_server.socket.clone(),
    )
    .unwrap();

    reconcile_direct_cleanup(&archived_harness, &mut archived_ledger)
        .await
        .unwrap();
    assert_eq!(archived_server.connection_count().await, 1);
    assert_eq!(archived_server.method_call_count("thread/archive").await, 0);
}

pub(super) async fn direct_cleanup_requires_safe_endpoint_and_exact_initialized_identity() {
    let unsafe_server =
        ArchiveReconciliationServer::start(exact_initialized_identity(), Vec::new(), Vec::new())
            .await;
    unsafe_server.set_socket_mode(0o644);
    assert_rejected_direct_cleanup(&unsafe_server, "socket_validation").await;

    for (initialized, expected) in [
        (
            json!({"codexHome": "relative-home", "userAgent": "codex-cli 0.0.1"}),
            "identity_unverified",
        ),
        (
            json!({"codexHome": "/tmp//codex-home", "userAgent": "codex-cli 0.0.1"}),
            "identity_unverified",
        ),
        (
            json!({"codexHome": "/tmp/codex-home", "userAgent": "codex-cli not-a-version"}),
            "identity_unverified",
        ),
        (
            json!({"codexHome": "/tmp/codex-home", "userAgent": "codex-cli 0.0.1"}),
            "version_unsupported",
        ),
    ] {
        let server = ArchiveReconciliationServer::start(initialized, Vec::new(), Vec::new()).await;
        assert_rejected_direct_cleanup(&server, expected).await;
    }
}

async fn assert_rejected_direct_cleanup(server: &ArchiveReconciliationServer, expected: &str) {
    let run_dir = tempfile::tempdir().unwrap();
    let mut ledger = FixedJournal::begin(
        run_dir.path().join(LIVE_ROOT_NAME),
        "00000000000000000000000000000008",
    )
    .unwrap();
    let harness =
        LiveHarness::for_test_native_socket(ledger.workspace().unwrap(), server.socket.clone())
            .unwrap();

    let error = reconcile_direct_cleanup(&harness, &mut ledger)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), expected);
    assert_eq!(server.method_call_count("initialized").await, 0);
    assert_eq!(server.method_call_count("thread/list").await, 0);
    assert_eq!(server.method_call_count("thread/read").await, 0);
    assert_eq!(server.method_call_count("thread/archive").await, 0);
    assert!(ledger.path.exists());
    assert!(ledger.workspace().unwrap().exists());
}

fn exact_initialized_identity() -> Value {
    json!({
        "codexHome": "/tmp/codex-home",
        "userAgent": concat!(
            "codex-cli ",
            env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION")
        ),
    })
}

fn exact_thread_read(id: &str, path: &str) -> Value {
    json!({"thread": {"id": id, "path": path}})
}

enum ExactReadReply {
    Result(Value),
}

impl ExactReadReply {
    fn result(result: Value) -> Self {
        Self::Result(result)
    }
}

struct ArchiveReconciliationServer {
    socket: PathBuf,
    requests: Arc<Mutex<Vec<Value>>>,
    connections: Arc<Mutex<usize>>,
    _directory: tempfile::TempDir,
}

impl ArchiveReconciliationServer {
    async fn start(
        initialized: Value,
        list_replies: Vec<Value>,
        read_replies: Vec<ExactReadReply>,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        set_mode(directory.path(), 0o700).unwrap();
        let socket = directory.path().join("archive-reconciliation.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        set_mode(&socket, 0o600).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded_requests = Arc::clone(&requests);
        let connections = Arc::new(Mutex::new(0));
        let recorded_connections = Arc::clone(&connections);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            *recorded_connections.lock().await += 1;
            let mut websocket = accept_async(stream).await.unwrap();
            let mut list_replies = list_replies.into_iter();
            let mut read_replies = read_replies.into_iter();
            while let Some(Ok(frame)) = websocket.next().await {
                let Message::Text(text) = frame else {
                    continue;
                };
                let request: Value = serde_json::from_str(text.as_str()).unwrap();
                let Some(method) = request.get("method").and_then(Value::as_str) else {
                    continue;
                };
                recorded_requests.lock().await.push(request.clone());
                if method == "initialized" {
                    continue;
                }
                let response = match method {
                    "initialize" => Ok(initialized.clone()),
                    "thread/list" => match list_replies.next() {
                        Some(result) => Ok(result),
                        None => Err(json!({
                            "code": -32603,
                            "message": "unexpected workspace list"
                        })),
                    },
                    "thread/read" => match read_replies.next() {
                        Some(ExactReadReply::Result(result)) => Ok(result),
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
            connections,
            _directory: directory,
        }
    }

    fn set_socket_mode(&self, mode: u32) {
        set_mode(&self.socket, mode).unwrap();
    }

    async fn connection_count(&self) -> usize {
        *self.connections.lock().await
    }

    async fn method_call_count(&self, method: &str) -> usize {
        self.requests
            .lock()
            .await
            .iter()
            .filter(|request| request.get("method").and_then(Value::as_str) == Some(method))
            .count()
    }

    async fn method_names(&self) -> Vec<String> {
        self.requests
            .lock()
            .await
            .iter()
            .filter_map(|request| request.get("method").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect()
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
    let hard_kill = std::env::var_os(LIVE_HARD_KILL);
    let recovery = std::env::var_os(LIVE_RECOVERY);
    match select_live_mode(opt_in.as_deref(), hard_kill.as_deref(), recovery.as_deref())? {
        LiveMode::Recovery => {
            let journal = recovery_ledger(opt_in.as_deref(), None)?;
            let root = journal
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| io::Error::other("live recovery journal has no fixed root"))?;
            return recover_hard_kill_ledger(root).await;
        }
        LiveMode::HardKill => {
            return Err(
                io::Error::other("hard-kill orchestration is not available in Slice 1").into(),
            );
        }
        LiveMode::Normal => {}
    }

    let mut ledger = start_normal_journal()?;
    let workspace = ledger.workspace()?;
    let mut harness = LiveHarness::from_ledger(&workspace)?;
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
        reconcile_direct_cleanup(&harness, &mut ledger).await?;
        ledger.cleanup_complete()?;
        ledger.delete_local_idempotently()?;
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

async fn recover_hard_kill_ledger(root: PathBuf) -> Result<(), Box<dyn Error>> {
    let mut ledger = FixedJournal::recover(root)?;
    let workspace = ledger.workspace()?;
    let harness = LiveHarness::from_ledger(&workspace)?;
    let cleanup_result = reconcile_direct_cleanup(&harness, &mut ledger).await;
    if let Err(error) = cleanup_result {
        return Err(ledger.cleanup_failure(error).into());
    }
    if let Err(error) = ledger
        .cleanup_complete()
        .and_then(|()| ledger.delete_local_idempotently())
    {
        return Err(ledger.cleanup_failure(error).into());
    }
    Ok(())
}

async fn run_all_thirteen_tools(
    harness: &mut LiveHarness,
    ledger: &mut FixedJournal,
) -> Result<(), Box<dyn Error>> {
    let workspace = ledger.workspace()?;
    let created = {
        let response = harness.mcp_mut()?.create_thread(&workspace).await?;
        ledger.record_created_response(&response)?
    };
    let forked = {
        let response = harness.mcp_mut()?.fork_thread(&created).await?;
        ledger.record_forked_response(&response)?
    };
    let mcp = harness.mcp_mut()?;
    mcp.list_threads(&workspace, false).await?;
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

async fn reconcile_direct_cleanup(
    harness: &LiveHarness,
    ledger: &mut FixedJournal,
) -> Result<(), Box<dyn Error>> {
    let mut native = harness.connect_native().await?;
    native.initialize().await?;
    native.recovery_home()?;
    native.finish_initialization().await?;
    ledger.record_recovered_ids(harness.workspace_thread_ids(&mut native, false).await?)?;
    archive_and_verify(&mut native, ledger).await
}

async fn archive_and_verify(
    native: &mut crate::live_harness::NativeConnection,
    ledger: &FixedJournal,
) -> Result<(), Box<dyn Error>> {
    let owned_thread_ids = ledger.owned_thread_ids()?;
    let mut archive_dispatched = vec![false; owned_thread_ids.len()];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);

    loop {
        let mut counts = ExactArchiveCounts::new(owned_thread_ids.len());
        let mut active = Vec::new();
        for (index, owned_id) in owned_thread_ids.iter().enumerate() {
            match exact_thread_storage(native, owned_id).await {
                Ok(ThreadStorage::Archived) => counts.archived += 1,
                Ok(ThreadStorage::Active) => {
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
            if archive_owned_thread(native, owned_id).await.is_err() {
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

async fn exact_thread_storage(
    native: &mut crate::live_harness::NativeConnection,
    owned_id: &OwnedThreadId,
) -> Result<ThreadStorage, &'static str> {
    let codex_home = native
        .recovery_home()
        .map(Path::to_path_buf)
        .map_err(|_| "unclassifiable_storage")?;
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
    let path = thread
        .get("path")
        .and_then(Value::as_str)
        .map(Path::new)
        .ok_or("unclassifiable_storage")?;
    classify_thread_storage(
        &codex_home,
        owned_id.as_str(),
        thread.get("id").and_then(Value::as_str),
        path,
    )
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
