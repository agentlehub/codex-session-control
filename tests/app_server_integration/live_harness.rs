use std::{
    collections::{BTreeSet, VecDeque},
    fmt, fs,
    future::Future,
    io,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use assert_cmd::cargo::cargo_bin;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    process::{Child, ChildStdin, ChildStdout, Command},
    time::Instant,
};
use tokio_tungstenite::{WebSocketStream, client_async, tungstenite::Message};

use crate::cases::OwnedThreadId;
use crate::endpoint_policy::{
    BRIDGE_SOCKET_ENV, EndpointMetadata, EndpointResolutionError, InitializedIdentity,
    ResolvedEndpoint, endpoint_metadata_is_safe, is_normalized_absolute_path,
    resolve_endpoint_with,
};

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
const MAX_MCP_FRAME_BYTES: usize = 64 * 1024;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const WAIT_REQUEST_TIMEOUT: Duration = Duration::from_secs(125);
const WAIT_NATIVE_RESERVE: Duration = Duration::from_secs(5);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(60);
const REAP_RESERVE: Duration = Duration::from_secs(2);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub(super) static PROCESS_INHERITANCE_TEST_LOCK: Mutex<()> = Mutex::new(());
const TOOLS_CALL_METHOD: &str = "tools/call";
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LiveCode {
    HardKillReady,
    OptInRejected,
    JournalRejected,
    EndpointRejected,
    IdentityUnverified,
    VersionUnsupported,
    ChildSpawnFailed,
    ChildReapFailed,
    ToolFailed,
    DeadlineExceeded,
    ArchiveProofFailed,
    CleanupFailed,
}

impl LiveCode {
    pub(super) const ALL: [Self; 12] = [
        Self::HardKillReady,
        Self::OptInRejected,
        Self::JournalRejected,
        Self::EndpointRejected,
        Self::IdentityUnverified,
        Self::VersionUnsupported,
        Self::ChildSpawnFailed,
        Self::ChildReapFailed,
        Self::ToolFailed,
        Self::DeadlineExceeded,
        Self::ArchiveProofFailed,
        Self::CleanupFailed,
    ];
}

impl fmt::Display for LiveCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::HardKillReady => "hard_kill_ready",
            Self::OptInRejected => "opt_in_rejected",
            Self::JournalRejected => "journal_rejected",
            Self::EndpointRejected => "endpoint_rejected",
            Self::IdentityUnverified => "identity_unverified",
            Self::VersionUnsupported => "version_unsupported",
            Self::ChildSpawnFailed => "child_spawn_failed",
            Self::ChildReapFailed => "child_reap_failed",
            Self::ToolFailed => "tool_failed",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ArchiveProofFailed => "archive_proof_failed",
            Self::CleanupFailed => "cleanup_failed",
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct Deadline {
    at: Instant,
}

impl Deadline {
    fn after(duration: Duration) -> Self {
        Self {
            at: Instant::now() + duration,
        }
    }

    fn check(self) -> Result<(), LiveCode> {
        if Instant::now() < self.at {
            Ok(())
        } else {
            Err(LiveCode::DeadlineExceeded)
        }
    }

    fn remaining(self) -> Result<Duration, LiveCode> {
        let remaining = self.at.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            Err(LiveCode::DeadlineExceeded)
        } else {
            Ok(remaining)
        }
    }

    async fn run<T>(
        self,
        future: impl Future<Output = Result<T, LiveCode>>,
    ) -> Result<T, LiveCode> {
        self.check()?;
        tokio::time::timeout_at(self.at, future)
            .await
            .map_err(|_| LiveCode::DeadlineExceeded)?
    }

    async fn run_io<T>(
        self,
        future: impl Future<Output = io::Result<T>>,
        failure: LiveCode,
    ) -> Result<T, LiveCode> {
        self.run(async { future.await.map_err(|_| failure) }).await
    }

    fn native_wait_timeout_ms(self) -> Result<u64, LiveCode> {
        let remaining = self.remaining()?;
        let native = remaining
            .checked_sub(WAIT_NATIVE_RESERVE)
            .ok_or(LiveCode::DeadlineExceeded)?;
        u64::try_from(native.as_millis()).map_err(|_| LiveCode::DeadlineExceeded)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CleanupBudget {
    deadline: Deadline,
}

impl CleanupBudget {
    pub(super) fn new() -> Self {
        Self {
            deadline: Deadline::after(CLEANUP_TIMEOUT),
        }
    }

    fn with_timeout(timeout: Duration) -> Self {
        Self {
            deadline: Deadline::after(timeout),
        }
    }

    pub(super) fn check(self) -> Result<(), LiveCode> {
        self.deadline.check()
    }

    pub(super) async fn sleep(self, duration: Duration) -> Result<(), LiveCode> {
        self.deadline
            .run(async {
                tokio::time::sleep(duration).await;
                Ok(())
            })
            .await
    }
}

pub(super) struct OwnedMcpChild {
    child: Option<Child>,
    process_group: Option<u32>,
    group_kill_sent: bool,
    wait_error_once_for_test: bool,
    waitability_error_once_for_test: bool,
}

impl OwnedMcpChild {
    fn spawn(command: &mut Command) -> Result<Self, LiveCode> {
        command.process_group(0);
        let _process_inheritance_guard = PROCESS_INHERITANCE_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let child = command.spawn().map_err(|_| LiveCode::ChildSpawnFailed)?;
        let owned = Self {
            process_group: child.id(),
            child: Some(child),
            group_kill_sent: false,
            wait_error_once_for_test: false,
            waitability_error_once_for_test: false,
        };
        if owned.process_group.is_none() {
            return Err(LiveCode::ChildSpawnFailed);
        }
        Ok(owned)
    }

    fn id(&self) -> u32 {
        self.child
            .as_ref()
            .and_then(Child::id)
            .expect("owned child is present until confirmed reap")
    }

    fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.as_mut()?.stdin.take()
    }

    fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.as_mut()?.stdout.take()
    }

    fn close_owned_stdin(&mut self) {
        if let Some(child) = self.child.as_mut() {
            child.stdin.take();
        }
    }

    fn process_group_exists(&self) -> Result<bool, LiveCode> {
        let Some(raw) = self.process_group else {
            return Ok(false);
        };
        let process_group =
            rustix::process::Pid::from_raw(raw as i32).ok_or(LiveCode::ChildReapFailed)?;
        match rustix::process::test_kill_process_group(process_group) {
            Ok(()) => Ok(true),
            Err(rustix::io::Errno::SRCH) => Ok(false),
            Err(_) => Err(LiveCode::ChildReapFailed),
        }
    }

    fn observe_and_clear_process_group(&mut self) -> Result<(), LiveCode> {
        if self.child.is_some() {
            return Err(LiveCode::ChildReapFailed);
        }
        let result = self.process_group_exists();
        self.process_group.take();
        match result {
            Ok(false) => Ok(()),
            Ok(true) | Err(_) => Err(LiveCode::ChildReapFailed),
        }
    }

    fn kill_process_group_once(&mut self) -> Result<(), LiveCode> {
        if self.child.is_none() {
            return Err(LiveCode::ChildReapFailed);
        }
        if self.group_kill_sent {
            return Ok(());
        }
        self.group_kill_sent = true;
        let Some(raw) = self.process_group else {
            return Ok(());
        };
        let process_group =
            rustix::process::Pid::from_raw(raw as i32).ok_or(LiveCode::ChildReapFailed)?;
        match rustix::process::kill_process_group(process_group, rustix::process::Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(_) => Err(LiveCode::ChildReapFailed),
        }
    }

    fn leader_is_waitable(&mut self) -> Result<bool, LiveCode> {
        if std::mem::take(&mut self.waitability_error_once_for_test) {
            return Err(LiveCode::ChildReapFailed);
        }
        let raw = self
            .child
            .as_ref()
            .and_then(Child::id)
            .ok_or(LiveCode::ChildReapFailed)?;
        let pid = rustix::process::Pid::from_raw(raw as i32).ok_or(LiveCode::ChildReapFailed)?;
        let options = rustix::process::WaitIdOptions::EXITED
            | rustix::process::WaitIdOptions::NOHANG
            | rustix::process::WaitIdOptions::NOWAIT;
        rustix::process::waitid(rustix::process::WaitId::Pid(pid), options)
            .map(|status| status.is_some())
            .map_err(|_| LiveCode::ChildReapFailed)
    }

    async fn wait_until_leader_is_waitable(&mut self, deadline: Deadline) -> Result<(), LiveCode> {
        loop {
            if self.leader_is_waitable()? || Instant::now() >= deadline.at {
                return Ok(());
            }
            tokio::time::sleep(
                CHILD_POLL_INTERVAL.min(deadline.at.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }

    async fn wait_for_leader(&mut self) -> io::Result<std::process::ExitStatus> {
        if std::mem::take(&mut self.wait_error_once_for_test) {
            return Err(io::Error::other("injected child wait failure"));
        }
        self.child
            .as_mut()
            .expect("leader wait requires retained child authority")
            .wait()
            .await
    }

    fn inject_wait_error_once_for_test(&mut self) {
        self.wait_error_once_for_test = true;
    }

    fn inject_waitability_error_once_for_test(&mut self) {
        self.waitability_error_once_for_test = true;
    }

    async fn shutdown_and_reap(&mut self, budget: CleanupBudget) -> Result<(), LiveCode> {
        self.close_owned_stdin();
        let remaining = match budget.deadline.remaining() {
            Ok(remaining) => remaining,
            Err(_) => {
                self.kill_process_group_once()?;
                match self.child.as_mut().map(Child::try_wait) {
                    Some(Ok(Some(_))) => {
                        self.child.take();
                    }
                    Some(Ok(None)) | None => {}
                    Some(Err(_)) => return Err(LiveCode::ChildReapFailed),
                }
                if self.child.is_none() {
                    let _ = self.observe_and_clear_process_group();
                }
                return Err(LiveCode::ChildReapFailed);
            }
        };
        let reserve = REAP_RESERVE.min(remaining / 2);
        let graceful_deadline = budget.deadline.at - reserve;
        let mut cleanup_uncertain = false;

        if self.child.is_some() {
            if self
                .wait_until_leader_is_waitable(Deadline {
                    at: graceful_deadline,
                })
                .await
                .is_err()
            {
                cleanup_uncertain = true;
            }
            self.kill_process_group_once()?;
            if self
                .wait_until_leader_is_waitable(budget.deadline)
                .await
                .is_err()
            {
                cleanup_uncertain = true;
            }
            match tokio::time::timeout_at(budget.deadline.at, self.wait_for_leader()).await {
                Ok(Ok(_)) => {
                    self.child.take();
                }
                Ok(Err(_)) => {
                    cleanup_uncertain = true;
                }
                Err(_) => return Err(LiveCode::ChildReapFailed),
            }
        }

        if self.child.is_some() {
            match tokio::time::timeout_at(budget.deadline.at, self.wait_for_leader()).await {
                Ok(Ok(_)) => {
                    self.child.take();
                }
                Ok(Err(_)) | Err(_) => return Err(LiveCode::ChildReapFailed),
            }
        }

        if self.observe_and_clear_process_group().is_err() || cleanup_uncertain {
            Err(LiveCode::ChildReapFailed)
        } else {
            Ok(())
        }
    }
}

impl Drop for OwnedMcpChild {
    fn drop(&mut self) {
        if self.child.is_none() && self.process_group.is_none() {
            return;
        }
        self.close_owned_stdin();
        if self.child.is_some() {
            let _ = self.kill_process_group_once();
            if matches!(self.child.as_mut().map(Child::try_wait), Some(Ok(Some(_)))) {
                self.child.take();
            }
        }
        if self.child.is_none() {
            let _ = self.observe_and_clear_process_group();
        }
    }
}

async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    deadline: Deadline,
    limit: usize,
) -> Result<Vec<u8>, LiveCode> {
    let mut retained = Vec::with_capacity(limit.min(4096));
    let mut overflow = false;
    loop {
        deadline.check()?;
        let (consumed, line_ended) = {
            let available = deadline
                .run_io(reader.fill_buf(), LiveCode::ToolFailed)
                .await?;
            if available.is_empty() {
                return Err(LiveCode::ToolFailed);
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(available.len(), |index| index + 1);
            let payload = &available[..newline.unwrap_or(available.len())];
            if !overflow {
                let remaining = limit.saturating_sub(retained.len());
                retained.extend_from_slice(&payload[..payload.len().min(remaining)]);
                overflow = payload.len() > remaining;
            }
            (consumed, newline.is_some())
        };
        reader.consume(consumed);
        if line_ended {
            if overflow {
                return Err(LiveCode::ToolFailed);
            }
            if retained.last() == Some(&b'\r') {
                retained.pop();
            }
            return Ok(retained);
        }
    }
}

fn process_group_is_absent(raw: u32) -> bool {
    let Some(process_group) = rustix::process::Pid::from_raw(raw as i32) else {
        return false;
    };
    matches!(
        rustix::process::test_kill_process_group(process_group),
        Err(rustix::io::Errno::SRCH)
    )
}

fn assert_process_and_group_absent(pid: u32) {
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "owned child leader was not reaped"
    );
    assert!(
        process_group_is_absent(pid),
        "owned child process group survived cleanup"
    );
}

fn sleeping_child_command() -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("sleep 30 & wait")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

async fn explicitly_reap_test_owner(owner: &mut Option<OwnedMcpChild>) {
    owner
        .as_mut()
        .expect("test retains explicit child authority")
        .shutdown_and_reap(CleanupBudget::with_timeout(Duration::from_secs(1)))
        .await
        .expect("test child is reaped within its explicit cleanup budget");
    owner.take();
}

async fn assert_startup_failure_retains_explicit_owner_for_test() {
    let mut command = sleeping_child_command();
    command.stdout(Stdio::null());
    let mut owner = None;
    assert!(matches!(
        McpClient::start_command(
            &mut owner,
            &mut command,
            Deadline::after(Duration::from_secs(1)),
            None,
        )
        .await,
        Err(LiveCode::ChildSpawnFailed)
    ));
    let pid = owner
        .as_ref()
        .expect("startup failure must return with explicit ownership")
        .id();
    explicitly_reap_test_owner(&mut owner).await;
    assert_process_and_group_absent(pid);
}

async fn assert_cleanup_error_kills_and_confirms_reap_for_test(
    inject_error: fn(&mut OwnedMcpChild),
) {
    let mut command = sleeping_child_command();
    let mut owner = Some(OwnedMcpChild::spawn(&mut command).unwrap());
    let pid = owner.as_ref().unwrap().id();
    inject_error(owner.as_mut().unwrap());
    assert_eq!(
        owner
            .as_mut()
            .unwrap()
            .shutdown_and_reap(CleanupBudget::with_timeout(Duration::from_secs(1)))
            .await,
        Err(LiveCode::ChildReapFailed)
    );
    assert!(
        owner.is_some(),
        "cleanup failure must retain the owner slot"
    );
    assert!(
        owner.as_ref().unwrap().group_kill_sent,
        "probe failure must still kill while child authority is retained"
    );
    assert_process_and_group_absent(pid);
    explicitly_reap_test_owner(&mut owner).await;
}

pub(super) async fn assert_owned_child_exit_paths_for_test() {
    assert_startup_failure_retains_explicit_owner_for_test().await;
    assert_cleanup_error_kills_and_confirms_reap_for_test(
        OwnedMcpChild::inject_wait_error_once_for_test,
    )
    .await;
    assert_cleanup_error_kills_and_confirms_reap_for_test(
        OwnedMcpChild::inject_waitability_error_once_for_test,
    )
    .await;

    let mut normal = Command::new("/bin/sh");
    normal
        .arg("-c")
        .arg("exit 0")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut owned = OwnedMcpChild::spawn(&mut normal).unwrap();
    let pid = owned.id();
    owned
        .shutdown_and_reap(CleanupBudget::with_timeout(Duration::from_secs(1)))
        .await
        .unwrap();
    assert!(
        owned.group_kill_sent,
        "normal cleanup must signal the group before reaping the leader"
    );
    assert_process_and_group_absent(pid);
}

pub(super) async fn assert_owned_child_timeout_for_test() {
    let mut command = sleeping_child_command();
    let mut owned = OwnedMcpChild::spawn(&mut command).unwrap();
    let pid = owned.id();
    owned
        .shutdown_and_reap(CleanupBudget::with_timeout(Duration::from_millis(400)))
        .await
        .unwrap();
    assert_process_and_group_absent(pid);

    let mut expired_command = sleeping_child_command();
    let mut expired_owner = Some(OwnedMcpChild::spawn(&mut expired_command).unwrap());
    let expired_pid = expired_owner.as_ref().unwrap().id();
    let expired_budget = CleanupBudget::with_timeout(Duration::from_millis(1));
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        expired_owner
            .as_mut()
            .unwrap()
            .shutdown_and_reap(expired_budget)
            .await,
        Err(LiveCode::ChildReapFailed)
    );
    assert!(
        expired_owner.is_some(),
        "expired cleanup must retain explicit ownership"
    );
    explicitly_reap_test_owner(&mut expired_owner).await;
    assert_process_and_group_absent(expired_pid);
}

pub(super) async fn assert_deadline_and_framing_contract_for_test() {
    assert_actual_mcp_deadline_paths_for_test().await;
    tokio::time::pause();
    let cleanup = CleanupBudget::with_timeout(Duration::from_millis(100));
    let original_deadline = cleanup.deadline.at;
    tokio::time::advance(Duration::from_millis(80)).await;
    assert_eq!(
        cleanup.sleep(Duration::from_millis(30)).await,
        Err(LiveCode::DeadlineExceeded)
    );
    assert_eq!(cleanup.deadline.at, original_deadline);
    assert_eq!(cleanup.check(), Err(LiveCode::DeadlineExceeded));

    let wait_request = Deadline::after(WAIT_REQUEST_TIMEOUT);
    assert_eq!(wait_request.native_wait_timeout_ms().unwrap(), 120_000);
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(wait_request.native_wait_timeout_ms().unwrap(), 118_000);
    assert_eq!(cleanup.deadline.at, original_deadline);

    let (mut writer, reader) = tokio::io::duplex(64);
    writer.write_all(b"123456789\n{\"id\":1}\n").await.unwrap();
    drop(writer);
    let mut reader = BufReader::new(reader);
    let frame_deadline = Deadline::after(Duration::from_secs(1));
    assert_eq!(
        read_bounded_line(&mut reader, frame_deadline, 8).await,
        Err(LiveCode::ToolFailed)
    );
    assert_eq!(
        read_bounded_line(&mut reader, frame_deadline, 8)
            .await
            .unwrap(),
        b"{\"id\":1}"
    );
}

async fn assert_actual_mcp_deadline_paths_for_test() {
    let startup_probe = McpDeadlineProbe::blocking_at(8, Duration::from_secs(2));
    let startup_deadline = Deadline::after(Duration::from_secs(1));
    let mut startup_owner = None;
    assert!(matches!(
        McpClient::start_with_deadline_probe(
            &mut startup_owner,
            startup_deadline,
            startup_probe.clone(),
        )
        .await,
        Err(LiveCode::DeadlineExceeded)
    ));
    let startup_observed = startup_probe.snapshot();
    assert!(
        startup_observed.len() > 5,
        "startup did not reach notification"
    );
    assert!(
        startup_observed
            .iter()
            .all(|(_, deadline)| *deadline == startup_deadline.at),
        "startup request and notification reset the absolute deadline"
    );
    explicitly_reap_test_owner(&mut startup_owner).await;

    let mut owner = None;
    let mut client = McpClient::start_with_deadline_probe(
        &mut owner,
        Deadline::after(Duration::from_secs(2)),
        McpDeadlineProbe::non_blocking(),
    )
    .await
    .unwrap();

    let request_probe = McpDeadlineProbe::blocking_at(5, Duration::from_secs(2));
    client.deadline_probe = Some(request_probe.clone());
    let request_deadline = Deadline::after(Duration::from_secs(1));
    assert_eq!(
        client
            .request_with_deadline("tools/list", json!({}), request_deadline)
            .await,
        Err(LiveCode::DeadlineExceeded)
    );
    let request_observed = request_probe.snapshot();
    assert_eq!(
        request_observed
            .iter()
            .map(|(stage, _)| *stage)
            .collect::<Vec<_>>(),
        vec![
            McpDeadlineStage::Serialize,
            McpDeadlineStage::Write,
            McpDeadlineStage::Flush,
            McpDeadlineStage::Read,
            McpDeadlineStage::Decode,
        ]
    );
    assert!(
        request_observed
            .iter()
            .all(|(_, deadline)| *deadline == request_deadline.at),
        "request stages reset the absolute deadline"
    );

    let notification_probe = McpDeadlineProbe::blocking_at(3, Duration::from_secs(2));
    client.deadline_probe = Some(notification_probe.clone());
    let notification_deadline = Deadline::after(Duration::from_secs(1));
    assert_eq!(
        client
            .notification_with_deadline("notifications/test", json!({}), notification_deadline)
            .await,
        Err(LiveCode::DeadlineExceeded)
    );
    let notification_observed = notification_probe.snapshot();
    assert_eq!(
        notification_observed
            .iter()
            .map(|(stage, _)| *stage)
            .collect::<Vec<_>>(),
        vec![
            McpDeadlineStage::Serialize,
            McpDeadlineStage::Write,
            McpDeadlineStage::Flush,
        ]
    );
    assert!(
        notification_observed
            .iter()
            .all(|(_, deadline)| *deadline == notification_deadline.at),
        "notification stages reset the absolute deadline"
    );
    client.close_stdin();
    explicitly_reap_test_owner(&mut owner).await;
}

#[derive(Debug)]
enum EndpointSource {
    Desktop {
        deterministic_sockets: Option<Mutex<VecDeque<PathBuf>>>,
    },
    Fixed(PathBuf),
}

impl EndpointSource {
    fn resolve(&self) -> io::Result<ResolvedEndpoint> {
        match self {
            Self::Desktop {
                deterministic_sockets: Some(sockets),
            } => {
                let socket = sockets
                    .lock()
                    .map_err(|_| socket_validation_error())?
                    .pop_front()
                    .ok_or_else(socket_validation_error)?;
                self.resolve_with(|name| {
                    (name == BRIDGE_SOCKET_ENV).then(|| socket.as_os_str().to_owned())
                })
            }
            Self::Desktop {
                deterministic_sockets: None,
            } => self.resolve_with(std::env::var_os::<&'static str>),
            Self::Fixed(socket_path) => Ok(ResolvedEndpoint::explicit(socket_path.clone())),
        }
    }

    fn resolve_with(
        &self,
        lookup: impl FnMut(&'static str) -> Option<std::ffi::OsString>,
    ) -> io::Result<ResolvedEndpoint> {
        match self {
            Self::Desktop { .. } => resolve_endpoint_with(lookup).map_err(|error| match error {
                EndpointResolutionError::TargetUnavailable | EndpointResolutionError::Rejected => {
                    socket_validation_error()
                }
            }),
            Self::Fixed(socket_path) => Ok(ResolvedEndpoint::explicit(socket_path.clone())),
        }
    }
}

pub(super) fn resolve_desktop_endpoint_for_test(
    lookup: impl FnMut(&'static str) -> Option<std::ffi::OsString>,
) -> io::Result<ResolvedEndpoint> {
    EndpointSource::Desktop {
        deterministic_sockets: None,
    }
    .resolve_with(lookup)
}

fn selected_directory_metadata(path: &Path) -> io::Result<EndpointMetadata> {
    let metadata = fs::symlink_metadata(path).map_err(|_| socket_validation_error())?;
    Ok(EndpointMetadata {
        owner: metadata.uid(),
        mode: metadata.mode(),
        expected_type: metadata.file_type().is_dir(),
    })
}

fn validate_native_endpoint(endpoint: &ResolvedEndpoint) -> io::Result<()> {
    if !is_normalized_absolute_path(endpoint.socket_path()) {
        return Err(socket_validation_error());
    }
    let derived = match endpoint.derived_directories() {
        Some([runtime, app, bridge]) => Some([
            selected_directory_metadata(runtime)?,
            selected_directory_metadata(app)?,
            selected_directory_metadata(bridge)?,
        ]),
        None => None,
    };
    let parent = endpoint
        .socket_path()
        .parent()
        .ok_or_else(socket_validation_error)?;
    let parent_metadata = selected_directory_metadata(parent)?;
    let parent_is_canonical = fs::canonicalize(parent)
        .map_err(|_| socket_validation_error())?
        .as_os_str()
        == parent.as_os_str();
    let socket =
        fs::symlink_metadata(endpoint.socket_path()).map_err(|_| socket_validation_error())?;
    if endpoint_metadata_is_safe(
        endpoint,
        rustix::process::geteuid().as_raw(),
        derived,
        parent_metadata,
        parent_is_canonical,
        EndpointMetadata {
            owner: socket.uid(),
            mode: socket.mode(),
            expected_type: socket.file_type().is_socket(),
        },
    ) {
        Ok(())
    } else {
        Err(socket_validation_error())
    }
}

fn socket_validation_error() -> io::Error {
    io::Error::other("socket_validation")
}

pub(super) struct LiveHarness {
    workspace: PathBuf,
    endpoint_source: EndpointSource,
    mcp: Option<McpClient>,
    mcp_child: Option<OwnedMcpChild>,
}

impl LiveHarness {
    pub(super) fn from_ledger(workspace: &Path) -> Result<Self, LiveCode> {
        Ok(Self {
            workspace: workspace.to_path_buf(),
            endpoint_source: EndpointSource::Desktop {
                deterministic_sockets: None,
            },
            mcp: None,
            mcp_child: None,
        })
    }

    pub(super) fn for_test_desktop_sockets(
        workspace: PathBuf,
        sockets: Vec<PathBuf>,
    ) -> io::Result<Self> {
        if sockets.is_empty() {
            return Err(socket_validation_error());
        }
        Ok(Self {
            workspace,
            endpoint_source: EndpointSource::Desktop {
                deterministic_sockets: Some(Mutex::new(sockets.into())),
            },
            mcp: None,
            mcp_child: None,
        })
    }

    pub(super) fn for_test_native_socket(workspace: PathBuf, socket: PathBuf) -> io::Result<Self> {
        Ok(Self {
            workspace,
            endpoint_source: EndpointSource::Fixed(socket),
            mcp: None,
            mcp_child: None,
        })
    }

    pub(super) async fn start_mcp(&mut self) -> Result<(), LiveCode> {
        let client = McpClient::start(&mut self.mcp_child).await?;
        self.mcp = Some(client);
        Ok(())
    }

    pub(super) fn mcp_mut(&mut self) -> Result<&mut McpClient, LiveCode> {
        self.mcp.as_mut().ok_or(LiveCode::ToolFailed)
    }

    pub(super) async fn assert_exact_catalog(&mut self) -> Result<(), LiveCode> {
        let listed = self.mcp_mut()?.list_tools().await?;
        let names = listed
            .get("tools")
            .and_then(Value::as_array)
            .ok_or(LiveCode::ToolFailed)?
            .iter()
            .map(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .ok_or(LiveCode::ToolFailed)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if names
            != SESSION_CONTROL_TOOLS
                .into_iter()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        {
            return Err(LiveCode::ToolFailed);
        }
        Ok(())
    }

    pub(super) async fn assert_empty_workspace_before_mutation(&mut self) -> Result<(), LiveCode> {
        let workspace = self.workspace.clone();
        for archived in [false, true] {
            let listed = self.mcp_mut()?.list_threads(&workspace, archived).await?;
            let threads = listed
                .get("threads")
                .and_then(Value::as_array)
                .ok_or(LiveCode::ToolFailed)?;
            if !threads.is_empty() {
                return Err(LiveCode::ToolFailed);
            }
        }
        Ok(())
    }

    pub(super) async fn assert_supported_native_version(&self) -> Result<(), LiveCode> {
        let budget = CleanupBudget::with_timeout(STARTUP_TIMEOUT);
        let mut native = self.connect_native(budget).await?;
        native.initialize(budget).await?;
        native.recovery_home()?;
        native.finish_initialization(budget).await?;
        native.shutdown(budget).await
    }

    pub(super) async fn connect_native(
        &self,
        budget: CleanupBudget,
    ) -> Result<NativeConnection, LiveCode> {
        budget.check()?;
        let endpoint = self
            .endpoint_source
            .resolve()
            .map_err(|_| LiveCode::EndpointRejected)?;
        NativeConnection::connect(&endpoint, budget).await
    }

    pub(super) async fn workspace_thread_ids(
        &self,
        native: &mut NativeConnection,
        archived: bool,
        budget: CleanupBudget,
    ) -> Result<Vec<String>, LiveCode> {
        let mut pages =
            WorkspacePages::new(&self.workspace).map_err(|_| LiveCode::ArchiveProofFailed)?;
        let mut cursor = None;
        loop {
            let page = native
                .request(
                    "thread/list",
                    exact_workspace_list_params(&self.workspace, archived, cursor.as_deref()),
                    budget,
                )
                .await?;
            let next = pages.add(&page).map_err(|_| LiveCode::ArchiveProofFailed)?;
            match next {
                Some(next) if cursor.as_deref() != Some(next.as_str()) => cursor = Some(next),
                Some(_) => return Err(LiveCode::ArchiveProofFailed),
                None => return Ok(pages.into_ids()),
            }
        }
    }

    pub(super) async fn stop_and_reap_mcp_child(
        &mut self,
        budget: CleanupBudget,
    ) -> Result<(), LiveCode> {
        if let Some(mcp) = self.mcp.as_mut() {
            mcp.close_stdin();
        }
        let Some(child) = self.mcp_child.as_mut() else {
            return Ok(());
        };
        child.shutdown_and_reap(budget).await?;
        self.mcp_child.take();
        self.mcp.take();
        Ok(())
    }
}

fn require_successful_child_wait(
    wait_result: io::Result<std::process::ExitStatus>,
) -> io::Result<()> {
    wait_result.map(|_| ())
}

pub(super) fn child_wait_error_is_rejected_for_test() -> bool {
    require_successful_child_wait(Err(io::Error::other("injected child wait failure"))).is_err()
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
            .ok_or_else(|| io::Error::other("thread/list emitted a malformed page"))?;
        let next = match page.get("nextCursor") {
            None | Some(Value::Null) => None,
            Some(cursor) => Some(
                cursor
                    .as_str()
                    .filter(|cursor| !cursor.is_empty() && cursor.len() <= 512)
                    .filter(|cursor| !cursor.bytes().any(|byte| byte.is_ascii_control()))
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| io::Error::other("thread/list emitted an invalid cursor"))?,
            ),
        };
        if data.is_empty() {
            if self.pages != 1 || next.is_some() {
                return Err(io::Error::other(
                    "thread/list emitted an empty continuation page",
                ));
            }
            return Ok(None);
        }
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
        if let Some(cursor) = &next
            && !self.cursors.insert(cursor.clone())
        {
            return Err(io::Error::other("thread/list repeated a cursor"));
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
    identity: Option<InitializedIdentity>,
}

impl NativeConnection {
    async fn connect(endpoint: &ResolvedEndpoint, budget: CleanupBudget) -> Result<Self, LiveCode> {
        budget.check()?;
        validate_native_endpoint(endpoint).map_err(|_| LiveCode::EndpointRejected)?;
        let stream = budget
            .deadline
            .run_io(
                UnixStream::connect(endpoint.socket_path()),
                LiveCode::EndpointRejected,
            )
            .await?;
        let (websocket, _) = budget
            .deadline
            .run(async {
                client_async("ws://localhost/rpc", stream)
                    .await
                    .map_err(|_| LiveCode::EndpointRejected)
            })
            .await?;
        Ok(Self {
            websocket,
            next_id: 1,
            identity: None,
        })
    }

    pub(super) async fn initialize(&mut self, budget: CleanupBudget) -> Result<(), LiveCode> {
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
                budget,
            )
            .await?;
        let identity = InitializedIdentity::from_initialize(
            initialized.get("codexHome").and_then(Value::as_str),
            initialized.get("userAgent").and_then(Value::as_str),
        );
        self.identity = Some(identity);
        Ok(())
    }

    pub(super) fn recovery_home(&self) -> Result<&Path, LiveCode> {
        self.identity
            .as_ref()
            .ok_or(LiveCode::IdentityUnverified)?
            .recovery_home(EXPECTED_CODEX_VERSION)
            .map_err(|code| match code {
                "version_unsupported" => LiveCode::VersionUnsupported,
                _ => LiveCode::IdentityUnverified,
            })
    }

    pub(super) async fn finish_initialization(
        &mut self,
        budget: CleanupBudget,
    ) -> Result<(), LiveCode> {
        budget
            .deadline
            .run(async {
                self.websocket
                    .send(Message::text(json!({"method": "initialized"}).to_string()))
                    .await
                    .map_err(|_| LiveCode::ArchiveProofFailed)
            })
            .await?;
        Ok(())
    }

    pub(super) async fn request(
        &mut self,
        _method: &str,
        params: impl serde::Serialize,
        budget: CleanupBudget,
    ) -> Result<Value, LiveCode> {
        budget.check()?;
        let id = self.next_id;
        self.next_id += 1;
        let message = serde_json::to_string(&json!({
            "id": id,
            "method": _method,
            "params": params
        }))
        .map_err(|_| LiveCode::ArchiveProofFailed)?;
        budget.check()?;
        budget
            .deadline
            .run(async {
                self.websocket
                    .send(Message::text(message))
                    .await
                    .map_err(|_| LiveCode::ArchiveProofFailed)
            })
            .await?;
        budget
            .deadline
            .run(async {
                loop {
                    let frame = self
                        .websocket
                        .next()
                        .await
                        .ok_or(LiveCode::ArchiveProofFailed)?
                        .map_err(|_| LiveCode::ArchiveProofFailed)?;
                    let Message::Text(text) = frame else {
                        continue;
                    };
                    let value: Value = serde_json::from_str(text.as_str())
                        .map_err(|_| LiveCode::ArchiveProofFailed)?;
                    budget.check()?;
                    if value.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if value.get("error").is_some() {
                        return Err(LiveCode::ArchiveProofFailed);
                    }
                    return value
                        .get("result")
                        .cloned()
                        .ok_or(LiveCode::ArchiveProofFailed);
                }
            })
            .await
    }

    pub(super) async fn shutdown(&mut self, budget: CleanupBudget) -> Result<(), LiveCode> {
        budget
            .deadline
            .run(async {
                self.websocket
                    .close(None)
                    .await
                    .map_err(|_| LiveCode::ArchiveProofFailed)
            })
            .await
    }
}

pub(super) struct McpClient {
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    deadline_probe: Option<McpDeadlineProbe>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum McpDeadlineStage {
    Serialize,
    Write,
    Flush,
    Read,
    Decode,
}

#[derive(Clone)]
struct McpDeadlineProbe {
    block_at: Option<usize>,
    block_for: Duration,
    observed: Arc<Mutex<Vec<(McpDeadlineStage, Instant)>>>,
}

impl McpDeadlineProbe {
    fn blocking_at(block_at: usize, block_for: Duration) -> Self {
        Self {
            block_at: Some(block_at),
            block_for,
            observed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn non_blocking() -> Self {
        Self {
            block_at: None,
            block_for: Duration::ZERO,
            observed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn enter(&self, stage: McpDeadlineStage, deadline: Deadline) -> Result<(), LiveCode> {
        let observed_count = {
            let mut observed = self
                .observed
                .lock()
                .expect("deadline probe lock is not poisoned");
            observed.push((stage, deadline.at));
            observed.len()
        };
        let delay = if self.block_at == Some(observed_count) {
            self.block_for
        } else {
            Duration::ZERO
        };
        deadline
            .run(async {
                tokio::time::sleep(delay).await;
                Ok(())
            })
            .await
    }

    fn snapshot(&self) -> Vec<(McpDeadlineStage, Instant)> {
        self.observed
            .lock()
            .expect("deadline probe lock is not poisoned")
            .clone()
    }
}

fn project_tool_call_result(result: &Value) -> Result<Value, LiveCode> {
    if result.get("isError") == Some(&Value::Bool(true)) {
        return Err(LiveCode::ToolFailed);
    }
    result
        .get("structuredContent")
        .cloned()
        .ok_or(LiveCode::ToolFailed)
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
    async fn enter_deadline_stage(
        &self,
        stage: McpDeadlineStage,
        deadline: Deadline,
    ) -> Result<(), LiveCode> {
        match &self.deadline_probe {
            Some(probe) => probe.enter(stage, deadline).await,
            None => deadline.check(),
        }
    }

    async fn start(owner: &mut Option<OwnedMcpChild>) -> Result<Self, LiveCode> {
        Self::start_with_optional_probe(owner, Deadline::after(STARTUP_TIMEOUT), None).await
    }

    async fn start_with_deadline_probe(
        owner: &mut Option<OwnedMcpChild>,
        startup: Deadline,
        deadline_probe: McpDeadlineProbe,
    ) -> Result<Self, LiveCode> {
        Self::start_with_optional_probe(owner, startup, Some(deadline_probe)).await
    }

    async fn start_with_optional_probe(
        owner: &mut Option<OwnedMcpChild>,
        startup: Deadline,
        deadline_probe: Option<McpDeadlineProbe>,
    ) -> Result<Self, LiveCode> {
        let binary = cargo_bin("codex-session-control");
        if !binary.is_file() {
            return Err(LiveCode::ChildSpawnFailed);
        }
        let mut command = Command::new(binary);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        Self::start_command(owner, &mut command, startup, deadline_probe).await
    }

    async fn start_command(
        owner: &mut Option<OwnedMcpChild>,
        command: &mut Command,
        startup: Deadline,
        deadline_probe: Option<McpDeadlineProbe>,
    ) -> Result<Self, LiveCode> {
        if owner.is_some() {
            return Err(LiveCode::ChildSpawnFailed);
        }
        *owner = Some(OwnedMcpChild::spawn(command)?);
        let child = owner
            .as_mut()
            .expect("spawned MCP child authority is installed before fallible setup");
        let stdin = child.take_stdin().ok_or(LiveCode::ChildSpawnFailed)?;
        let stdout = child.take_stdout().ok_or(LiveCode::ChildSpawnFailed)?;
        let mut client = Self {
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
            deadline_probe,
        };
        client
            .request_with_deadline(
                "initialize",
                json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "live-all-tools", "version": "1.0.0"},
                }),
                startup,
            )
            .await?;
        client
            .notification_with_deadline("notifications/initialized", json!({}), startup)
            .await?;
        Ok(client)
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    pub(super) async fn list_tools(&mut self) -> Result<Value, LiveCode> {
        self.request("tools/list", json!({})).await
    }

    pub(super) async fn list_threads(
        &mut self,
        workspace: &Path,
        archived: bool,
    ) -> Result<Value, LiveCode> {
        self.call_tool(
            "threads_list",
            json!({"cwd": workspace, "archived": archived}),
        )
        .await
    }

    pub(super) async fn create_thread(&mut self, workspace: &Path) -> Result<Value, LiveCode> {
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
    ) -> Result<Value, LiveCode> {
        self.call_tool(
            "thread_fork",
            json!({"threadId": owned_id.as_str(), "deferGoalContinuation": false}),
        )
        .await
    }

    pub(super) async fn read_thread(
        &mut self,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, LiveCode> {
        self.call_tool("thread_read", json!({"threadId": owned_id.as_str()}))
            .await
    }

    pub(super) async fn wait_threads(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, LiveCode> {
        let deadline = Deadline::after(WAIT_REQUEST_TIMEOUT);
        let timeout_ms = deadline.native_wait_timeout_ms()?;
        self.call_tool_request(
            tool_call_params(
                "threads_wait",
                json!({"threadIds": [owned_id.as_str()], "timeoutMs": timeout_ms}),
                Some(caller.as_str()),
            ),
            deadline,
        )
        .await
    }

    pub(super) async fn send_message(
        &mut self,
        caller: &OwnedThreadId,
        owned_id: &OwnedThreadId,
    ) -> Result<Value, LiveCode> {
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

    pub(super) async fn set_title(&mut self, owned_id: &OwnedThreadId) -> Result<Value, LiveCode> {
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
    ) -> Result<Value, LiveCode> {
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
    ) -> Result<Value, LiveCode> {
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
    ) -> Result<Value, LiveCode> {
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
    ) -> Result<Value, LiveCode> {
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
    ) -> Result<Value, LiveCode> {
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
    ) -> Result<Value, LiveCode> {
        self.call_tool_as(
            caller,
            "thread_interrupt",
            json!({"threadId": owned_id.as_str(), "includeDescendants": false}),
        )
        .await
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, LiveCode> {
        let deadline = Deadline::after(REQUEST_TIMEOUT);
        self.call_tool_request(tool_call_params(name, arguments, None), deadline)
            .await
    }

    async fn call_tool_as(
        &mut self,
        caller: &OwnedThreadId,
        name: &str,
        arguments: Value,
    ) -> Result<Value, LiveCode> {
        let deadline = Deadline::after(REQUEST_TIMEOUT);
        self.call_tool_request(
            tool_call_params(name, arguments, Some(caller.as_str())),
            deadline,
        )
        .await
    }

    async fn call_tool_request(
        &mut self,
        params: Value,
        deadline: Deadline,
    ) -> Result<Value, LiveCode> {
        let response = self
            .request_with_deadline(TOOLS_CALL_METHOD, params, deadline)
            .await?;
        project_tool_call_result(&response)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, LiveCode> {
        self.request_with_deadline(method, params, Deadline::after(REQUEST_TIMEOUT))
            .await
    }

    async fn request_with_deadline(
        &mut self,
        method: &str,
        params: Value,
        deadline: Deadline,
    ) -> Result<Value, LiveCode> {
        deadline.check()?;
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.enter_deadline_stage(McpDeadlineStage::Serialize, deadline)
            .await?;
        let mut encoded = serde_json::to_vec(&message).map_err(|_| LiveCode::ToolFailed)?;
        deadline.check()?;
        if encoded.len() >= MAX_MCP_FRAME_BYTES {
            return Err(LiveCode::ToolFailed);
        }
        encoded.push(b'\n');
        self.enter_deadline_stage(McpDeadlineStage::Write, deadline)
            .await?;
        {
            let stdin = self.stdin.as_mut().ok_or(LiveCode::ToolFailed)?;
            deadline
                .run_io(stdin.write_all(&encoded), LiveCode::ToolFailed)
                .await?;
        }
        self.enter_deadline_stage(McpDeadlineStage::Flush, deadline)
            .await?;
        let stdin = self.stdin.as_mut().ok_or(LiveCode::ToolFailed)?;
        deadline.run_io(stdin.flush(), LiveCode::ToolFailed).await?;
        loop {
            self.enter_deadline_stage(McpDeadlineStage::Read, deadline)
                .await?;
            let line = read_bounded_line(&mut self.stdout, deadline, MAX_MCP_FRAME_BYTES).await?;
            deadline.check()?;
            self.enter_deadline_stage(McpDeadlineStage::Decode, deadline)
                .await?;
            let value: Value = serde_json::from_slice(&line).map_err(|_| LiveCode::ToolFailed)?;
            deadline.check()?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if value.get("error").is_some() {
                return Err(LiveCode::ToolFailed);
            }
            return value.get("result").cloned().ok_or(LiveCode::ToolFailed);
        }
    }

    async fn notification_with_deadline(
        &mut self,
        method: &str,
        params: Value,
        deadline: Deadline,
    ) -> Result<(), LiveCode> {
        deadline.check()?;
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.enter_deadline_stage(McpDeadlineStage::Serialize, deadline)
            .await?;
        let mut encoded = serde_json::to_vec(&message).map_err(|_| LiveCode::ToolFailed)?;
        deadline.check()?;
        if encoded.len() >= MAX_MCP_FRAME_BYTES {
            return Err(LiveCode::ToolFailed);
        }
        encoded.push(b'\n');
        self.enter_deadline_stage(McpDeadlineStage::Write, deadline)
            .await?;
        {
            let stdin = self.stdin.as_mut().ok_or(LiveCode::ToolFailed)?;
            deadline
                .run_io(stdin.write_all(&encoded), LiveCode::ToolFailed)
                .await?;
        }
        self.enter_deadline_stage(McpDeadlineStage::Flush, deadline)
            .await?;
        let stdin = self.stdin.as_mut().ok_or(LiveCode::ToolFailed)?;
        deadline.run_io(stdin.flush(), LiveCode::ToolFailed).await?;
        Ok(())
    }
}
