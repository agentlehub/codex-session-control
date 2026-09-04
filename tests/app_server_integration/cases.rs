use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, File},
    future::Future,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use futures_util::{FutureExt, SinkExt, StreamExt};
use rustix::fs::{AtFlags, CWD, FileType, FlockOperation, Mode, OFlags, flock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{net::UnixListener, sync::Mutex};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::endpoint_policy::{ThreadStorage, classify_thread_storage, is_normalized_absolute_path};
use crate::live_harness::{CleanupBudget, LiveCode, LiveHarness, PROCESS_INHERITANCE_TEST_LOCK};

const LIVE_OPT_IN: &str = "CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS";
const LIVE_HARD_KILL: &str = "CODEX_SESSION_CONTROL_LIVE_HARD_KILL";
const LIVE_RECOVERY: &str = "CODEX_SESSION_CONTROL_LIVE_RECOVER";
const CONTROL_NAME: &str = "codex-session-control";
const LIVE_ROOT_NAME: &str = "live-test";
const JOURNAL_NAME: &str = "current.json";
const STAGING_NAME: &str = "current.next";
const LOCK_NAME: &str = "lock";
const RUNS_NAME: &str = "runs";
const MAX_JOURNAL_BYTES: usize = 4 * 1024 * 1024;
const HARD_KILL_BARRIER_TIMEOUT: Duration = Duration::from_secs(5);
type LiveRunResult = Result<Result<(), LiveCode>, Box<dyn std::any::Any + Send>>;

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
    runtime_dir: Option<File>,
    control_name: Option<OsString>,
    root_parent: File,
    root_name: OsString,
    root_dir: File,
    runs_dir: File,
    run_dir: Option<File>,
    workspace_dir: Option<File>,
    _lock: File,
    document: JournalDocument,
    writes: usize,
    journal_published: bool,
    #[cfg(test)]
    fault: Option<JournalFault>,
}

struct FixedLiveAuthority {
    root: PathBuf,
    runtime_dir: Option<File>,
    control_name: Option<OsString>,
    control_dir: File,
    root_name: OsString,
}

impl FixedLiveAuthority {
    fn from_root_path(root: PathBuf) -> io::Result<Self> {
        let parent_path = root
            .parent()
            .ok_or_else(|| io::Error::other("live authority has no parent"))?;
        let root_name = root
            .file_name()
            .ok_or_else(|| io::Error::other("live authority has no name"))?
            .to_owned();
        let control_dir = open_private_directory_path(parent_path)?;
        Ok(Self {
            root,
            runtime_dir: None,
            control_name: None,
            control_dir,
            root_name,
        })
    }

    fn validate(&self) -> io::Result<()> {
        validate_private_directory_descriptor(&self.control_dir)?;
        if let (Some(runtime_dir), Some(control_name)) =
            (self.runtime_dir.as_ref(), self.control_name.as_deref())
        {
            validate_private_directory_descriptor(runtime_dir)?;
            ensure_child_identity(runtime_dir, control_name, &self.control_dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JournalFault {
    AfterJournalRename,
    GenerationParentSync,
    WorkspaceParentSync,
    WorkspaceRemovalParentSync,
    GenerationRemovalParentSync,
}

impl FixedJournal {
    fn begin(root: PathBuf, generation: &str) -> io::Result<Self> {
        Self::begin_at(FixedLiveAuthority::from_root_path(root)?, generation)
    }

    fn begin_at(authority: FixedLiveAuthority, generation: &str) -> io::Result<Self> {
        if !valid_generation(generation) {
            return Err(io::Error::other("live authority is not wholly absent"));
        }
        authority.validate()?;
        let FixedLiveAuthority {
            root,
            runtime_dir,
            control_name,
            control_dir: root_parent,
            root_name,
        } = authority;
        if open_optional_private_directory_at(&root_parent, &root_name)?.is_some() {
            return Err(io::Error::other("live authority is not wholly absent"));
        }
        let root_dir = create_private_directory_at(&root_parent, &root_name)?;
        rustix::fs::fsync(&root_parent).map_err(io::Error::from)?;
        let runs_dir = create_private_directory_at(&root_dir, OsStr::new(RUNS_NAME))?;
        rustix::fs::fsync(&root_dir).map_err(io::Error::from)?;
        let lock = create_private_file_at(&root_dir, OsStr::new(LOCK_NAME))?;
        rustix::fs::fsync(&root_dir).map_err(io::Error::from)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(io::Error::other)?;
        let mut journal = Self {
            root,
            runtime_dir,
            control_name,
            root_parent,
            root_name,
            root_dir,
            runs_dir,
            run_dir: None,
            workspace_dir: None,
            _lock: lock,
            document: JournalDocument {
                state: JournalState::Idle,
            },
            writes: 0,
            journal_published: false,
            #[cfg(test)]
            fault: None,
        };
        journal.persist()?;
        journal.activate(generation)?;
        Ok(journal)
    }

    fn open(root: PathBuf) -> io::Result<Self> {
        Self::open_at(FixedLiveAuthority::from_root_path(root)?)
    }

    fn open_at(authority: FixedLiveAuthority) -> io::Result<Self> {
        authority.validate()?;
        let FixedLiveAuthority {
            root,
            runtime_dir,
            control_name,
            control_dir: root_parent,
            root_name,
        } = authority;
        let root_dir = open_private_directory_at(&root_parent, &root_name)?;
        let runs_dir = open_private_directory_at(&root_dir, OsStr::new(RUNS_NAME))?;
        let lock = open_private_file_at(
            &root_dir,
            OsStr::new(LOCK_NAME),
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        )?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(io::Error::other)?;
        if probe_child_exists(&root_dir, OsStr::new(STAGING_NAME))? {
            return Err(io::Error::other("live journal staging file is present"));
        }
        let document = read_journal_document(&root_dir)?;
        let mut journal = Self {
            root,
            runtime_dir,
            control_name,
            root_parent,
            root_name,
            root_dir,
            runs_dir,
            run_dir: None,
            workspace_dir: None,
            _lock: lock,
            document,
            writes: 0,
            journal_published: true,
            #[cfg(test)]
            fault: None,
        };
        journal.load_state_directories()?;
        journal.validate_authority(&journal.document)?;
        Ok(journal)
    }

    fn recover(root: PathBuf) -> io::Result<Self> {
        Self::recover_at(FixedLiveAuthority::from_root_path(root)?)
    }

    fn recover_at(authority: FixedLiveAuthority) -> io::Result<Self> {
        let mut journal = Self::open_at(authority)?;
        match journal.document.state {
            JournalState::Active { .. } => journal.persist()?,
            JournalState::CleanupComplete { .. } => {}
            JournalState::Idle => {
                return Err(io::Error::other("live recovery requires cleanup authority"));
            }
        }
        Ok(journal)
    }

    #[cfg(test)]
    fn inject_fault(&mut self, fault: JournalFault) {
        self.fault = Some(fault);
    }

    #[cfg(test)]
    fn take_fault(&mut self, fault: JournalFault) -> bool {
        if self.fault == Some(fault) {
            self.fault = None;
            true
        } else {
            false
        }
    }

    fn activate(&mut self, generation: &str) -> io::Result<()> {
        let JournalState::Idle = self.document.state else {
            return Err(io::Error::other(
                "normal live mode requires an idle journal",
            ));
        };
        if !valid_generation(generation) {
            return Err(io::Error::other("live journal has an invalid generation"));
        }
        self.validate_authority(&self.document)?;
        let run_dir = create_private_directory_at(&self.runs_dir, OsStr::new(generation))?;
        let generation_sync_fault = self.take_fault(JournalFault::GenerationParentSync);
        if let Err(error) = sync_directory(&self.runs_dir, generation_sync_fault) {
            return Err(activation_creation_error(
                error,
                rollback_activation_creation(&self.runs_dir, generation, &run_dir, None),
            ));
        }
        let workspace_dir = match create_private_directory_at(&run_dir, OsStr::new("workspace")) {
            Ok(workspace) => workspace,
            Err(error) => {
                return Err(activation_creation_error(
                    error,
                    rollback_activation_creation(&self.runs_dir, generation, &run_dir, None),
                ));
            }
        };
        let workspace_sync_fault = self.take_fault(JournalFault::WorkspaceParentSync);
        if let Err(error) = sync_directory(&run_dir, workspace_sync_fault) {
            return Err(activation_creation_error(
                error,
                rollback_activation_creation(
                    &self.runs_dir,
                    generation,
                    &run_dir,
                    Some(&workspace_dir),
                ),
            ));
        }
        let run = match private_directory_identity(&run_dir) {
            Ok(identity) => identity,
            Err(error) => {
                return Err(activation_creation_error(
                    error,
                    rollback_activation_creation(
                        &self.runs_dir,
                        generation,
                        &run_dir,
                        Some(&workspace_dir),
                    ),
                ));
            }
        };
        let workspace = match private_directory_identity(&workspace_dir) {
            Ok(identity) => identity,
            Err(error) => {
                return Err(activation_creation_error(
                    error,
                    rollback_activation_creation(
                        &self.runs_dir,
                        generation,
                        &run_dir,
                        Some(&workspace_dir),
                    ),
                ));
            }
        };
        self.run_dir = Some(run_dir);
        self.workspace_dir = Some(workspace_dir);
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

    fn abandon_before_mutation(&mut self) -> io::Result<()> {
        match &self.document.state {
            JournalState::Active {
                owned_thread_ids, ..
            } if owned_thread_ids.is_empty() => {}
            _ => {
                return Err(io::Error::other(
                    "pre-mutation cleanup has no empty active authority",
                ));
            }
        }
        self.cleanup_complete()?;
        self.delete_local_idempotently()
    }

    fn delete_local_idempotently(&mut self) -> io::Result<()> {
        let JournalState::CleanupComplete { generation, .. } = &self.document.state else {
            return Err(io::Error::other("live journal cleanup is not authorized"));
        };
        let generation = generation.clone();
        self.validate_authority(&self.document)?;
        let workspace_sync_fault = self.take_fault(JournalFault::WorkspaceRemovalParentSync);
        let generation_sync_fault = self.take_fault(JournalFault::GenerationRemovalParentSync);
        if let Some(run_dir) = self.run_dir.as_ref() {
            if let Some(workspace_dir) = self.workspace_dir.as_ref() {
                ensure_directory_entries(workspace_dir, &[])?;
                rustix::fs::unlinkat(run_dir, Path::new("workspace"), AtFlags::REMOVEDIR)
                    .map_err(io::Error::from)?;
                self.workspace_dir = None;
            } else if probe_child_exists(run_dir, OsStr::new("workspace"))? {
                return Err(io::Error::other("live journal cleanup authority drifted"));
            }
            sync_directory(run_dir, workspace_sync_fault)?;
            ensure_directory_entries(run_dir, &[])?;
            rustix::fs::unlinkat(&self.runs_dir, Path::new(&generation), AtFlags::REMOVEDIR)
                .map_err(io::Error::from)?;
            self.run_dir = None;
            sync_directory(&self.runs_dir, generation_sync_fault)?;
        } else if probe_child_exists(&self.runs_dir, OsStr::new(&generation))? {
            return Err(io::Error::other("live journal cleanup authority drifted"));
        } else {
            sync_directory(&self.runs_dir, generation_sync_fault)?;
        }
        ensure_directory_entries(&self.runs_dir, &[])?;
        if self.workspace_dir.is_some() {
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
        self.validate_authority(&self.document)?;
        Ok(self.root.join(RUNS_NAME).join(generation).join("workspace"))
    }

    fn journal_path(&self) -> PathBuf {
        self.root.join(JOURNAL_NAME)
    }

    fn runs_path(&self) -> PathBuf {
        self.root.join(RUNS_NAME)
    }

    fn persist_document(&mut self, document: JournalDocument) -> io::Result<()> {
        self.validate_authority(&document)?;
        let bytes = serde_json::to_vec(&document).map_err(io::Error::other)?;
        let post_rename_fault = self.take_fault(JournalFault::AfterJournalRename);
        atomic_write_fixed(&self.root_dir, bytes, post_rename_fault)?;
        self.document = document;
        self.writes += 1;
        self.journal_published = true;
        Ok(())
    }

    fn persist(&mut self) -> io::Result<()> {
        self.persist_document(self.document.clone())
    }

    fn load_state_directories(&mut self) -> io::Result<()> {
        let generation = match &self.document.state {
            JournalState::Idle => return Ok(()),
            JournalState::Active { generation, .. }
            | JournalState::CleanupComplete { generation, .. } => generation,
        };
        self.run_dir = open_optional_private_directory_at(&self.runs_dir, OsStr::new(generation))?;
        self.workspace_dir = match self.run_dir.as_ref() {
            Some(run_dir) => open_optional_private_directory_at(run_dir, OsStr::new("workspace"))?,
            None => None,
        };
        Ok(())
    }

    fn validate_authority(&self, document: &JournalDocument) -> io::Result<()> {
        validate_private_directory_descriptor(&self.root_parent)?;
        if let (Some(runtime_dir), Some(control_name)) =
            (self.runtime_dir.as_ref(), self.control_name.as_deref())
        {
            validate_private_directory_descriptor(runtime_dir)?;
            ensure_child_identity(runtime_dir, control_name, &self.root_parent)?;
        }
        ensure_child_identity(&self.root_parent, &self.root_name, &self.root_dir)?;
        ensure_child_identity(&self.root_dir, OsStr::new(RUNS_NAME), &self.runs_dir)?;
        ensure_child_identity(&self.root_dir, OsStr::new(LOCK_NAME), &self._lock)?;
        if self.journal_published {
            let journal = open_private_file_at(
                &self.root_dir,
                OsStr::new(JOURNAL_NAME),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            )?;
            validate_private_file_descriptor(&journal)?;
        }
        if probe_child_exists(&self.root_dir, OsStr::new(STAGING_NAME))? {
            return Err(io::Error::other("live journal staging file is present"));
        }
        self.validate_layout(document)?;
        self.validate_state(document)
    }

    fn validate_state(&self, document: &JournalDocument) -> io::Result<()> {
        match &document.state {
            JournalState::Idle => {
                if self.run_dir.is_some() || self.workspace_dir.is_some() {
                    return Err(io::Error::other("live authority has unexpected entries"));
                }
                Ok(())
            }
            state @ JournalState::Active { .. } => validate_active_state(
                &self.runs_dir,
                self.run_dir.as_ref(),
                self.workspace_dir.as_ref(),
                state,
            ),
            state @ JournalState::CleanupComplete { .. } => validate_cleanup_complete_state(
                &self.runs_dir,
                self.run_dir.as_ref(),
                self.workspace_dir.as_ref(),
                state,
            ),
        }
    }

    fn validate_layout(&self, document: &JournalDocument) -> io::Result<()> {
        let root_expected = if self.journal_published {
            &[JOURNAL_NAME, LOCK_NAME, RUNS_NAME][..]
        } else {
            &[LOCK_NAME, RUNS_NAME][..]
        };
        ensure_directory_entries(&self.root_dir, root_expected)?;
        let runs_expected = match (&document.state, self.run_dir.as_ref()) {
            (JournalState::Idle, None) | (JournalState::CleanupComplete { .. }, None) => Vec::new(),
            (JournalState::Active { generation, .. }, Some(_))
            | (JournalState::CleanupComplete { generation, .. }, Some(_)) => {
                vec![generation.as_str()]
            }
            _ => return Err(io::Error::other("live authority runs are unexpected")),
        };
        ensure_directory_entries(&self.runs_dir, &runs_expected)
    }
}

fn validate_active_state(
    runs: &File,
    run: Option<&File>,
    workspace: Option<&File>,
    state: &JournalState,
) -> io::Result<()> {
    let JournalState::Active {
        generation,
        run_device,
        run_inode,
        workspace_device,
        workspace_inode,
        owned_thread_ids,
    } = state
    else {
        unreachable!("active validator requires Active")
    };
    if !valid_generation(generation) {
        return Err(io::Error::other("live journal has an invalid generation"));
    }
    let run = run.ok_or_else(|| io::Error::other("live journal run is missing"))?;
    let workspace =
        workspace.ok_or_else(|| io::Error::other("live journal workspace is missing"))?;
    ensure_child_identity(runs, OsStr::new(generation), run)?;
    ensure_child_identity(run, OsStr::new("workspace"), workspace)?;
    if private_directory_identity(run)? != (*run_device, *run_inode)
        || private_directory_identity(workspace)? != (*workspace_device, *workspace_inode)
    {
        return Err(io::Error::other("live journal identity binding drifted"));
    }
    ensure_directory_entries(run, &["workspace"])?;
    let mut unique = BTreeSet::new();
    for id in owned_thread_ids {
        if !unique.insert(id.as_str()) || valid_owned_id(id.as_str()).is_err() {
            return Err(io::Error::other("live journal has an invalid owned ID"));
        }
    }
    Ok(())
}

fn validate_cleanup_complete_state(
    runs: &File,
    run: Option<&File>,
    workspace: Option<&File>,
    state: &JournalState,
) -> io::Result<()> {
    let JournalState::CleanupComplete {
        generation,
        run_device,
        run_inode,
        workspace_device,
        workspace_inode,
    } = state
    else {
        unreachable!("cleanup validator requires CleanupComplete")
    };
    if !valid_generation(generation) {
        return Err(io::Error::other("live journal has an invalid generation"));
    }
    let Some(run) = run else {
        ensure_directory_entries(runs, &[])?;
        return Ok(());
    };
    ensure_child_identity(runs, OsStr::new(generation), run)?;
    if private_directory_identity(run)? != (*run_device, *run_inode) {
        return Err(io::Error::other("live journal identity binding drifted"));
    }
    if let Some(workspace) = workspace {
        ensure_child_identity(run, OsStr::new("workspace"), workspace)?;
        if private_directory_identity(workspace)? != (*workspace_device, *workspace_inode) {
            return Err(io::Error::other("live journal identity binding drifted"));
        }
        ensure_directory_entries(workspace, &[])?;
        ensure_directory_entries(run, &["workspace"])?;
    } else {
        ensure_directory_entries(run, &[])?;
    }
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

fn private_tempdir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    set_mode(directory.path(), 0o700).unwrap();
    directory
}

fn assert_bounded_authority_rejection(operation: impl FnOnce() -> bool + Send + 'static) {
    let (result_sender, result_receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = result_sender.send(operation());
    });
    assert_eq!(
        result_receiver.recv_timeout(std::time::Duration::from_millis(250)),
        Ok(true),
        "unsafe authority rejection blocked"
    );
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

fn atomic_write_fixed(root: &File, bytes: Vec<u8>, fail_after_rename: bool) -> io::Result<()> {
    let mut staged = create_private_file_at(root, OsStr::new(STAGING_NAME))?;
    staged.write_all(&bytes)?;
    rustix::fs::fsync(&staged).map_err(io::Error::from)?;
    drop(staged);
    rustix::fs::renameat(root, Path::new(STAGING_NAME), root, Path::new(JOURNAL_NAME))
        .map_err(io::Error::from)?;
    if fail_after_rename {
        return Err(io::Error::other(
            "injected failure after live journal rename",
        ));
    }
    rustix::fs::fsync(root).map_err(io::Error::from)
}

fn sync_directory(directory: &File, fail: bool) -> io::Result<()> {
    if fail {
        return Err(io::Error::other("injected live directory sync failure"));
    }
    rustix::fs::fsync(directory).map_err(io::Error::from)
}

fn rollback_activation_creation(
    runs: &File,
    generation: &str,
    run: &File,
    workspace: Option<&File>,
) -> io::Result<()> {
    validate_private_directory_descriptor(run)?;
    ensure_child_identity(runs, OsStr::new(generation), run)?;
    if let Some(workspace) = workspace {
        validate_private_directory_descriptor(workspace)?;
        ensure_child_identity(run, OsStr::new("workspace"), workspace)?;
        ensure_directory_entries(workspace, &[])?;
        ensure_directory_entries(run, &["workspace"])?;
        rustix::fs::unlinkat(run, Path::new("workspace"), AtFlags::REMOVEDIR)
            .map_err(io::Error::from)?;
        rustix::fs::fsync(run).map_err(io::Error::from)?;
    }
    ensure_child_identity(runs, OsStr::new(generation), run)?;
    ensure_directory_entries(run, &[])?;
    rustix::fs::unlinkat(runs, Path::new(generation), AtFlags::REMOVEDIR)
        .map_err(io::Error::from)?;
    rustix::fs::fsync(runs).map_err(io::Error::from)?;
    if probe_child_exists(runs, OsStr::new(generation))? {
        return Err(io::Error::other("live activation rollback was incomplete"));
    }
    Ok(())
}

fn activation_creation_error(primary: io::Error, rollback: io::Result<()>) -> io::Error {
    match rollback {
        Ok(()) => primary,
        Err(rollback) => io::Error::other(format!(
            "{primary}; live activation rollback also failed: {rollback}"
        )),
    }
}

fn open_private_directory_path(path: &Path) -> io::Result<File> {
    let directory = rustix::fs::openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from)?;
    validate_private_directory_descriptor(&directory)?;
    Ok(directory)
}

fn open_private_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    open_optional_private_directory_at(parent, name)?
        .ok_or_else(|| io::Error::other("live authority directory is missing"))
}

fn open_optional_private_directory_at(parent: &File, name: &OsStr) -> io::Result<Option<File>> {
    let directory = match rustix::fs::openat(
        parent,
        Path::new(name),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(directory) => File::from(directory),
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(error) => return Err(io::Error::from(error)),
    };
    validate_private_directory_descriptor(&directory)?;
    Ok(Some(directory))
}

fn create_private_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    rustix::fs::mkdirat(parent, Path::new(name), Mode::RWXU).map_err(io::Error::from)?;
    open_private_directory_at(parent, name)
}

fn create_private_file_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let file = rustix::fs::openat(
        parent,
        Path::new(name),
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map(File::from)
    .map_err(io::Error::from)?;
    validate_private_file_descriptor(&file)?;
    Ok(file)
}

fn open_private_file_at(parent: &File, name: &OsStr, flags: OFlags) -> io::Result<File> {
    let file = rustix::fs::openat(
        parent,
        Path::new(name),
        flags | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from)?;
    validate_private_file_descriptor(&file)?;
    Ok(file)
}

fn validate_private_directory_descriptor(directory: &File) -> io::Result<()> {
    let metadata = directory.metadata()?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != uzers::get_current_uid()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(io::Error::other("live authority directory is unsafe"));
    }
    Ok(())
}

fn validate_private_file_descriptor(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != uzers::get_current_uid()
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err(io::Error::other("live authority file is unsafe"));
    }
    Ok(())
}

fn private_directory_identity(directory: &File) -> io::Result<(u64, u64)> {
    validate_private_directory_descriptor(directory)?;
    let metadata = directory.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

fn ensure_child_identity(parent: &File, name: &OsStr, expected: &File) -> io::Result<()> {
    validate_private_authority_descriptor(expected)?;
    let actual = rustix::fs::statat(parent, Path::new(name), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(io::Error::from)?;
    let expected_metadata = expected.metadata()?;
    let expected_type = if expected_metadata.file_type().is_dir() {
        FileType::Directory
    } else if expected_metadata.file_type().is_file() {
        FileType::RegularFile
    } else {
        return Err(io::Error::other("live authority object is unsafe"));
    };
    let expected_mode = if expected_type == FileType::Directory {
        0o700
    } else {
        0o600
    };
    if actual.st_dev != expected_metadata.dev()
        || actual.st_ino != expected_metadata.ino()
        || FileType::from_raw_mode(actual.st_mode) != expected_type
        || actual.st_uid != uzers::get_current_uid()
        || actual.st_mode & 0o7777 != expected_mode
    {
        return Err(io::Error::other("live journal identity binding drifted"));
    }
    Ok(())
}

fn validate_private_authority_descriptor(descriptor: &File) -> io::Result<()> {
    let metadata = descriptor.metadata()?;
    if metadata.file_type().is_dir() {
        validate_private_directory_descriptor(descriptor)
    } else if metadata.file_type().is_file() {
        validate_private_file_descriptor(descriptor)
    } else {
        Err(io::Error::other("live authority object is unsafe"))
    }
}

fn ensure_directory_entries(directory: &File, expected: &[&str]) -> io::Result<()> {
    let mut actual = Vec::new();
    let mut entries = rustix::fs::Dir::read_from(directory).map_err(io::Error::from)?;
    for entry in &mut entries {
        let entry = entry.map_err(io::Error::from)?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            actual.push(name.to_vec());
        }
    }
    actual.sort();
    let mut expected = expected
        .iter()
        .map(|name| name.as_bytes().to_vec())
        .collect::<Vec<_>>();
    expected.sort();
    if actual != expected {
        return Err(io::Error::other("live authority has unexpected entries"));
    }
    Ok(())
}

fn probe_child_exists(parent: &File, name: &OsStr) -> io::Result<bool> {
    match rustix::fs::statat(parent, Path::new(name), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(false),
        Err(error) => Err(io::Error::from(error)),
    }
}

fn read_journal_document(root: &File) -> io::Result<JournalDocument> {
    let file = open_private_file_at(
        root,
        OsStr::new(JOURNAL_NAME),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
    )?;
    let mut bytes = Vec::new();
    file.take((MAX_JOURNAL_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_JOURNAL_BYTES {
        return Err(io::Error::other("live journal exceeds byte limit"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
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
    let _process_inheritance_guard = PROCESS_INHERITANCE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temporary = private_tempdir();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let mut journal = FixedJournal::begin(root, "00000000000000000000000000000001").unwrap();
    let before = fs::read(journal.journal_path()).unwrap();
    fs::write(journal.root.join(STAGING_NAME), b"occupied").unwrap();
    set_mode(&journal.root.join(STAGING_NAME), 0o600).unwrap();

    assert!(journal.record_id("prospective".to_owned()).is_err());
    assert!(matches!(
        &journal.document.state,
        JournalState::Active { owned_thread_ids, .. } if owned_thread_ids.is_empty()
    ));
    assert_eq!(fs::read(journal.root.join(JOURNAL_NAME)).unwrap(), before);
    fs::remove_file(journal.root.join(STAGING_NAME)).unwrap();

    let temporary = private_tempdir();
    let root = temporary.path().join("active-recovery");
    let journal = FixedJournal::begin(root.clone(), "00000000000000000000000000000004").unwrap();
    let active = fs::read(journal.journal_path()).unwrap();
    drop(journal);
    let recovered = FixedJournal::recover(root).unwrap();
    assert_eq!(
        recovered.writes, 1,
        "Active must be re-persisted before use"
    );
    assert_eq!(fs::read(recovered.journal_path()).unwrap(), active);

    let temporary = private_tempdir();
    let root = temporary.path().join("post-rename-failure");
    let mut journal =
        FixedJournal::begin(root.clone(), "00000000000000000000000000000010").unwrap();
    journal.inject_fault(JournalFault::AfterJournalRename);
    assert!(
        journal
            .record_id("durable-but-not-returned".to_owned())
            .is_err()
    );
    assert!(matches!(
        &journal.document.state,
        JournalState::Active { owned_thread_ids, .. } if owned_thread_ids.is_empty()
    ));
    drop(journal);
    let recovered = FixedJournal::recover(root).unwrap();
    assert_eq!(recovered.writes, 1);
    assert_eq!(
        recovered
            .owned_thread_ids()
            .unwrap()
            .iter()
            .map(OwnedThreadId::as_str)
            .collect::<Vec<_>>(),
        ["durable-but-not-returned"]
    );

    for fault in [
        JournalFault::GenerationParentSync,
        JournalFault::WorkspaceParentSync,
    ] {
        let temporary = private_tempdir();
        let root = temporary.path().join(format!("activation-{fault:?}"));
        let mut journal =
            FixedJournal::begin(root.clone(), "00000000000000000000000000000011").unwrap();
        journal.cleanup_complete().unwrap();
        journal.delete_local_idempotently().unwrap();
        journal.inject_fault(fault);
        assert!(
            journal
                .activate("00000000000000000000000000000012")
                .is_err()
        );
        assert_eq!(journal.document.state, JournalState::Idle);
        assert!(matches!(
            serde_json::from_slice::<JournalDocument>(&fs::read(journal.journal_path()).unwrap())
                .unwrap()
                .state,
            JournalState::Idle
        ));
        drop(journal);
        let mut reopened = FixedJournal::open(root).unwrap();
        reopened
            .activate("00000000000000000000000000000012")
            .unwrap();
        reopened.abandon_before_mutation().unwrap();
        assert_eq!(reopened.document.state, JournalState::Idle);
    }

    let temporary = private_tempdir();
    let runs = open_private_directory_path(temporary.path()).unwrap();
    let generation = "00000000000000000000000000000024";
    let run = create_private_directory_at(&runs, OsStr::new(generation)).unwrap();
    let displaced = temporary.path().join("displaced-generation");
    fs::rename(temporary.path().join(generation), &displaced).unwrap();
    fs::create_dir(temporary.path().join(generation)).unwrap();
    set_mode(&temporary.path().join(generation), 0o700).unwrap();
    assert!(rollback_activation_creation(&runs, generation, &run, None).is_err());
    assert!(temporary.path().join(generation).is_dir());
    assert!(displaced.is_dir());

    let temporary = private_tempdir();
    let runs = open_private_directory_path(temporary.path()).unwrap();
    let generation = "00000000000000000000000000000025";
    let run = create_private_directory_at(&runs, OsStr::new(generation)).unwrap();
    let workspace = create_private_directory_at(&run, OsStr::new("workspace")).unwrap();
    let workspace_path = temporary.path().join(generation).join("workspace");
    let displaced = temporary.path().join("displaced-workspace");
    fs::rename(&workspace_path, &displaced).unwrap();
    fs::create_dir(&workspace_path).unwrap();
    set_mode(&workspace_path, 0o700).unwrap();
    assert!(rollback_activation_creation(&runs, generation, &run, Some(&workspace)).is_err());
    assert!(workspace_path.is_dir());
    assert!(displaced.is_dir());

    for fault in [
        JournalFault::WorkspaceRemovalParentSync,
        JournalFault::GenerationRemovalParentSync,
    ] {
        let temporary = private_tempdir();
        let root = temporary.path().join(format!("deletion-{fault:?}"));
        let mut journal =
            FixedJournal::begin(root.clone(), "00000000000000000000000000000013").unwrap();
        journal.cleanup_complete().unwrap();
        journal.inject_fault(fault);
        assert!(journal.delete_local_idempotently().is_err());
        assert!(matches!(
            journal.document.state,
            JournalState::CleanupComplete { .. }
        ));
        drop(journal);
        let mut recovered = FixedJournal::recover(root.clone()).unwrap();
        recovered.delete_local_idempotently().unwrap();
        drop(recovered);
        assert_eq!(
            FixedJournal::open(root).unwrap().document.state,
            JournalState::Idle
        );
    }

    for deletion_boundary in 0..=2 {
        let temporary = private_tempdir();
        let root = temporary
            .path()
            .join(format!("cleanup-boundary-{deletion_boundary}"));
        let mut journal =
            FixedJournal::begin(root.clone(), "00000000000000000000000000000005").unwrap();
        journal.cleanup_complete().unwrap();
        let workspace = journal.workspace().unwrap();
        let generation = workspace.parent().unwrap().to_path_buf();
        drop(journal);
        if deletion_boundary >= 1 {
            fs::remove_dir(&workspace).unwrap();
        }
        if deletion_boundary >= 2 {
            fs::remove_dir(&generation).unwrap();
        }

        let mut recovered = FixedJournal::recover(root.clone()).unwrap();
        assert!(matches!(
            recovered.document.state,
            JournalState::CleanupComplete { .. }
        ));
        assert_eq!(
            recovered.writes, 0,
            "CleanupComplete must not be rewritten before local-only recovery"
        );
        recovered.delete_local_idempotently().unwrap();
        assert_eq!(recovered.document.state, JournalState::Idle);
        drop(recovered);
        assert_eq!(
            FixedJournal::open(root).unwrap().document.state,
            JournalState::Idle
        );
    }

    let temporary = private_tempdir();
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
    let _process_inheritance_guard = PROCESS_INHERITANCE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temporary = private_tempdir();
    assert!(fixed_live_authority_from(temporary.path().join("."), false).is_err());

    let temporary = private_tempdir();
    let root = temporary.path().join("fifo-staging");
    let mut journal = FixedJournal::begin(root, "00000000000000000000000000000022").unwrap();
    rustix::fs::mkfifoat(
        &journal.root_dir,
        Path::new(STAGING_NAME),
        Mode::RUSR | Mode::WUSR,
    )
    .unwrap();
    assert_bounded_authority_rejection(move || journal.record_id("blocked".to_owned()).is_err());

    let temporary = private_tempdir();
    let root = temporary.path().join("fifo-journal");
    drop(FixedJournal::begin(root.clone(), "00000000000000000000000000000023").unwrap());
    fs::remove_file(root.join(JOURNAL_NAME)).unwrap();
    rustix::fs::mkfifoat(CWD, root.join(JOURNAL_NAME), Mode::RUSR | Mode::WUSR).unwrap();
    assert_bounded_authority_rejection(move || FixedJournal::open(root).is_err());

    let runtime = private_tempdir();
    let authority = fixed_live_authority_from(runtime.path().to_path_buf(), true).unwrap();
    let control = runtime.path().join("codex-session-control");
    let displaced_control = runtime.path().join("displaced-control");
    fs::rename(&control, &displaced_control).unwrap();
    fs::create_dir(&control).unwrap();
    set_mode(&control, 0o700).unwrap();
    assert!(FixedJournal::begin_at(authority, "00000000000000000000000000000020",).is_err());

    for drifted in ["runtime", "control", "root", "runs", "lock"] {
        let runtime = private_tempdir();
        let authority = fixed_live_authority_from(runtime.path().to_path_buf(), true).unwrap();
        let mut journal =
            FixedJournal::begin_at(authority, "00000000000000000000000000000021").unwrap();
        let target = match drifted {
            "runtime" => runtime.path().to_path_buf(),
            "control" => runtime.path().join("codex-session-control"),
            "root" => journal.root.clone(),
            "runs" => journal.runs_path(),
            "lock" => journal.root.join(LOCK_NAME),
            _ => unreachable!(),
        };
        set_mode(&target, if drifted == "lock" { 0o644 } else { 0o755 }).unwrap();
        assert!(
            journal
                .record_id(format!("must-not-publish-after-{drifted}-drift"))
                .is_err(),
            "post-open {drifted} metadata drift was accepted"
        );
        assert!(matches!(
            &journal.document.state,
            JournalState::Active { owned_thread_ids, .. } if owned_thread_ids.is_empty()
        ));
    }

    let temporary = private_tempdir();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let journal = FixedJournal::begin(root.clone(), "00000000000000000000000000000002").unwrap();
    drop(journal);
    set_mode(&root.join(LOCK_NAME), 0o644).unwrap();
    assert!(FixedJournal::open(root).is_err());

    let temporary = private_tempdir();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let journal = FixedJournal::begin(root.clone(), "00000000000000000000000000000003").unwrap();
    let mut corrupted = journal.document.clone();
    let JournalState::Active { run_inode, .. } = &mut corrupted.state else {
        panic!("new journal must be active")
    };
    *run_inode += 1;
    fs::write(
        journal.journal_path(),
        serde_json::to_vec(&corrupted).unwrap(),
    )
    .unwrap();
    drop(journal);
    assert!(FixedJournal::open(root).is_err());

    let temporary = private_tempdir();
    let control = temporary.path().join("control");
    fs::create_dir(&control).unwrap();
    set_mode(&control, 0o700).unwrap();
    let root = control.join(LIVE_ROOT_NAME);
    drop(FixedJournal::begin(root, "0000000000000000000000000000000b").unwrap());
    let alias = temporary.path().join("control-alias");
    std::os::unix::fs::symlink(&control, &alias).unwrap();
    assert!(FixedJournal::open(alias.join(LIVE_ROOT_NAME)).is_err());

    let temporary = private_tempdir();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    drop(FixedJournal::begin(root.clone(), "0000000000000000000000000000000b").unwrap());
    let saved_root = temporary.path().join("saved-live-test");
    fs::rename(&root, &saved_root).unwrap();
    std::os::unix::fs::symlink(&saved_root, &root).unwrap();
    assert!(FixedJournal::open(root).is_err());

    for relative in [
        PathBuf::from(LOCK_NAME),
        PathBuf::from(JOURNAL_NAME),
        PathBuf::from(RUNS_NAME),
        PathBuf::from(RUNS_NAME).join("0000000000000000000000000000000c"),
        PathBuf::from(RUNS_NAME)
            .join("0000000000000000000000000000000c")
            .join("workspace"),
    ] {
        let temporary = private_tempdir();
        let root = temporary.path().join(LIVE_ROOT_NAME);
        drop(FixedJournal::begin(root.clone(), "0000000000000000000000000000000c").unwrap());
        let original = root.join(&relative);
        let saved = original.with_extension("saved");
        fs::rename(&original, &saved).unwrap();
        std::os::unix::fs::symlink(&saved, &original).unwrap();
        assert!(
            FixedJournal::open(root).is_err(),
            "symlinked authority component was accepted: {}",
            relative.display()
        );
    }

    let temporary = private_tempdir();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    drop(FixedJournal::begin(root.clone(), "0000000000000000000000000000000d").unwrap());
    std::os::unix::fs::symlink("missing", root.join(STAGING_NAME)).unwrap();
    assert!(FixedJournal::open(root).is_err());

    let temporary = private_tempdir();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let journal = FixedJournal::begin(root.clone(), "0000000000000000000000000000000e").unwrap();
    let workspace = journal.workspace().unwrap();
    fs::rename(&workspace, workspace.with_extension("saved")).unwrap();
    fs::create_dir(&workspace).unwrap();
    set_mode(&workspace, 0o700).unwrap();
    let mut journal = journal;
    assert!(journal.record_id("must-not-publish".to_owned()).is_err());
    assert!(matches!(
        &journal.document.state,
        JournalState::Active { owned_thread_ids, .. } if owned_thread_ids.is_empty()
    ));

    let temporary = private_tempdir();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let journal = FixedJournal::begin(root.clone(), "0000000000000000000000000000000f").unwrap();
    let path = journal.journal_path();
    drop(journal);
    fs::write(&path, vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
    assert!(FixedJournal::open(root).is_err());
}

pub(super) fn live_mode_matrix_is_total_and_recovery_is_fixed_authority() {
    let _process_inheritance_guard = PROCESS_INHERITANCE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    let temporary = private_tempdir();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let mut first = start_normal_journal_at(root.clone()).unwrap();
    let first_generation = match &first.document.state {
        JournalState::Active { generation, .. } => generation.clone(),
        state => panic!("normal activation must be Active, got {state:?}"),
    };
    assert!(
        first_generation.len() == 32
            && first_generation
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_eq!(first.root, root);
    first.cleanup_complete().unwrap();
    first.delete_local_idempotently().unwrap();
    drop(first);

    let mut second = start_normal_journal_at(root.clone()).unwrap();
    let second_generation = match &second.document.state {
        JournalState::Active { generation, .. } => generation.clone(),
        state => panic!("normal activation must be Active, got {state:?}"),
    };
    assert_ne!(first_generation, second_generation);
    assert_eq!(
        second.root, root,
        "the fixed authority root must not rotate"
    );
    second.cleanup_complete().unwrap();
    second.delete_local_idempotently().unwrap();

    let collision = "11111111111111111111111111111111";
    let retry = "22222222222222222222222222222222";
    let collision_path = second.runs_path().join(collision);
    fs::create_dir(&collision_path).unwrap();
    set_mode(&collision_path, 0o700).unwrap();
    assert!(second.activate(collision).is_err());
    assert_eq!(second.document.state, JournalState::Idle);
    fs::remove_dir(&collision_path).unwrap();
    second.activate(retry).unwrap();
    assert!(matches!(
        &second.document.state,
        JournalState::Active {
            generation,
            owned_thread_ids,
            ..
        } if generation == retry && owned_thread_ids.is_empty()
    ));
    second.abandon_before_mutation().unwrap();
    assert_eq!(second.document.state, JournalState::Idle);
    assert!(fs::read_dir(second.runs_path()).unwrap().next().is_none());
}

pub(super) fn workspace_recovery_validates_all_pages_before_one_journal_write() {
    let temporary = private_tempdir();
    let root = temporary.path().join(LIVE_ROOT_NAME);
    let mut journal = FixedJournal::begin(root, "00000000000000000000000000000004").unwrap();
    let workspace = journal.workspace().unwrap();
    let before = fs::read(journal.journal_path()).unwrap();
    let writes = journal.writes;
    let invalid_later_page = vec![
        json!({"data": [{"id": "one", "cwd": workspace}], "nextCursor": "next"}),
        json!({"data": [{"id": "one", "cwd": workspace}]}),
    ];
    assert!(
        crate::live_harness::collect_workspace_page_ids(&workspace, &invalid_later_page).is_err()
    );
    assert_eq!(fs::read(journal.journal_path()).unwrap(), before);
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
    assert_eq!(
        crate::live_harness::collect_workspace_page_ids(
            workspace,
            &[json!({"data": [], "nextCursor": null})],
        )
        .unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        crate::live_harness::collect_workspace_page_ids(
            workspace,
            &[json!({
                "data": [{"id": "terminal", "cwd": workspace}],
                "nextCursor": null,
            })],
        )
        .unwrap(),
        vec!["terminal"]
    );
    for malformed in [json!(false), json!("")] {
        assert!(
            crate::live_harness::collect_workspace_page_ids(
                workspace,
                &[json!({
                    "data": [{"id": "one", "cwd": workspace}],
                    "nextCursor": malformed,
                })],
            )
            .is_err()
        );
    }
    assert!(
        crate::live_harness::collect_workspace_page_ids(
            workspace,
            &[
                json!({"data": [], "nextCursor": "more"}),
                json!({"data": [{"id": "one", "cwd": workspace}]})
            ],
        )
        .is_err()
    );
    assert!(
        crate::live_harness::collect_workspace_page_ids(
            workspace,
            &[
                json!({"data": [{"id": "one", "cwd": workspace}], "nextCursor": "more"}),
                json!({"data": []})
            ],
        )
        .is_err()
    );
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

fn recovery_ledger(opt_in: Option<&OsStr>, ledger: Option<&OsStr>) -> io::Result<FixedJournal> {
    require_live_opt_in(opt_in)?;
    if ledger.is_some() {
        return Err(io::Error::other(
            "hard-kill recovery never accepts a caller-selected journal path",
        ));
    }
    FixedJournal::recover_at(fixed_live_authority(false)?)
}

fn fixed_live_authority(create_control: bool) -> io::Result<FixedLiveAuthority> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("live recovery has no fixed runtime authority"))?;
    fixed_live_authority_from(runtime, create_control)
}

fn fixed_live_authority_from(
    runtime: PathBuf,
    create_control: bool,
) -> io::Result<FixedLiveAuthority> {
    if !is_normalized_absolute_path(&runtime) {
        return Err(io::Error::other(
            "live recovery has no fixed runtime authority",
        ));
    }
    let runtime_dir = open_private_directory_path(&runtime)?;
    let control_name = OsString::from(CONTROL_NAME);
    let control_dir = match open_optional_private_directory_at(&runtime_dir, &control_name)? {
        Some(control_dir) => control_dir,
        None if create_control => {
            let control_dir = create_private_directory_at(&runtime_dir, &control_name)?;
            rustix::fs::fsync(&runtime_dir).map_err(io::Error::from)?;
            control_dir
        }
        None => return Err(io::Error::other("live authority directory is missing")),
    };
    let authority = FixedLiveAuthority {
        root: runtime.join(CONTROL_NAME).join(LIVE_ROOT_NAME),
        runtime_dir: Some(runtime_dir),
        control_name: Some(control_name),
        control_dir,
        root_name: OsString::from(LIVE_ROOT_NAME),
    };
    authority.validate()?;
    Ok(authority)
}

fn start_normal_journal() -> io::Result<FixedJournal> {
    start_normal_journal_with_authority(fixed_live_authority(true)?)
}

fn start_normal_journal_with_authority(authority: FixedLiveAuthority) -> io::Result<FixedJournal> {
    authority.validate()?;
    let root_exists = probe_child_exists(&authority.control_dir, &authority.root_name)?;
    let generation = fresh_generation()?;
    if !root_exists {
        return FixedJournal::begin_at(authority, &generation);
    }
    let mut journal = FixedJournal::open_at(authority)?;
    journal.activate(&generation)?;
    Ok(journal)
}

fn start_normal_journal_at(root: PathBuf) -> io::Result<FixedJournal> {
    start_normal_journal_with_authority(FixedLiveAuthority::from_root_path(root)?)
}

fn fresh_generation() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut generation = String::with_capacity(32);
    for byte in bytes {
        generation.push(HEX[usize::from(byte >> 4)] as char);
        generation.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    Ok(generation)
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
    let run_dir = private_tempdir();
    let mut ledger = FixedJournal::begin(
        run_dir.path().join(LIVE_ROOT_NAME),
        "00000000000000000000000000000007",
    )
    .unwrap();
    let workspace = ledger.workspace().unwrap();
    let server = ArchiveReconciliationServer::start(
        exact_initialized_identity(),
        vec![
            (
                crate::live_harness::exact_workspace_list_params(&workspace, false, None),
                json!({"data": [{"id": "owned-target", "cwd": workspace}], "nextCursor": null}),
            ),
            (
                crate::live_harness::exact_workspace_list_params(&workspace, true, None),
                json!({"data": [], "nextCursor": null}),
            ),
        ],
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
    let writes = ledger.writes;

    let result = reconcile_direct_cleanup(&harness, &mut ledger, CleanupBudget::new()).await;
    let methods = server.method_names().await;

    assert!(
        result.is_ok(),
        "active target must converge after archive: {result:?}"
    );
    assert_eq!(server.connection_count().await, 1);
    assert_eq!(server.method_call_count("initialize").await, 1);
    assert_eq!(server.method_call_count("thread/list").await, 2);
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
    assert_eq!(ledger.writes, writes + 1);

    let mut archived_ledger = FixedJournal::begin(
        run_dir.path().join("already-archived"),
        "00000000000000000000000000000009",
    )
    .unwrap();
    let archived_workspace = archived_ledger.workspace().unwrap();
    let archived_server = ArchiveReconciliationServer::start(
        exact_initialized_identity(),
        vec![
            (
                crate::live_harness::exact_workspace_list_params(&archived_workspace, false, None),
                json!({"data": [], "nextCursor": null}),
            ),
            (
                crate::live_harness::exact_workspace_list_params(&archived_workspace, true, None),
                json!({"data": [{"id": "owned-target", "cwd": archived_workspace}], "nextCursor": null}),
            ),
        ],
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

    reconcile_direct_cleanup(
        &archived_harness,
        &mut archived_ledger,
        CleanupBudget::new(),
    )
    .await
    .unwrap();
    assert_eq!(archived_server.connection_count().await, 1);
    assert_eq!(archived_server.method_call_count("thread/list").await, 2);
    assert_eq!(archived_server.method_call_count("thread/read").await, 1);
    assert_eq!(archived_server.method_call_count("thread/archive").await, 0);

    let mut duplicate_ledger = FixedJournal::begin(
        run_dir.path().join("cross-view-duplicate"),
        "0000000000000000000000000000000a",
    )
    .unwrap();
    let duplicate_workspace = duplicate_ledger.workspace().unwrap();
    let duplicate_server = ArchiveReconciliationServer::start(
        exact_initialized_identity(),
        vec![
            (
                crate::live_harness::exact_workspace_list_params(&duplicate_workspace, false, None),
                json!({"data": [{"id": "duplicate", "cwd": duplicate_workspace}], "nextCursor": null}),
            ),
            (
                crate::live_harness::exact_workspace_list_params(&duplicate_workspace, true, None),
                json!({"data": [{"id": "duplicate", "cwd": duplicate_workspace}], "nextCursor": null}),
            ),
        ],
        Vec::new(),
    )
    .await;
    let duplicate_harness = LiveHarness::for_test_native_socket(
        duplicate_ledger.workspace().unwrap(),
        duplicate_server.socket.clone(),
    )
    .unwrap();
    let writes = duplicate_ledger.writes;
    assert!(
        reconcile_direct_cleanup(
            &duplicate_harness,
            &mut duplicate_ledger,
            CleanupBudget::new(),
        )
        .await
        .is_err()
    );
    assert_eq!(duplicate_ledger.writes, writes);
    assert_eq!(duplicate_server.method_call_count("thread/read").await, 0);
    assert_eq!(
        duplicate_server.method_call_count("thread/archive").await,
        0
    );
}

pub(super) async fn direct_cleanup_requires_safe_endpoint_and_exact_initialized_identity() {
    use crate::endpoint_policy::{APP_ID_ENV, BRIDGE_SOCKET_ENV, RUNTIME_DIR_ENV};
    use crate::live_harness::resolve_desktop_endpoint_for_test;

    let runtime = private_tempdir();
    assert_eq!(
        resolve_desktop_endpoint_for_test(|name| match name {
            BRIDGE_SOCKET_ENV => None,
            RUNTIME_DIR_ENV => Some(runtime.path().as_os_str().to_owned()),
            APP_ID_ENV => Some(OsString::from("codex/desktop")),
            _ => panic!("unexpected endpoint input: {name}"),
        })
        .err()
        .unwrap()
        .to_string(),
        "socket_validation"
    );
    let selected = resolve_desktop_endpoint_for_test(|name| match name {
        BRIDGE_SOCKET_ENV => None,
        RUNTIME_DIR_ENV => Some(runtime.path().as_os_str().to_owned()),
        APP_ID_ENV => Some(OsString::from("Agentle_1.2-x")),
        _ => panic!("unexpected endpoint input: {name}"),
    })
    .unwrap();
    assert_eq!(
        selected.socket_path(),
        runtime
            .path()
            .join("Agentle_1.2-x/app-server-bridge/app-server.sock")
    );

    let first_server =
        ArchiveReconciliationServer::start(exact_initialized_identity(), Vec::new(), Vec::new())
            .await;
    let second_server =
        ArchiveReconciliationServer::start(exact_initialized_identity(), Vec::new(), Vec::new())
            .await;
    let desktop_harness = LiveHarness::for_test_desktop_sockets(
        runtime.path().join("workspace"),
        vec![first_server.socket.clone(), second_server.socket.clone()],
    )
    .unwrap();
    let first_budget = CleanupBudget::new();
    let mut first_connection = desktop_harness.connect_native(first_budget).await.unwrap();
    first_connection.shutdown(first_budget).await.unwrap();
    let second_budget = CleanupBudget::new();
    let mut second_connection = desktop_harness.connect_native(second_budget).await.unwrap();
    second_connection.shutdown(second_budget).await.unwrap();
    assert_eq!(first_server.connection_count().await, 1);
    assert_eq!(second_server.connection_count().await, 1);

    let unsafe_server =
        ArchiveReconciliationServer::start(exact_initialized_identity(), Vec::new(), Vec::new())
            .await;
    unsafe_server.set_socket_mode(0o644);
    assert_rejected_direct_cleanup(&unsafe_server, "endpoint_rejected").await;

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
    let run_dir = private_tempdir();
    let mut ledger = FixedJournal::begin(
        run_dir.path().join(LIVE_ROOT_NAME),
        "00000000000000000000000000000008",
    )
    .unwrap();
    let harness =
        LiveHarness::for_test_native_socket(ledger.workspace().unwrap(), server.socket.clone())
            .unwrap();

    let error = reconcile_direct_cleanup(&harness, &mut ledger, CleanupBudget::new())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), expected);
    assert_eq!(server.method_call_count("initialized").await, 0);
    assert_eq!(server.method_call_count("thread/list").await, 0);
    assert_eq!(server.method_call_count("thread/read").await, 0);
    assert_eq!(server.method_call_count("thread/archive").await, 0);
    assert!(ledger.journal_path().exists());
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
        list_replies: Vec<(Value, Value)>,
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
                        Some((expected_params, result))
                            if request.get("params") == Some(&expected_params) =>
                        {
                            Ok(result)
                        }
                        Some(_) => Err(json!({
                            "code": -32602,
                            "message": "unexpected workspace list parameters"
                        })),
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

pub(super) async fn child_is_owned_immediately_and_every_exit_path_reaps() {
    crate::live_harness::assert_owned_child_exit_paths_for_test().await;
}

pub(super) async fn child_timeout_kills_and_confirms_reap() {
    crate::live_harness::assert_owned_child_timeout_for_test().await;
}

pub(super) async fn deadline_scopes_are_bounded_and_do_not_extend_each_other() {
    crate::live_harness::assert_deadline_and_framing_contract_for_test().await;
}

pub(super) async fn live_codes_are_the_only_output_and_cleanup_has_precedence() {
    assert_live_gate_output_boundary_for_test().await;
    assert_eq!(
        LiveCode::ALL.map(|code| code.to_string()),
        [
            "hard_kill_ready",
            "opt_in_rejected",
            "journal_rejected",
            "endpoint_rejected",
            "identity_unverified",
            "version_unsupported",
            "child_spawn_failed",
            "child_reap_failed",
            "tool_failed",
            "deadline_exceeded",
            "archive_proof_failed",
            "cleanup_failed",
        ]
    );
    assert_eq!(
        finish_live_result(Err(LiveCode::ArchiveProofFailed), Ok(Ok(()))),
        Err(LiveCode::ArchiveProofFailed)
    );
    assert_eq!(
        finish_live_result(
            Err(LiveCode::ChildReapFailed),
            Ok(Err(LiveCode::ToolFailed))
        ),
        Err(LiveCode::ChildReapFailed)
    );
    assert_eq!(
        finish_live_result(
            Err(LiveCode::ChildReapFailed),
            Err(Box::new("sensitive panic payload")),
        ),
        Err(LiveCode::ChildReapFailed)
    );
    assert_eq!(
        finish_live_result(Ok(()), Ok(Err(LiveCode::ToolFailed))),
        Err(LiveCode::ToolFailed)
    );
}

pub(super) async fn live_desktop_authority_all_thirteen_tools_are_disposable() -> ExitCode {
    let opt_in = std::env::var_os(LIVE_OPT_IN);
    let hard_kill = std::env::var_os(LIVE_HARD_KILL);
    let recovery = std::env::var_os(LIVE_RECOVERY);
    let execution = async {
        let mode = select_live_mode(opt_in.as_deref(), hard_kill.as_deref(), recovery.as_deref())
            .map_err(|_| LiveCode::OptInRejected)?;
        execute_live_mode(mode, opt_in.as_deref()).await
    };
    let mut stderr = std::io::stderr().lock();
    live_gate_status(execution, &mut stderr).await
}

type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>;

struct PanicHookGuard {
    previous: Option<PanicHook>,
}

impl PanicHookGuard {
    fn replace(replacement: PanicHook) -> Self {
        let previous = std::panic::take_hook();
        std::panic::set_hook(replacement);
        Self {
            previous: Some(previous),
        }
    }

    fn suppress() -> Self {
        Self::replace(Box::new(|_| {}))
    }
}

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::panic::set_hook(previous);
        }
    }
}

async fn live_gate_status<F, W>(execution: F, output: &mut W) -> ExitCode
where
    F: Future<Output = Result<(), LiveCode>>,
    W: Write,
{
    let hook = PanicHookGuard::suppress();
    let result = AssertUnwindSafe(execution).catch_unwind().await;
    drop(hook);
    match result {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(code)) => {
            let _ = writeln!(output, "{code}");
            ExitCode::FAILURE
        }
        Err(_) => {
            let _ = writeln!(output, "{}", LiveCode::ToolFailed);
            ExitCode::FAILURE
        }
    }
}

async fn assert_live_gate_output_boundary_for_test() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let hook_calls = Arc::new(AtomicUsize::new(0));
    let hook_calls_for_hook = Arc::clone(&hook_calls);
    let hook = PanicHookGuard::replace(Box::new(move |_| {
        hook_calls_for_hook.fetch_add(1, Ordering::SeqCst);
    }));

    let mut error_output = Vec::new();
    let error_status = live_gate_status(
        async { Err(LiveCode::ArchiveProofFailed) },
        &mut error_output,
    )
    .await;
    assert_eq!(error_status, ExitCode::FAILURE);
    assert_eq!(error_output, b"archive_proof_failed\n");

    let mut panic_output = Vec::new();
    let panic_status = live_gate_status(
        async {
            panic!("sensitive panic payload");
            #[allow(unreachable_code)]
            Ok(())
        },
        &mut panic_output,
    )
    .await;
    assert_eq!(panic_status, ExitCode::FAILURE);
    assert_eq!(panic_output, b"tool_failed\n");
    assert_eq!(hook_calls.load(Ordering::SeqCst), 0);

    let _ = std::panic::catch_unwind(|| panic!("restored hook probe"));
    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    drop(hook);
}

async fn execute_live_mode(mode: LiveMode, opt_in: Option<&OsStr>) -> Result<(), LiveCode> {
    if mode == LiveMode::Recovery {
        let budget = CleanupBudget::new();
        budget.check()?;
        let ledger = recovery_ledger(opt_in, None).map_err(|_| LiveCode::JournalRejected)?;
        return recover_hard_kill_ledger(ledger, budget).await;
    }

    let mut ledger = start_normal_journal().map_err(|_| LiveCode::JournalRejected)?;
    let workspace = ledger.workspace().map_err(|_| LiveCode::JournalRejected)?;
    let mut harness = LiveHarness::from_ledger(&workspace)?;
    let preflight_result = AssertUnwindSafe(async {
        harness.start_mcp().await?;
        harness.assert_exact_catalog().await?;
        harness.assert_empty_workspace_before_mutation().await?;
        harness.assert_supported_native_version().await?;
        Ok(())
    })
    .catch_unwind()
    .await;
    if !matches!(preflight_result, Ok(Ok(()))) {
        let budget = CleanupBudget::new();
        let cleanup_result = async {
            harness.stop_and_reap_mcp_child(budget).await?;
            budget.check()?;
            ledger
                .abandon_before_mutation()
                .map_err(|_| LiveCode::CleanupFailed)?;
            Ok(())
        }
        .await;
        return finish_live_result(cleanup_result, preflight_result);
    }

    let mut run_result = AssertUnwindSafe(run_all_thirteen_tools(&mut harness, &mut ledger))
        .catch_unwind()
        .await;
    if mode == LiveMode::HardKill && matches!(run_result, Ok(Ok(()))) {
        let handshake = (|| {
            let mut output = std::io::stdout().lock();
            // End libtest's in-progress `test ...` line before the exact runner token.
            writeln!(output, "\n{}", LiveCode::HardKillReady)?;
            output.flush()
        })();
        if handshake.is_ok() {
            tokio::time::sleep(HARD_KILL_BARRIER_TIMEOUT).await;
            run_result = Ok(Err(LiveCode::DeadlineExceeded));
        } else {
            run_result = Ok(Err(LiveCode::ToolFailed));
        }
    }

    let budget = CleanupBudget::new();
    let cleanup_result = async {
        harness.stop_and_reap_mcp_child(budget).await?;
        reconcile_direct_cleanup(&harness, &mut ledger, budget).await?;
        budget.check()?;
        ledger
            .cleanup_complete()
            .map_err(|_| LiveCode::CleanupFailed)?;
        budget.check()?;
        ledger
            .delete_local_idempotently()
            .map_err(|_| LiveCode::CleanupFailed)?;
        Ok(())
    }
    .await;

    finish_live_result(cleanup_result, run_result)
}

fn finish_live_result(
    cleanup_result: Result<(), LiveCode>,
    run_result: LiveRunResult,
) -> Result<(), LiveCode> {
    match (cleanup_result, run_result) {
        (Err(cleanup_error), _) => Err(cleanup_error),
        (Ok(()), Ok(result)) => result,
        (Ok(()), Err(payload)) => std::panic::resume_unwind(payload),
    }
}

async fn recover_hard_kill_ledger(
    mut ledger: FixedJournal,
    budget: CleanupBudget,
) -> Result<(), LiveCode> {
    budget.check()?;
    if matches!(ledger.document.state, JournalState::CleanupComplete { .. }) {
        ledger
            .delete_local_idempotently()
            .map_err(|_| LiveCode::CleanupFailed)?;
        return Ok(());
    }
    let workspace = ledger.workspace().map_err(|_| LiveCode::JournalRejected)?;
    let harness = LiveHarness::from_ledger(&workspace)?;
    reconcile_direct_cleanup(&harness, &mut ledger, budget).await?;
    budget.check()?;
    ledger
        .cleanup_complete()
        .map_err(|_| LiveCode::CleanupFailed)?;
    budget.check()?;
    ledger
        .delete_local_idempotently()
        .map_err(|_| LiveCode::CleanupFailed)?;
    Ok(())
}

async fn run_all_thirteen_tools(
    harness: &mut LiveHarness,
    ledger: &mut FixedJournal,
) -> Result<(), LiveCode> {
    let workspace = ledger.workspace().map_err(|_| LiveCode::JournalRejected)?;
    let created = {
        let response = harness.mcp_mut()?.create_thread(&workspace).await?;
        ledger
            .record_created_response(&response)
            .map_err(|_| LiveCode::JournalRejected)?
    };
    let forked = {
        let response = harness.mcp_mut()?.fork_thread(&created).await?;
        ledger
            .record_forked_response(&response)
            .map_err(|_| LiveCode::JournalRejected)?
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
    budget: CleanupBudget,
) -> Result<(), LiveCode> {
    budget.check()?;
    let mut native = harness.connect_native(budget).await?;
    let result = async {
        native.initialize(budget).await?;
        native.recovery_home()?;
        native.finish_initialization(budget).await?;
        let active = harness
            .workspace_thread_ids(&mut native, false, budget)
            .await?;
        let archived = harness
            .workspace_thread_ids(&mut native, true, budget)
            .await?;
        let mut unique = BTreeSet::new();
        let mut prospective = Vec::with_capacity(active.len() + archived.len());
        for id in active.into_iter().chain(archived) {
            if !unique.insert(id.clone()) {
                return Err(LiveCode::ArchiveProofFailed);
            }
            prospective.push(id);
        }
        if prospective.is_empty() {
            return Err(LiveCode::ArchiveProofFailed);
        }
        budget.check()?;
        ledger
            .record_recovered_ids(prospective)
            .map_err(|_| LiveCode::JournalRejected)?;
        archive_and_verify(&mut native, ledger, budget).await
    }
    .await;
    let shutdown = native.shutdown(budget).await;
    match (result, shutdown) {
        (_, Err(error)) => Err(error),
        (result, Ok(())) => result,
    }
}

async fn archive_and_verify(
    native: &mut crate::live_harness::NativeConnection,
    ledger: &FixedJournal,
    budget: CleanupBudget,
) -> Result<(), LiveCode> {
    budget.check()?;
    let owned_thread_ids = ledger
        .owned_thread_ids()
        .map_err(|_| LiveCode::JournalRejected)?;
    let mut archive_dispatched = vec![false; owned_thread_ids.len()];

    loop {
        budget.check()?;
        let mut archived = 0;
        let mut active = Vec::new();
        for (index, owned_id) in owned_thread_ids.iter().enumerate() {
            match exact_thread_storage(native, owned_id, budget).await? {
                ThreadStorage::Archived => archived += 1,
                ThreadStorage::Active => {
                    if !archive_dispatched[index] {
                        active.push((index, owned_id));
                    }
                }
            }
        }
        if archived == owned_thread_ids.len() {
            return Ok(());
        }
        for (index, owned_id) in active {
            archive_dispatched[index] = true;
            archive_owned_thread(native, owned_id, budget).await?;
        }
        budget.sleep(std::time::Duration::from_millis(100)).await?;
    }
}

async fn exact_thread_storage(
    native: &mut crate::live_harness::NativeConnection,
    owned_id: &OwnedThreadId,
    budget: CleanupBudget,
) -> Result<ThreadStorage, LiveCode> {
    let codex_home = native
        .recovery_home()
        .map(Path::to_path_buf)
        .map_err(|_| LiveCode::ArchiveProofFailed)?;
    let response = native
        .request(
            "thread/read",
            json!({"threadId": owned_id.as_str(), "includeTurns": false}),
            budget,
        )
        .await?;
    let thread = response
        .get("thread")
        .and_then(Value::as_object)
        .ok_or(LiveCode::ArchiveProofFailed)?;
    let path = thread
        .get("path")
        .and_then(Value::as_str)
        .map(Path::new)
        .ok_or(LiveCode::ArchiveProofFailed)?;
    classify_thread_storage(
        &codex_home,
        owned_id.as_str(),
        thread.get("id").and_then(Value::as_str),
        path,
    )
    .map_err(|_| LiveCode::ArchiveProofFailed)
}

async fn archive_owned_thread(
    native: &mut crate::live_harness::NativeConnection,
    owned_id: &OwnedThreadId,
    budget: CleanupBudget,
) -> Result<(), LiveCode> {
    native
        .request(
            "thread/archive",
            json!({"threadId": owned_id.as_str()}),
            budget,
        )
        .await?;
    Ok(())
}
