#[path = "support/process_guard.rs"]
mod process_guard;

use process_guard::ChildGuard;
use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{Mutex, MutexGuard, OnceLock},
    time::Duration,
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
const GENERIC_MCP_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const INSTALLER_FIXTURE_TIMEOUT: Duration = Duration::from_secs(120);
const CODEX_PLUGIN_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const CODEX_READ_ONLY_PROBE_TIMEOUT: Duration = Duration::from_secs(90);

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
    let mut command = Command::new("sh");
    command.args(["-c", "command -v cargo"]);
    let output = run_bounded_command(
        &mut command,
        GENERIC_MCP_EXIT_TIMEOUT,
        "resolve real Cargo executable",
    )
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
    assert_eq!(
        metadata.mode() & 0o7777,
        0o755,
        "{} has an unsafe staged mode",
        path.display()
    );
}

fn assert_current_machine(path: &Path) {
    let mut command = Command::new("readelf");
    command.args(["--file-header"]).arg(path);
    let output = run_bounded_command(
        &mut command,
        GENERIC_MCP_EXIT_TIMEOUT,
        "inspect staged executable ELF header",
    )
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

fn run_bounded_command(
    command: &mut Command,
    timeout: Duration,
    context: &str,
) -> Result<Output, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = ChildGuard::spawn(command)
        .map_err(|_| format!("{context} could not start within the isolated test"))?;
    match child.wait_with_output(timeout) {
        Ok(output) => Ok(output),
        Err(error) if error.kind() == io::ErrorKind::TimedOut => Err(format!(
            "{context} timed out before producing a complete result"
        )),
        Err(_) => Err(format!(
            "{context} failed while collecting a complete result"
        )),
    }
}

fn verify_direct_codex_prerequisite(binary: &Path) -> Result<(), &'static str> {
    if !binary.is_absolute() {
        return Err("direct Codex binary must be absolute");
    }
    let metadata = fs::symlink_metadata(binary)
        .map_err(|_| "direct Codex binary metadata must be available")?;
    if !metadata.file_type().is_file() {
        return Err("direct Codex binary must be regular");
    }
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err("direct Codex binary must be owned by the effective user");
    }
    if metadata.mode() & 0o7777 != 0o755 {
        return Err("direct Codex binary must have exact mode 0755");
    }

    let mut readelf = Command::new("readelf");
    readelf.args(["--file-header"]).arg(binary);
    let header = run_bounded_command(
        &mut readelf,
        GENERIC_MCP_EXIT_TIMEOUT,
        "direct Codex ELF inspection",
    )
    .map_err(|_| "direct Codex binary must be a native executable for this machine")?;
    if !header.status.success()
        || !String::from_utf8_lossy(&header.stdout)
            .lines()
            .any(|line| line.contains("Machine:") && line.contains(expected_machine()))
    {
        return Err("direct Codex binary must be a native executable for this machine");
    }

    let mut version = Command::new(binary);
    version.arg("--version").env_clear();
    let version = run_bounded_command(
        &mut version,
        GENERIC_MCP_EXIT_TIMEOUT,
        "direct Codex version check",
    )
    .map_err(|_| "direct Codex version check must complete")?;
    if !version.status.success()
        || version.stdout != b"codex-cli 0.149.1\n"
        || !version.stderr.is_empty()
    {
        return Err("direct Codex binary must report exactly codex-cli 0.149.1");
    }
    Ok(())
}

fn copy_isolated_auth_after_verified_binary(
    binary: &Path,
    auth: &Path,
    codex_home: &Path,
) -> Result<(), &'static str> {
    verify_direct_codex_prerequisite(binary)?;
    let auth_metadata =
        fs::symlink_metadata(auth).map_err(|_| "auth source metadata must be available")?;
    if !auth_metadata.file_type().is_file() {
        return Err("auth source must be regular");
    }

    let copied_auth = codex_home.join("auth.json");
    fs::copy(auth, &copied_auth).map_err(|_| "isolated auth copy failed")?;
    fs::set_permissions(&copied_auth, fs::Permissions::from_mode(0o600))
        .map_err(|_| "isolated auth copy mode could not be set")?;
    let copied_metadata = fs::symlink_metadata(&copied_auth)
        .map_err(|_| "isolated auth copy metadata must be available")?;
    if !copied_metadata.file_type().is_file() || copied_metadata.mode() & 0o7777 != 0o600 {
        return Err("isolated auth copy must be a regular mode-0600 file");
    }
    Ok(())
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
  if [[ \"${FAKE_CODEX_NUL_JSON:-}\" == 1 ]]; then\n\
    printf '%s\\0' '{\"marketplaces\":[]}'\n\
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
        self.run_installer_at(&installer(), &self.root, invalid_json)
    }

    fn run_installer_with_nul_json(&self) -> Output {
        self.run_installer_at_with_nul_json(&installer(), &self.root, false, true)
    }

    fn run_installer_at(&self, script: &Path, cwd: &Path, invalid_json: bool) -> Output {
        self.run_installer_at_with_nul_json(script, cwd, invalid_json, false)
    }

    fn run_installer_at_with_nul_json(
        &self,
        script: &Path,
        cwd: &Path,
        invalid_json: bool,
        nul_json: bool,
    ) -> Output {
        let inherited_path = current_path();
        let path = env::join_paths(
            std::iter::once(self.bin.clone()).chain(env::split_paths(&inherited_path)),
        )
        .expect("construct fake-tool PATH");
        let mut command = Command::new(script);
        command
            .current_dir(cwd)
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
        if nul_json {
            command.env("FAKE_CODEX_NUL_JSON", "1");
        }
        run_bounded_command(
            &mut command,
            INSTALLER_FIXTURE_TIMEOUT,
            "run checkout-local installer",
        )
        .expect("run checkout-local installer")
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

struct DisposableClone {
    _root: tempfile::TempDir,
    root: PathBuf,
    script: PathBuf,
    plugin: PathBuf,
}

impl DisposableClone {
    fn new() -> Self {
        let root = private_tempdir();
        let root_path = root.path().join("clone");
        private_directory(&root_path);
        let script = root_path.join("scripts/install-local-plugin.sh");
        private_directory(script.parent().expect("installer script has parent"));
        fs::copy(installer(), &script).expect("copy installer into disposable clone");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("make disposable installer executable");
        let plugin = root_path.join("plugins/codex-session-control");
        private_directory(plugin.join(".codex-plugin").as_path());
        Self {
            _root: root,
            root: root_path,
            script,
            plugin,
        }
    }

    fn copy_manifests(&self) {
        let marketplace = self.root.join(".agents/plugins/marketplace.json");
        private_directory(marketplace.parent().expect("marketplace has parent"));
        fs::copy(
            repository_root().join(".agents/plugins/marketplace.json"),
            marketplace,
        )
        .expect("copy disposable marketplace manifest");
        fs::copy(
            repository_root().join("plugins/codex-session-control/.codex-plugin/plugin.json"),
            self.plugin.join(".codex-plugin/plugin.json"),
        )
        .expect("copy disposable legacy plugin manifest");
        fs::copy(
            repository_root().join("plugins/codex-session-control/.mcp.json"),
            self.plugin.join(".mcp.json"),
        )
        .expect("copy disposable legacy MCP manifest");
    }

    fn manifest_path(&self, manifest: &str) -> PathBuf {
        match manifest {
            "marketplace" => self.root.join(".agents/plugins/marketplace.json"),
            "plugin" => self.plugin.join(".codex-plugin/plugin.json"),
            "mcp" => self.plugin.join(".mcp.json"),
            _ => unreachable!("fixed disposable manifest cases"),
        }
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
    let clone = DisposableClone::new();
    clone.copy_manifests();
    private_directory(clone.plugin.join("bin").as_path());
    std::os::unix::fs::symlink(
        clone.root.join("missing"),
        clone.plugin.join("bin/codex-session-control"),
    )
    .expect("create disposable broken staged executable symlink");

    let output = fixture.run_installer_at(&clone.script, &clone.root, false);

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
fn installer_rejects_multidocument_manifest_streams_before_build_or_codex_mutation() {
    let _serial = installer_lock();

    for manifest in ["marketplace", "plugin", "mcp"] {
        let fixture = FakeCodex::new();
        let clone = DisposableClone::new();
        clone.copy_manifests();
        private_directory(clone.plugin.join("bin").as_path());
        let staged = clone.plugin.join("bin/codex-session-control");
        write_executable(&staged, "#!/usr/bin/env bash\nprintf '%s\\n' untouched\n");
        let before = fs::read(&staged).expect("read disposable staged executable before failure");
        let path = clone.manifest_path(manifest);
        let approved = fs::read(&path).expect("read approved disposable manifest");
        let mut stream = b"{}\n".to_vec();
        stream.extend_from_slice(&approved);
        fs::write(&path, stream).expect("write multi-document disposable manifest stream");

        let output = fixture.run_installer_at(&clone.script, &clone.root, false);

        assert!(
            !output.status.success(),
            "{manifest} stream must fail closed"
        );
        assert!(
            String::from_utf8(output.stderr)
                .expect("installer stderr is UTF-8")
                .ends_with("Plugin manifest is not valid JSON.\n"),
            "{manifest} stream must be rejected before later validation"
        );
        assert!(
            fixture.cargo_commands().is_empty(),
            "{manifest} stream must not start a build"
        );
        assert!(
            fixture.commands().is_empty(),
            "{manifest} stream must not query Codex"
        );
        assert!(
            fixture.mutations().is_empty(),
            "{manifest} stream must not mutate Codex"
        );
        assert_eq!(
            fs::read(&staged).expect("read disposable staged executable after failure"),
            before,
            "{manifest} stream must not replace the staged executable"
        );
    }
}

#[test]
fn installer_rejects_symlinked_manifest_parents_before_build_or_codex_mutation() {
    let _serial = installer_lock();

    for component in [".agents", ".agents/plugins", "plugin/.codex-plugin"] {
        let fixture = FakeCodex::new();
        let clone = DisposableClone::new();
        let escaped = clone
            .root
            .parent()
            .expect("disposable clone has a private parent")
            .join(format!("escaped-{}", component.replace('/', "-")));
        private_directory(&escaped);
        match component {
            ".agents" => {
                private_directory(escaped.join(".agents/plugins").as_path());
                std::os::unix::fs::symlink(escaped.join(".agents"), clone.root.join(".agents"))
                    .expect("create symlinked .agents parent");
            }
            ".agents/plugins" => {
                private_directory(clone.root.join(".agents").as_path());
                private_directory(escaped.join("plugins").as_path());
                std::os::unix::fs::symlink(
                    escaped.join("plugins"),
                    clone.root.join(".agents/plugins"),
                )
                .expect("create symlinked marketplace parent");
            }
            "plugin/.codex-plugin" => {
                fs::remove_dir(clone.plugin.join(".codex-plugin"))
                    .expect("remove empty disposable plugin manifest directory");
                private_directory(escaped.join(".codex-plugin").as_path());
                std::os::unix::fs::symlink(
                    escaped.join(".codex-plugin"),
                    clone.plugin.join(".codex-plugin"),
                )
                .expect("create symlinked plugin manifest parent");
            }
            _ => unreachable!("fixed symlink parent cases"),
        }
        clone.copy_manifests();
        private_directory(clone.plugin.join("bin").as_path());

        let output = fixture.run_installer_at(&clone.script, &clone.root, false);

        assert!(
            !output.status.success(),
            "symlinked {component} must fail closed"
        );
        assert!(
            String::from_utf8(output.stderr)
                .expect("installer stderr is UTF-8")
                .ends_with("Checkout directory must not be a symlink.\n"),
            "symlinked {component} must be rejected before later validation"
        );
        assert!(
            fixture.cargo_commands().is_empty(),
            "symlinked {component} must not start a build"
        );
        assert!(
            fixture.commands().is_empty(),
            "symlinked {component} must not query Codex"
        );
        assert!(
            fixture.mutations().is_empty(),
            "symlinked {component} must not mutate Codex"
        );
    }
}

#[test]
fn installer_rejects_symlinked_bin_before_build_or_codex_mutation() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();
    let clone = DisposableClone::new();
    let escaped_bin = clone
        .root
        .parent()
        .expect("disposable clone has a private parent")
        .join("escaped-bin");
    private_directory(&escaped_bin);
    std::os::unix::fs::symlink(&escaped_bin, clone.plugin.join("bin"))
        .expect("create symlinked plugin bin directory");
    clone.copy_manifests();

    let output = fixture.run_installer_at(&clone.script, &clone.root, false);

    assert!(!output.status.success(), "symlinked bin must fail closed");
    assert!(
        String::from_utf8(output.stderr)
            .expect("installer stderr is UTF-8")
            .ends_with("Checkout directory must not be a symlink.\n"),
        "symlinked bin must be rejected before later validation"
    );
    assert!(
        fixture.cargo_commands().is_empty(),
        "symlinked bin must not start a build"
    );
    assert!(
        fixture.commands().is_empty(),
        "symlinked bin must not query Codex"
    );
    assert!(
        fixture.mutations().is_empty(),
        "symlinked bin must not mutate Codex"
    );
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
    let mut stale_handle = fs::File::open(staged_binary())
        .expect("hold the stale executable open across atomic replacement");

    assert_success(&fixture.run_installer(false), "same-root installer rerun");

    let mut retained_stale_bytes = Vec::new();
    stale_handle
        .read_to_end(&mut retained_stale_bytes)
        .expect("read retained stale executable descriptor");
    assert_eq!(
        retained_stale_bytes, b"stale staged executable",
        "restaging must atomically replace the destination instead of overwriting it in place"
    );

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
    let mut direct_command = Command::new(fixture.bin.join("codex"));
    direct_command
        .env_remove("MISE_QUIET")
        .env("FAKE_CODEX_STATE", &fixture.state)
        .args(["plugin", "marketplace", "list", "--json"]);
    let direct = run_bounded_command(
        &mut direct_command,
        GENERIC_MCP_EXIT_TIMEOUT,
        "run unquiet fake Codex",
    )
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

#[test]
fn installer_rejects_nul_suffixed_machine_json_before_mutation() {
    let _serial = installer_lock();
    let fixture = FakeCodex::new();

    let output = fixture.run_installer_with_nul_json();

    assert!(
        !output.status.success(),
        "NUL-suffixed JSON must fail closed"
    );
    assert!(
        String::from_utf8(output.stderr)
            .expect("installer stderr is UTF-8")
            .ends_with("Codex marketplace listing was not valid machine-readable JSON.\n"),
        "NUL-suffixed JSON must report the fail-closed marketplace error"
    );
    assert!(
        fixture.mutations().is_empty(),
        "NUL-suffixed JSON must never trigger marketplace or plugin mutation"
    );
}

fn run_catalog_from(binary: &Path, cwd: &Path, timeout: Duration) -> Result<Vec<Value>, String> {
    let mut command = Command::new(binary);
    run_catalog_command(&mut command, cwd, timeout)
}

fn run_catalog_command(
    command: &mut Command,
    cwd: &Path,
    timeout: Duration,
) -> Result<Vec<Value>, String> {
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
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child =
        ChildGuard::spawn(command).map_err(|_| "generic MCP binary could not start".to_owned())?;
    {
        let stdin = child.stdin_mut().expect("generic MCP stdin must be piped");
        for message in messages {
            writeln!(stdin, "{message}").expect("write generic MCP request");
        }
    }
    child.close_stdin();
    let output = match child.wait_with_output(timeout) {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
            return Err("generic MCP binary did not complete within its deadline".to_owned());
        }
        Err(_) => {
            return Err("generic MCP binary failed while collecting a complete result".to_owned());
        }
    };
    if !output.status.success() {
        return Err("generic MCP binary did not exit successfully".to_owned());
    }
    String::from_utf8(output.stdout)
        .map_err(|_| "MCP stdout is not UTF-8".to_owned())?
        .lines()
        .map(|line| {
            serde_json::from_str(line).map_err(|_| "MCP stdout is not JSON framing".to_owned())
        })
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

fn run_isolated_plugin_command(command: &mut Command, context: &str) -> Output {
    run_bounded_command(command, CODEX_PLUGIN_COMMAND_TIMEOUT, context).expect(context)
}

fn parse_single_json_object(bytes: &[u8], context: &str) -> Value {
    let value: Value = serde_json::from_slice(bytes).expect(context);
    assert!(value.is_object(), "{context}");
    value
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
    command.args([
        "exec",
        "--ephemeral",
        "--skip-git-repo-check",
        "--json",
        "Use the read-only plugin_probe tool once, then reply with its result.",
    ]);
    run_bounded_command(
        &mut command,
        CODEX_READ_ONLY_PROBE_TIMEOUT,
        "isolated read-only Codex probe",
    )
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

    let responses = run_catalog_from(&staged_binary(), unrelated.path(), GENERIC_MCP_EXIT_TIMEOUT)
        .expect("generic client must complete within its deadline");

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
fn generic_client_deadline_reaps_a_slow_catalog_process() {
    let root = private_tempdir();
    let mut slow_server = Command::new("sh");
    slow_server.args(["-c", "sleep 1"]);

    let direct_children_before = fs::read_to_string("/proc/thread-self/children")
        .expect("Linux procfs direct-child state must be available");
    let started = std::time::Instant::now();
    let result = run_catalog_command(&mut slow_server, root.path(), Duration::from_millis(100));
    let direct_children_after = fs::read_to_string("/proc/thread-self/children")
        .expect("Linux procfs direct-child state must be available");

    assert_eq!(
        result,
        Err("generic MCP binary did not complete within its deadline".to_owned()),
        "generic clients must not wait indefinitely for a staged process"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "generic client deadline must terminate the slow catalog process promptly"
    );
    assert_eq!(
        direct_children_after, direct_children_before,
        "generic client deadline must reap the slow catalog process"
    );
}

#[test]
fn generic_client_reports_capture_failures_separately_from_timeouts() {
    let root = private_tempdir();
    let mut oversized_server = Command::new("sh");
    oversized_server.args(["-c", "head -c 65537 /dev/zero"]);

    let result = run_catalog_command(&mut oversized_server, root.path(), GENERIC_MCP_EXIT_TIMEOUT);

    assert_eq!(
        result,
        Err("generic MCP binary failed while collecting a complete result".to_owned())
    );
}

#[test]
fn bounded_command_reports_timeouts_without_capturing_process_output() {
    let mut command = Command::new("sh");
    command.args(["-c", "printf '%s' timeout-output-marker >&2; sleep 1"]);

    let result = run_bounded_command(&mut command, Duration::from_millis(100), "timeout fixture");

    assert_eq!(
        result,
        Err("timeout fixture timed out before producing a complete result".to_owned())
    );
}

#[test]
fn bounded_command_reports_capture_failures_separately_from_timeouts() {
    let mut command = Command::new("sh");
    command.args(["-c", "head -c 65537 /dev/zero"]);

    let result = run_bounded_command(&mut command, GENERIC_MCP_EXIT_TIMEOUT, "capture fixture");

    assert_eq!(
        result,
        Err("capture fixture failed while collecting a complete result".to_owned())
    );
}

#[test]
fn real_host_prerequisite_rejects_regular_script_wrapper_before_auth_copy() {
    let root = private_tempdir();
    let wrapper = root.path().join("codex-wrapper");
    let auth = root.path().join("auth.json");
    let codex_home = root.path().join("codex-home");
    write_executable(
        &wrapper,
        "#!/usr/bin/env bash\nprintf '%s\\n' 'codex-cli 0.149.1'\n",
    );
    fs::write(&auth, b"isolated auth fixture").expect("write regular auth fixture");
    fs::set_permissions(&auth, fs::Permissions::from_mode(0o600))
        .expect("make auth fixture private");
    private_directory(&codex_home);

    assert_eq!(
        copy_isolated_auth_after_verified_binary(&wrapper, &auth, &codex_home),
        Err("direct Codex binary must be a native executable for this machine"),
        "a regular executable script must not satisfy the direct native Codex prerequisite"
    );
    assert!(
        !codex_home.join("auth.json").exists(),
        "a rejected wrapper must fail before any isolated auth copy"
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
    assert!(auth.is_absolute(), "auth source must be absolute");

    let root = private_tempdir();
    let home = root.path().join("home");
    let codex_home = root.path().join("codex-home");
    private_directory(&home);
    private_directory(&codex_home);
    copy_isolated_auth_after_verified_binary(&binary, &auth, &codex_home)
        .expect("copy auth only after direct Codex verification into isolated state");

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
    codex
        .args(["plugin", "marketplace", "add"])
        .arg(&clone)
        .arg("--json");
    let add_marketplace =
        run_isolated_plugin_command(&mut codex, "register isolated plugin marketplace");
    assert!(
        add_marketplace.status.success(),
        "isolated marketplace registration must succeed"
    );
    let mut add_plugin_command = isolated_codex_command(&binary, &home, &codex_home);
    add_plugin_command.args([
        "plugin",
        "add",
        "codex-session-control@codex-session-control-local",
        "--json",
    ]);
    let add_plugin =
        run_isolated_plugin_command(&mut add_plugin_command, "install isolated legacy plugin");
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
    let initial_digest = sha256(&initial_cache);

    write_host_probe(&source_probe, &probe_log, "same-version-refresh");
    let mut same_version_command = isolated_codex_command(&binary, &home, &codex_home);
    same_version_command.args([
        "plugin",
        "add",
        "codex-session-control@codex-session-control-local",
        "--json",
    ]);
    let same_version_add = run_isolated_plugin_command(
        &mut same_version_command,
        "refresh isolated plugin with the same manifest version",
    );
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
    let mut version_bump_command = isolated_codex_command(&binary, &home, &codex_home);
    version_bump_command.args([
        "plugin",
        "add",
        "codex-session-control@codex-session-control-local",
        "--json",
    ]);
    let version_bump_add = run_isolated_plugin_command(
        &mut version_bump_command,
        "refresh isolated plugin after manifest version bump",
    );
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

    let mut remove_command = isolated_codex_command(&binary, &home, &codex_home);
    remove_command.args([
        "plugin",
        "remove",
        "codex-session-control@codex-session-control-local",
        "--json",
    ]);
    let remove =
        run_isolated_plugin_command(&mut remove_command, "remove isolated plugin registration");
    assert!(
        remove.status.success(),
        "isolated plugin removal must succeed"
    );
    let mut list_after_remove_command = isolated_codex_command(&binary, &home, &codex_home);
    list_after_remove_command.args(["plugin", "list", "--json"]);
    let list_after_remove = run_isolated_plugin_command(
        &mut list_after_remove_command,
        "list isolated plugins after native removal",
    );
    assert!(
        list_after_remove.status.success(),
        "isolated plugin listing after removal must succeed"
    );
    assert_eq!(
        parse_single_json_object(
            &list_after_remove.stdout,
            "Codex 0.149.1 plugin list must be one JSON object after removal",
        ),
        json!({"available": [], "installed": []}),
        "native removal must leave the isolated Codex plugin list empty before v1 re-add"
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
    let mut v1_command = isolated_codex_command(&binary, &home, &codex_home);
    v1_command.args([
        "plugin",
        "add",
        "codex-session-control@codex-session-control-local",
        "--json",
    ]);
    let v1_add = run_isolated_plugin_command(
        &mut v1_command,
        "attempt isolated Agent Plugins v1 root MCP registration",
    );
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
