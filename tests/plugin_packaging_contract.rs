use std::{
    env,
    ffi::OsString,
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{Mutex, MutexGuard, OnceLock},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MARKETPLACE_NAME: &str = "codex-session-control-local";
const PLUGIN_NAME: &str = "codex-session-control";
const FORWARDED_ENVIRONMENT: [&str; 3] = [
    "XDG_RUNTIME_DIR",
    "CODEX_LINUX_APP_ID",
    "CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET",
];
const TOOL_NAMES: [&str; 13] = [
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

static INSTALLER_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn installer_lock() -> MutexGuard<'static, ()> {
    INSTALLER_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn installer() -> PathBuf {
    repository_root().join("scripts/install-local-plugin.sh")
}

fn staged_binary() -> PathBuf {
    repository_root().join("plugins/codex-session-control/bin/codex-session-control")
}

fn expected_machine() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "Advanced Micro Devices X86-64",
        "aarch64" => "AArch64",
        architecture => {
            panic!("test host architecture is unsupported by the installer: {architecture}")
        }
    }
}

fn private_tempdir() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("create isolated packaging test root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("make isolated packaging test root private");
    root
}

fn private_directory(path: &Path) {
    fs::create_dir_all(path).expect("create private fixture directory");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("make fixture directory private");
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make fixture executable");
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn current_path() -> OsString {
    env::var_os("PATH").expect("test process PATH must be set")
}

fn current_cargo() -> PathBuf {
    let output = Command::new("sh")
        .args(["-c", "command -v cargo"])
        .output()
        .expect("resolve real cargo executable");
    assert!(
        output.status.success(),
        "cargo must be available for packaging tests"
    );
    PathBuf::from(
        String::from_utf8(output.stdout)
            .expect("cargo path is UTF-8")
            .trim(),
    )
}

fn sha256(path: &Path) -> String {
    hex::encode(Sha256::digest(
        fs::read(path).expect("read executable for digest"),
    ))
}

fn assert_regular_executable(path: &Path) {
    let metadata = fs::symlink_metadata(path).expect("staged executable must exist");
    assert!(
        metadata.file_type().is_file(),
        "{} must be a regular file",
        path.display()
    );
    assert!(
        !metadata.file_type().is_symlink(),
        "{} must not be a symlink",
        path.display()
    );
    assert!(
        metadata.mode() & 0o111 != 0,
        "{} must be executable",
        path.display()
    );
    assert_eq!(
        metadata.mode() & 0o7777,
        0o755,
        "{} has an unsafe staged mode",
        path.display()
    );
}

fn assert_current_machine(path: &Path) {
    let output = Command::new("readelf")
        .args(["--file-header"])
        .arg(path)
        .output()
        .expect("run readelf for staged executable");
    assert!(
        output.status.success(),
        "readelf must inspect the staged executable"
    );
    let header = String::from_utf8(output.stdout).expect("ELF header is UTF-8");
    assert!(
        header
            .lines()
            .any(|line| line.contains("Machine:") && line.contains(expected_machine())),
        "staged executable does not match the current native machine"
    );
}

struct StagedBinaryRestore {
    path: PathBuf,
    original: Option<(Vec<u8>, u32)>,
}

impl StagedBinaryRestore {
    fn replace_with_broken_symlink(target: &Path) -> Self {
        let path = staged_binary();
        let original = match fs::symlink_metadata(&path) {
            Ok(metadata) => Some((
                fs::read(&path).expect("preserve staged executable before symlink fixture"),
                metadata.mode() & 0o7777,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("inspect staged executable before symlink fixture: {error}"),
        };
        if original.is_some() {
            fs::remove_file(&path).expect("replace task-owned staged executable with test symlink");
        }
        std::os::unix::fs::symlink(target, &path)
            .expect("create broken staged executable symlink fixture");
        Self { path, original }
    }
}

impl Drop for StagedBinaryRestore {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        if let Some((bytes, mode)) = &self.original {
            let _ = fs::write(&self.path, bytes);
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(*mode));
        }
    }
}

struct FakeCodex {
    _root: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    state: PathBuf,
}

impl FakeCodex {
    fn new() -> Self {
        let root = private_tempdir();
        let root_path = root.path().to_path_buf();
        let bin = root_path.join("bin");
        let state = root_path.join("state");
        private_directory(&bin);
        private_directory(&state);
        private_directory(&root_path.join("home"));
        private_directory(&root_path.join("codex-home"));
        private_directory(&root_path.join("tmp"));

        let fixture = Self {
            _root: root,
            root: root_path,
            bin,
            state,
        };
        fixture.write_fakes();
        fixture
    }

    fn write_fakes(&self) {
        write_executable(
            &self.bin.join("cargo"),
            "#!/usr/bin/env bash\n\
set -euo pipefail\n\
printf '%s\\n' \"$*\" >> \"$FAKE_CODEX_STATE/cargo.log\"\n\
exec \"$FAKE_REAL_CARGO\" \"$@\"\n",
        );
        write_executable(
            &self.bin.join("codex"),
            "#!/usr/bin/env bash\n\
set -euo pipefail\n\
: \"${FAKE_CODEX_STATE:?}\"\n\
printf '%s\\t%s\\n' \"${MISE_QUIET:-unset}\" \"$*\" >> \"$FAKE_CODEX_STATE/commands.log\"\n\
if [[ \"${MISE_QUIET:-}\" != 1 ]]; then\n\
  printf '%s\\n' 'mise advisory: use MISE_QUIET=1 for machine output'\n\
fi\n\
if [[ \"$#\" -eq 4 && \"$1\" == plugin && \"$2\" == marketplace && \"$3\" == list && \"$4\" == --json ]]; then\n\
  if [[ \"${FAKE_CODEX_INVALID_JSON:-}\" == 1 ]]; then\n\
    printf '%s\\n' '{\"marketplaces\":[]}' 'contaminated machine output'\n\
    exit 0\n\
  fi\n\
  if [[ -f \"$FAKE_CODEX_STATE/marketplace-root\" ]]; then\n\
    jq -cn --arg root \"$(<\"$FAKE_CODEX_STATE/marketplace-root\")\" '{marketplaces:[{name:\"codex-session-control-local\",root:$root}]}'\n\
  else\n\
    printf '%s\\n' '{\"marketplaces\":[]}'\n\
  fi\n\
  exit 0\n\
fi\n\
if [[ \"$#\" -eq 5 && \"$1\" == plugin && \"$2\" == marketplace && \"$3\" == add && \"$5\" == --json ]]; then\n\
  printf '%s\\n' \"$*\" >> \"$FAKE_CODEX_STATE/mutations.log\"\n\
  printf '%s' \"$4\" > \"$FAKE_CODEX_STATE/marketplace-root\"\n\
  printf '%s\\n' '{\"ok\":true}'\n\
  exit 0\n\
fi\n\
if [[ \"$#\" -eq 4 && \"$1\" == plugin && \"$2\" == add && \"$4\" == --json ]]; then\n\
  printf '%s\\n' \"$*\" >> \"$FAKE_CODEX_STATE/mutations.log\"\n\
  printf '%s\\n' '{\"ok\":true}'\n\
  exit 0\n\
fi\n\
printf '%s\\n' 'unexpected fake Codex invocation' >&2\n\
exit 64\n",
        );
    }

    fn set_machine(&self, machine: &str) {
        write_executable(
            &self.bin.join("uname"),
            &format!(
                "#!/usr/bin/env bash\nset -euo pipefail\ntest \"$#\" -eq 1\ntest \"$1\" = -m\nprintf '%s\\n' {machine}\n"
            ),
        );
    }

    fn set_collision_root(&self, root: &Path) {
        fs::write(
            self.state.join("marketplace-root"),
            root.as_os_str().as_encoded_bytes(),
        )
        .expect("seed marketplace collision root");
    }

    fn run_installer(&self, invalid_json: bool) -> Output {
        let inherited_path = current_path();
        let path = env::join_paths(
            std::iter::once(self.bin.clone()).chain(env::split_paths(&inherited_path)),
        )
        .expect("construct fake-tool PATH");
        let mut command = Command::new(installer());
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("CODEX_HOME", self.root.join("codex-home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("FAKE_CODEX_STATE", &self.state)
            .env("FAKE_REAL_CARGO", current_cargo());
        for name in ["CARGO_HOME", "RUSTUP_HOME", "RUSTC"] {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        if invalid_json {
            command.env("FAKE_CODEX_INVALID_JSON", "1");
        }
        command.output().expect("run checkout-local installer")
    }

    fn mutations(&self) -> String {
        fs::read_to_string(self.state.join("mutations.log")).unwrap_or_default()
    }

    fn commands(&self) -> String {
        fs::read_to_string(self.state.join("commands.log")).unwrap_or_default()
    }

    fn cargo_commands(&self) -> String {
        fs::read_to_string(self.state.join("cargo.log")).unwrap_or_default()
    }
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn root_manifests_are_exact_and_versioned() {
    let root = repository_root();
    let marketplace: Value = serde_json::from_slice(
        &fs::read(root.join(".agents/plugins/marketplace.json"))
            .expect("checkout marketplace manifest must exist"),
    )
    .expect("checkout marketplace manifest must be JSON");
    let plugin: Value = serde_json::from_slice(
        &fs::read(root.join("plugins/codex-session-control/.codex-plugin/plugin.json"))
            .expect("legacy plugin manifest must exist"),
    )
    .expect("legacy plugin manifest must be JSON");
    let mcp: Value = serde_json::from_slice(
        &fs::read(root.join("plugins/codex-session-control/.mcp.json"))
            .expect("legacy MCP manifest must exist"),
    )
    .expect("legacy MCP manifest must be JSON");
    let cargo: toml::Value = toml::from_str(
        &fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml must exist"),
    )
    .expect("Cargo.toml must parse");
    let version = cargo["package"]["version"]
        .as_str()
        .expect("Cargo package version must be a string");

    assert_eq!(
        marketplace,
        json!({
            "name": MARKETPLACE_NAME,
            "interface": {"displayName": "Codex session control"},
            "plugins": [{
                "name": PLUGIN_NAME,
                "source": {"source": "local", "path": "./plugins/codex-session-control"},
                "policy": {"installation": "AVAILABLE"},
                "category": "Coding",
            }],
        })
    );
    assert_eq!(
        plugin,
        json!({
            "name": PLUGIN_NAME,
            "version": version,
            "description": "Control Codex sessions via MCP",
            "author": {"name": "Agentlehub"},
            "license": "MIT",
            "mcpServers": "./.mcp.json",
            "interface": {
                "displayName": "Codex session control",
                "shortDescription": "Control Codex sessions via MCP",
                "category": "Coding",
                "capabilities": ["Read", "Write"],
            },
        })
    );
    assert_eq!(
        mcp,
        json!({
            "mcpServers": {
                PLUGIN_NAME: {
                    "type": "stdio",
                    "command": "./bin/codex-session-control",
                    "cwd": ".",
                    "env_vars": FORWARDED_ENVIRONMENT,
                    "tool_timeout_sec": 86460,
                }
            }
        })
    );
    assert_eq!(
        fs::read_to_string(root.join("plugins/codex-session-control/bin/.gitignore"))
            .expect("plugin binary ignore rule must exist"),
        "/codex-session-control\n"
    );
    assert!(
        !root.join("plugins/codex-session-control/mcp.json").exists(),
        "the unsupported Agent Plugins v1 root mcp.json format must remain absent"
    );

    let installer_text = fs::read_to_string(installer()).expect("checkout installer must exist");
    for forbidden in [
        "curl",
        "wget",
        "http://",
        "https://",
        "sha256",
        "checksum",
        "cargo install",
        "systemctl",
        ".local/bin",
        "plugin marketplace remove",
        "rm -rf",
        "ln -s",
    ] {
        assert!(
            !installer_text.contains(forbidden),
            "installer must not contain forbidden {forbidden:?} behavior"
        );
    }
}

#[test]
fn installer_rejects_unsupported_host_before_codex_mutation() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();
    fixture.set_machine("riscv64");

    let output = fixture.run_installer(false);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).expect("installer stderr is UTF-8"),
        "Unsupported architecture: riscv64\n"
    );
    assert!(fixture.commands().is_empty(), "Codex must not be queried");
    assert!(fixture.mutations().is_empty(), "Codex must not be mutated");
    assert!(
        fixture.cargo_commands().is_empty(),
        "unsupported hosts must not start a build"
    );
}

#[test]
fn installer_rejects_broken_staged_symlink_before_build_or_codex_mutation() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();
    let _restore = StagedBinaryRestore::replace_with_broken_symlink(&fixture.root.join("missing"));

    let output = fixture.run_installer(false);

    assert!(
        !output.status.success(),
        "broken staged symlink must fail closed"
    );
    assert!(
        String::from_utf8(output.stderr)
            .expect("installer stderr is UTF-8")
            .ends_with("Staged executable must not be a symlink.\n")
    );
    assert!(fixture.cargo_commands().is_empty());
    assert!(fixture.commands().is_empty());
    assert!(fixture.mutations().is_empty());
}

#[test]
fn installer_builds_locked_and_atomically_stages_native_executable() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();
    let output = fixture.run_installer(false);

    assert_success(&output, "native checkout installer");
    assert_eq!(
        fixture.cargo_commands(),
        "metadata --locked --no-deps --format-version 1\nbuild --release --locked\n",
        "the only build must be the locked native release build after version validation"
    );
    let staged = staged_binary();
    assert_regular_executable(&staged);
    assert_current_machine(&staged);
    assert_eq!(
        sha256(&staged),
        sha256(&repository_root().join("target/release/codex-session-control")),
        "staging must copy the freshly built native executable"
    );
    let bin = staged.parent().expect("staged binary has a bin directory");
    assert!(
        fs::read_dir(bin)
            .expect("read staged binary directory")
            .all(|entry| {
                !entry
                    .expect("read staged binary entry")
                    .file_name()
                    .as_encoded_bytes()
                    .starts_with(b".codex-session-control.")
            }),
        "atomic staging must clean its temporary file"
    );
    let script = fs::read_to_string(installer()).expect("read installer");
    let temporary_stage = script
        .find("mktemp \"$plugin_root/bin/.codex-session-control.XXXXXX\"")
        .expect("installer must create a private staging file");
    let atomic_rename = script
        .find("mv -fT -- \"$stage\" \"$staged_binary\"")
        .expect("installer must atomically rename the staged executable");
    assert!(
        temporary_stage < atomic_rename,
        "installer must create its temporary file before the atomic rename"
    );
}

#[test]
fn installer_same_root_restages_and_does_not_duplicate_marketplace() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();
    assert_success(&fixture.run_installer(false), "first installer run");
    let expected = sha256(&staged_binary());
    fs::write(staged_binary(), b"stale staged executable").expect("make stale staging fixture");
    fs::set_permissions(staged_binary(), fs::Permissions::from_mode(0o755))
        .expect("keep stale staging fixture executable");

    assert_success(&fixture.run_installer(false), "same-root installer rerun");

    assert_eq!(
        sha256(&staged_binary()),
        expected,
        "same-root run must restage"
    );
    let mutations = fixture.mutations();
    assert_eq!(
        mutations
            .lines()
            .filter(|line| line.starts_with("plugin marketplace add "))
            .count(),
        1,
        "same root must be registered once"
    );
    assert_eq!(
        mutations
            .lines()
            .filter(|line| line == &format!("plugin add {PLUGIN_NAME}@{MARKETPLACE_NAME} --json"))
            .count(),
        2,
        "every run must refresh the plugin registration"
    );
}

#[test]
fn installer_rejects_marketplace_collision_before_plugin_mutation() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();
    let collision = fixture.root.join("different-checkout");
    private_directory(&collision);
    fixture.set_collision_root(&collision);

    let output = fixture.run_installer(false);

    assert!(!output.status.success(), "collision must fail closed");
    assert!(
        String::from_utf8(output.stderr)
            .expect("installer stderr is UTF-8")
            .ends_with("Marketplace name already targets another root.\n"),
        "collision error must remain specific after Cargo diagnostics"
    );
    assert!(
        fixture.mutations().is_empty(),
        "collision must happen before marketplace or plugin mutation"
    );
}

#[test]
fn installer_suppresses_mise_advisory_for_machine_json() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();
    let direct = Command::new(fixture.bin.join("codex"))
        .env("FAKE_CODEX_STATE", &fixture.state)
        .args(["plugin", "marketplace", "list", "--json"])
        .output()
        .expect("run unquiet fake Codex");
    assert!(
        String::from_utf8(direct.stdout)
            .expect("fake output is UTF-8")
            .starts_with("mise advisory:"),
        "fake must reproduce the observed mise stdout contamination"
    );
    fs::write(fixture.state.join("commands.log"), b"")
        .expect("reset direct fake invocation record");

    let output = fixture.run_installer(false);

    assert_success(&output, "installer with mise wrapper");
    assert!(
        !String::from_utf8(output.stdout)
            .expect("installer stdout is UTF-8")
            .contains("mise advisory:"),
        "machine JSON must not inherit mise stdout contamination"
    );
    assert!(
        fixture
            .commands()
            .lines()
            .all(|line| line.starts_with("1\t")),
        "every Codex machine invocation must use the one quiet wrapper"
    );
}

#[test]
fn installer_rejects_invalid_machine_json_before_mutation() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();

    let output = fixture.run_installer(true);

    assert!(
        !output.status.success(),
        "contaminated JSON must fail closed"
    );
    assert!(
        String::from_utf8(output.stderr)
            .expect("installer stderr is UTF-8")
            .ends_with("Codex marketplace listing was not valid machine-readable JSON.\n"),
        "invalid JSON must report the fail-closed marketplace error after Cargo diagnostics"
    );
    assert!(
        fixture.mutations().is_empty(),
        "invalid JSON must never be treated as an absent marketplace"
    );
}

fn run_catalog_from(binary: &Path, cwd: &Path) -> Vec<Value> {
    let messages = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "generic-packaging-client", "version": "1.0.0"}
            }
        }),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    ];
    let mut child = Command::new(binary)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn staged generic MCP binary");
    {
        let stdin = child
            .stdin
            .as_mut()
            .expect("generic MCP stdin must be piped");
        for message in messages {
            writeln!(stdin, "{message}").expect("write generic MCP request");
        }
    }
    let output = child
        .wait_with_output()
        .expect("wait for staged generic MCP binary");
    assert!(
        output.status.success(),
        "staged generic MCP binary must exit after stdin EOF"
    );
    String::from_utf8(output.stdout)
        .expect("MCP stdout is UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("MCP stdout must be JSON framing"))
        .collect()
}

fn isolated_codex_command(binary: &Path, home: &Path, codex_home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .env("PATH", current_path())
        .env("HOME", home)
        .env("CODEX_HOME", codex_home);
    command
}

fn write_host_probe(path: &Path, log: &Path, marker: &str) {
    write_executable(
        path,
        &format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n%s\\n%s\\n%s\\n%s\\n%s' {marker:?} \"$PWD\" \"$#\" \"${{XDG_RUNTIME_DIR:-}}\" \"${{CODEX_LINUX_APP_ID:-}}\" \"${{CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET:-}}\" > {}\nwhile IFS= read -r request; do\n  id=$(jq -r '.id // empty' <<<\"$request\")\n  method=$(jq -r '.method // empty' <<<\"$request\")\n  case \"$method\" in\n    initialize) printf '%s\\n' \"{{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":$id,\\\"result\\\":{{\\\"protocolVersion\\\":\\\"2025-11-25\\\",\\\"capabilities\\\":{{\\\"tools\\\":{{}}}},\\\"serverInfo\\\":{{\\\"name\\\":\\\"probe\\\",\\\"version\\\":\\\"1.0.0\\\"}}}}}}\" ;;\n    tools/list) printf '%s\\n' \"{{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":$id,\\\"result\\\":{{\\\"tools\\\":[{{\\\"name\\\":\\\"plugin_probe\\\",\\\"description\\\":\\\"Read-only packaging probe\\\",\\\"inputSchema\\\":{{\\\"type\\\":\\\"object\\\",\\\"properties\\\":{{}}}},\\\"annotations\\\":{{\\\"readOnlyHint\\\":true,\\\"destructiveHint\\\":false,\\\"openWorldHint\\\":false}}}}]}}}}\" ;;\n    tools/call) printf '%s\\n' \"{{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":$id,\\\"result\\\":{{\\\"content\\\":[{{\\\"type\\\":\\\"text\\\",\\\"text\\\":\\\"probe complete\\\"}}]}}}}\" ;;\n  esac\ndone\n",
            shell_quote(log)
        ),
    );
}

fn run_isolated_host_probe(
    binary: &Path,
    home: &Path,
    codex_home: &Path,
    sentinels: &[(&str, PathBuf); 3],
) -> Output {
    let mut command = isolated_codex_command(binary, home, codex_home);
    for (name, value) in sentinels {
        command.env(name, value);
    }
    command
        .args([
            "exec",
            "--ephemeral",
            "--skip-git-repo-check",
            "--json",
            "Use the read-only plugin_probe tool once, then reply with its result.",
        ])
        .output()
        .expect("run isolated read-only Codex probe")
}

fn assert_host_probe(
    log: &Path,
    marker: &str,
    source_probe: &Path,
    codex_home: &Path,
    sentinels: &[(&str, PathBuf); 3],
) -> PathBuf {
    let facts = fs::read_to_string(log).expect("read private probe facts");
    let facts = facts.lines().collect::<Vec<_>>();
    assert_eq!(
        facts.len(),
        6,
        "probe must record contained execution facts"
    );
    assert_eq!(
        facts[0], marker,
        "host cache must refresh the probe payload"
    );
    let probe_cwd = PathBuf::from(facts[1]);
    assert!(probe_cwd.starts_with(codex_home));
    assert_eq!(facts[2], "0", "legacy command must pass no extra argv");
    for (actual, (_, expected)) in facts[3..].iter().zip(sentinels) {
        assert_eq!(Path::new(actual), expected);
    }
    let cached_probe = probe_cwd.join("bin/codex-session-control");
    assert_regular_executable(&cached_probe);
    assert_eq!(sha256(&cached_probe), sha256(source_probe));
    cached_probe
}

#[test]
fn generic_client_initializes_and_lists_exact_catalog_from_another_cwd() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();
    assert_success(
        &fixture.run_installer(false),
        "installer before generic client contract",
    );
    let unrelated = private_tempdir();

    let responses = run_catalog_from(&staged_binary(), unrelated.path());

    let initialize = responses
        .iter()
        .find(|response| response["id"] == json!(1))
        .expect("generic client must receive initialize response");
    assert!(initialize["result"].is_object());
    let list = responses
        .iter()
        .find(|response| response["id"] == json!(2))
        .expect("generic client must receive tools/list response");
    assert_eq!(
        list["result"]["tools"]
            .as_array()
            .expect("tools/list must contain tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name is a string"))
            .collect::<Vec<_>>(),
        TOOL_NAMES
    );
}

#[test]
#[ignore = "requires an explicitly opted-in, isolated Codex CLI 0.149.1 plugin-host contract"]
fn legacy_plugin_host_contract_on_codex_0_149_1() {
    // The real-host implementation is added with the private probe fixture below. Keeping this
    // ignored makes all noninteractive packaging tests hermetic while preserving the required
    // explicit operator gate for the actual Codex plugin cache contract.
    let enabled = env::var("CODEX_SESSION_CONTROL_PLUGIN_HOST_CONTRACT").unwrap_or_default();
    assert_eq!(
        enabled, "1",
        "CODEX_SESSION_CONTROL_PLUGIN_HOST_CONTRACT=1 is required before any host action"
    );
    let binary = env::var_os("CODEX_SESSION_CONTROL_CODEX_0_149_1_BIN")
        .map(PathBuf::from)
        .expect("CODEX_SESSION_CONTROL_CODEX_0_149_1_BIN is required before any host action");
    let auth = env::var_os("CODEX_SESSION_CONTROL_PLUGIN_HOST_AUTH_JSON")
        .map(PathBuf::from)
        .expect("CODEX_SESSION_CONTROL_PLUGIN_HOST_AUTH_JSON is required before any host action");
    assert!(binary.is_absolute(), "direct Codex binary must be absolute");
    assert!(auth.is_absolute(), "auth source must be absolute");
    let binary_metadata = fs::symlink_metadata(&binary).expect("read direct Codex binary metadata");
    assert!(
        binary_metadata.file_type().is_file(),
        "Codex binary must be regular"
    );
    assert!(
        !binary_metadata.file_type().is_symlink(),
        "Codex binary must not be a wrapper symlink"
    );
    assert_eq!(
        binary_metadata.uid(),
        rustix::process::geteuid().as_raw(),
        "Codex binary must be owned by the effective user"
    );
    assert_eq!(
        binary_metadata.mode() & 0o7777,
        0o755,
        "Codex binary must have the exact regular executable mode"
    );
    let version = Command::new(&binary)
        .arg("--version")
        .env_clear()
        .output()
        .expect("run direct Codex version command");
    assert!(
        version.status.success(),
        "direct Codex version command must succeed"
    );
    assert_eq!(
        version.stdout, b"codex-cli 0.149.1\n",
        "direct Codex binary must report exactly codex-cli 0.149.1"
    );
    let auth_metadata = fs::symlink_metadata(&auth).expect("read auth source metadata");
    assert!(
        auth_metadata.file_type().is_file(),
        "auth source must be regular"
    );
    assert!(
        !auth_metadata.file_type().is_symlink(),
        "auth source must not be a symlink"
    );

    let root = private_tempdir();
    let home = root.path().join("home");
    let codex_home = root.path().join("codex-home");
    private_directory(&home);
    private_directory(&codex_home);
    let copied_auth = codex_home.join("auth.json");
    fs::copy(&auth, &copied_auth).expect("copy auth only into isolated Codex state");
    fs::set_permissions(&copied_auth, fs::Permissions::from_mode(0o600))
        .expect("make isolated auth copy private");
    let copied_auth_metadata = fs::symlink_metadata(&copied_auth).expect("read isolated auth copy");
    assert!(copied_auth_metadata.file_type().is_file());
    assert!(!copied_auth_metadata.file_type().is_symlink());
    assert_eq!(copied_auth_metadata.mode() & 0o7777, 0o600);

    let clone = root.path().join("clone");
    let marketplace = clone.join(".agents/plugins/marketplace.json");
    let plugin = clone.join("plugins/codex-session-control");
    private_directory(marketplace.parent().expect("marketplace has parent"));
    private_directory(plugin.join(".codex-plugin").as_path());
    private_directory(plugin.join("bin").as_path());
    fs::copy(
        repository_root().join(".agents/plugins/marketplace.json"),
        &marketplace,
    )
    .expect("copy isolated marketplace manifest");
    fs::copy(
        repository_root().join("plugins/codex-session-control/.codex-plugin/plugin.json"),
        plugin.join(".codex-plugin/plugin.json"),
    )
    .expect("copy isolated legacy plugin manifest");
    fs::copy(
        repository_root().join("plugins/codex-session-control/.mcp.json"),
        plugin.join(".mcp.json"),
    )
    .expect("copy isolated legacy MCP manifest");

    let probe_log = root.path().join("probe.log");
    let source_probe = plugin.join("bin/codex-session-control");
    write_host_probe(&source_probe, &probe_log, "initial");

    let sentinels = [
        ("XDG_RUNTIME_DIR", root.path().join("runtime")),
        ("CODEX_LINUX_APP_ID", root.path().join("app-id-sentinel")),
        (
            "CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET",
            root.path().join("socket-sentinel"),
        ),
    ];
    let mut codex = isolated_codex_command(&binary, &home, &codex_home);
    for (name, value) in &sentinels {
        codex.env(name, value);
    }
    let add_marketplace = codex
        .args(["plugin", "marketplace", "add"])
        .arg(&clone)
        .arg("--json")
        .output()
        .expect("register isolated plugin marketplace");
    assert!(
        add_marketplace.status.success(),
        "isolated marketplace registration must succeed"
    );
    let add_plugin = isolated_codex_command(&binary, &home, &codex_home)
        .args([
            "plugin",
            "add",
            "codex-session-control@codex-session-control-local",
            "--json",
        ])
        .output()
        .expect("install isolated legacy plugin");
    assert!(
        add_plugin.status.success(),
        "isolated plugin install must succeed"
    );

    let exec = run_isolated_host_probe(&binary, &home, &codex_home, &sentinels);
    assert!(
        exec.status.success(),
        "isolated read-only Codex probe must succeed without normal Codex state"
    );
    let initial_cache = assert_host_probe(
        &probe_log,
        "initial",
        &source_probe,
        &codex_home,
        &sentinels,
    );
    assert_ne!(
        initial_cache
            .parent()
            .expect("cache binary has a plugin root"),
        plugin.join("bin"),
        "host must execute the copied plugin cache"
    );
    let initial_digest = sha256(&initial_cache);

    write_host_probe(&source_probe, &probe_log, "same-version-refresh");
    let same_version_add = isolated_codex_command(&binary, &home, &codex_home)
        .args([
            "plugin",
            "add",
            "codex-session-control@codex-session-control-local",
            "--json",
        ])
        .output()
        .expect("refresh isolated plugin with the same manifest version");
    assert!(
        same_version_add.status.success(),
        "same-version plugin refresh must succeed"
    );
    let same_version_probe = run_isolated_host_probe(&binary, &home, &codex_home, &sentinels);
    assert!(
        same_version_probe.status.success(),
        "same-version cache refresh must launch the updated contained executable"
    );
    let same_version_cache = assert_host_probe(
        &probe_log,
        "same-version-refresh",
        &source_probe,
        &codex_home,
        &sentinels,
    );
    let same_version_digest = sha256(&same_version_cache);
    assert_ne!(
        initial_digest, same_version_digest,
        "same-version registration must refresh cached executable bytes"
    );

    let isolated_plugin_manifest = plugin.join(".codex-plugin/plugin.json");
    let mut bumped_plugin: Value = serde_json::from_slice(
        &fs::read(&isolated_plugin_manifest).expect("read isolated plugin manifest"),
    )
    .expect("isolated plugin manifest remains JSON");
    bumped_plugin["version"] = json!("0.3.3");
    fs::write(
        &isolated_plugin_manifest,
        serde_json::to_vec_pretty(&bumped_plugin).expect("serialize bumped plugin manifest"),
    )
    .expect("write bumped isolated plugin manifest");
    write_host_probe(&source_probe, &probe_log, "version-bump-refresh");
    let version_bump_add = isolated_codex_command(&binary, &home, &codex_home)
        .args([
            "plugin",
            "add",
            "codex-session-control@codex-session-control-local",
            "--json",
        ])
        .output()
        .expect("refresh isolated plugin after manifest version bump");
    assert!(
        version_bump_add.status.success(),
        "version-bump plugin refresh must succeed"
    );
    let version_bump_probe = run_isolated_host_probe(&binary, &home, &codex_home, &sentinels);
    assert!(
        version_bump_probe.status.success(),
        "version-bump cache refresh must launch the updated contained executable"
    );
    let version_bump_cache = assert_host_probe(
        &probe_log,
        "version-bump-refresh",
        &source_probe,
        &codex_home,
        &sentinels,
    );
    assert_ne!(
        same_version_digest,
        sha256(&version_bump_cache),
        "version bump must refresh cached executable bytes"
    );

    let remove = isolated_codex_command(&binary, &home, &codex_home)
        .args([
            "plugin",
            "remove",
            "codex-session-control@codex-session-control-local",
            "--json",
        ])
        .output()
        .expect("remove isolated plugin registration");
    assert!(
        remove.status.success(),
        "isolated plugin removal must succeed"
    );
    assert!(
        clone.is_dir(),
        "native removal must retain the checkout clone"
    );
    assert_regular_executable(&source_probe);

    let v1_manifest = plugin.join("mcp.json");
    fs::write(
        plugin.join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name": "codex-session-control",
            "version": "0.3.3",
            "description": "Control Codex sessions via MCP",
        }))
        .expect("serialize isolated Agent Plugins v1 plugin manifest"),
    )
    .expect("write isolated Agent Plugins v1 plugin manifest");
    fs::write(
        &v1_manifest,
        serde_json::to_vec_pretty(&json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
            "mcpServers": {
                "codex-session-control": {
                    "type": "stdio",
                    "command": "./bin/codex-session-control",
                    "cwd": "./",
                }
            }
        }))
        .expect("serialize isolated Agent Plugins v1 root MCP negative control"),
    )
    .expect("write isolated Agent Plugins v1 root MCP negative control");
    let _ = fs::remove_file(&probe_log);
    write_host_probe(&source_probe, &probe_log, "v1-negative-control");
    let v1_add = isolated_codex_command(&binary, &home, &codex_home)
        .args([
            "plugin",
            "add",
            "codex-session-control@codex-session-control-local",
            "--json",
        ])
        .output()
        .expect("attempt isolated Agent Plugins v1 root MCP registration");
    assert!(
        v1_add.status.success(),
        "Codex 0.149.1 must accept the Agent Plugins v1 root MCP negative control"
    );
    let v1_probe = run_isolated_host_probe(&binary, &home, &codex_home, &sentinels);
    assert!(
        v1_probe.status.success(),
        "v1 negative-control probe must complete without normal Codex state"
    );
    let v1_facts = fs::read_to_string(&probe_log).expect("read private v1 negative-control facts");
    let v1_facts = v1_facts.split('\n').collect::<Vec<_>>();
    assert_eq!(v1_facts.len(), 6);
    assert_eq!(v1_facts[0], "v1-negative-control");
    assert!(Path::new(v1_facts[1]).starts_with(&codex_home));
    assert_eq!(v1_facts[2], "0");
    assert!(
        v1_facts[3..].iter().all(|value| value.is_empty()),
        "Agent Plugins v1 root mcp.json must forward none of the required host variables"
    );
}
