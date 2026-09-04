use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::Command;

#[path = "support/private_tempdir.rs"]
mod test_support;

const CHECKOUT_SHA: &str = "3d3c42e5aac5ba805825da76410c181273ba90b1";
const CHECKOUT_REFERENCE: &str =
    "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1";

fn workflow(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".github")
        .join("workflows")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read workflow {}: {error}", path.display()))
}

#[test]
fn tested_codex_version_has_one_canonical_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let raw = fs::read_to_string(root.join("supported-codex-version.txt")).unwrap();
    let version = raw
        .strip_suffix('\n')
        .expect("tested Codex version must end with one newline");
    assert!(!version.contains(['\r', '\n']));
    assert_eq!(
        semver::Version::parse(version).unwrap().to_string(),
        version,
        "tested Codex version must be canonical SemVer"
    );
    for canonical_full_version in [
        "0.150.0-alpha.12.2",
        "1.2.3+build.7",
        "1.2.3-alpha.1+linux.x86-64",
    ] {
        assert_eq!(
            semver::Version::parse(canonical_full_version)
                .unwrap()
                .to_string(),
            canonical_full_version,
            "canonical full SemVer must round-trip: {canonical_full_version}"
        );
    }

    let build_script = fs::read_to_string(root.join("build.rs")).unwrap();
    assert_required(
        &build_script,
        &[
            "supported-codex-version.txt",
            "CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION",
            "semver::Version::parse",
            "parsed.to_string() == version",
        ],
        "tested Codex version build bridge",
    );

    let app_server = fs::read_to_string(root.join("src/app_server.rs")).unwrap();
    assert_required(
        &app_server,
        &["env!(\"CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION\")"],
        "application tested Codex version",
    );
    let readme = fs::read_to_string(root.join("README.md")).unwrap();
    let marker = "<!-- generated: supported-codex-version -->";
    let marked_lines = readme
        .lines()
        .filter(|line| line.contains(marker))
        .collect::<Vec<_>>();
    assert_eq!(
        marked_lines,
        [format!(
            "- Native app-server protocol validated against Codex `{version}`. {marker}"
        )],
        "README must expose exactly one generated tested-version line"
    );
    assert!(
        readme.contains("- Codex CLI `0.149.1` on `PATH` is the plugin-host target."),
        "README must separately state the Codex CLI plugin-host target"
    );

    let fixture_raw =
        fs::read_to_string(root.join("tests/fixtures/app-server-contract.json")).unwrap();
    assert!(!fixture_raw.contains("__TESTED_CODEX_VERSION__"));
    let fixture: serde_json::Value = serde_json::from_str(&fixture_raw).unwrap();
    assert_eq!(fixture["codexVersion"], version);
    assert!(
        fixture["successfulExemplars"]["initialize"]["userAgent"]
            .as_str()
            .unwrap()
            .contains(version)
    );

    let setter = root.join("scripts/set-supported-codex-version.sh");
    assert_eq!(
        fs::metadata(&setter).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let setter_source = fs::read_to_string(setter).unwrap();
    assert_required(
        &setter_source,
        &[
            "supported-codex-version.txt",
            "generated: supported-codex-version",
            "VERSION must be canonical SemVer",
        ],
        "supported Codex version setter",
    );
    for unrelated_action in ["codex --version", "cargo test", "npm install"] {
        assert!(
            !setter_source.contains(unrelated_action),
            "setter must not perform unrelated action: {unrelated_action}"
        );
    }
}

#[test]
fn tested_codex_version_setter_updates_only_generated_version_data() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let original_version =
        fs::read_to_string(source_root.join("supported-codex-version.txt")).unwrap();
    let original_version = original_version
        .strip_suffix('\n')
        .expect("tested Codex version must end with one newline");
    let original_readme = fs::read_to_string(source_root.join("README.md")).unwrap();
    let original_fixture =
        fs::read(source_root.join("tests/fixtures/app-server-contract.json")).unwrap();
    let root = test_support::private_tempdir();
    let scripts = root.path().join("scripts");
    let fixtures = root.path().join("tests/fixtures");
    fs::create_dir(&scripts).unwrap();
    fs::create_dir_all(&fixtures).unwrap();
    fs::copy(source_root.join("README.md"), root.path().join("README.md")).unwrap();
    fs::copy(
        source_root.join("supported-codex-version.txt"),
        root.path().join("supported-codex-version.txt"),
    )
    .unwrap();
    fs::copy(
        source_root.join("tests/fixtures/app-server-contract.json"),
        fixtures.join("app-server-contract.json"),
    )
    .unwrap();
    let setter = scripts.join("set-supported-codex-version.sh");
    fs::copy(
        source_root.join("scripts/set-supported-codex-version.sh"),
        &setter,
    )
    .unwrap();
    let marker = "<!-- generated: supported-codex-version -->";
    let mut expected_version = original_version.to_owned();
    let mut expected_readme = original_readme.clone();
    for accepted_version in [
        "0.150.0-alpha.12.2",
        "1.2.3+build.7",
        "1.2.3-alpha.1+linux.x86-64",
    ] {
        let output = Command::new(&setter)
            .arg(accepted_version)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "setter rejected {accepted_version}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(root.path().join("supported-codex-version.txt")).unwrap(),
            format!("{accepted_version}\n")
        );
        let old_line = format!(
            "- Native app-server protocol validated against Codex `{expected_version}`. {marker}"
        );
        let new_line = format!(
            "- Native app-server protocol validated against Codex `{accepted_version}`. {marker}"
        );
        expected_readme = expected_readme.replace(&old_line, &new_line);
        assert_eq!(
            fs::read_to_string(root.path().join("README.md")).unwrap(),
            expected_readme
        );
        assert_eq!(
            fs::read(fixtures.join("app-server-contract.json")).unwrap(),
            original_fixture
        );
        expected_version = accepted_version.to_owned();
    }

    let version_before_rejection =
        fs::read(root.path().join("supported-codex-version.txt")).unwrap();
    let readme_before_rejection = fs::read(root.path().join("README.md")).unwrap();
    let fixture_before_rejection = fs::read(fixtures.join("app-server-contract.json")).unwrap();
    for rejected_version in [
        "1.2.3-01",
        "1.2.3-alpha..1",
        "1.2.3-alpha/1",
        "1.2.3-alpha\\1",
        "1.2.3 ",
        "1.2.3\n",
        "../1.2.3",
    ] {
        let rejected = Command::new(&setter)
            .arg(rejected_version)
            .output()
            .unwrap();
        assert!(
            !rejected.status.success(),
            "setter accepted invalid SemVer: {rejected_version:?}"
        );
        assert_eq!(
            fs::read(root.path().join("supported-codex-version.txt")).unwrap(),
            version_before_rejection
        );
        assert_eq!(
            fs::read(root.path().join("README.md")).unwrap(),
            readme_before_rejection
        );
        assert_eq!(
            fs::read(fixtures.join("app-server-contract.json")).unwrap(),
            fixture_before_rejection
        );
    }

    let real_mv = Command::new("sh")
        .args(["-c", "command -v mv"])
        .output()
        .unwrap();
    assert!(real_mv.status.success());
    let real_mv = String::from_utf8(real_mv.stdout).unwrap();
    let real_mv = real_mv.trim();
    let fake_bin = root.path().join("fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    let mv_count = root.path().join("mv-count");
    let fake_mv = fake_bin.join("mv");
    fs::write(
        &fake_mv,
        format!(
            "#!/bin/sh\ncount=0\n[ ! -f '{count}' ] || count=$(cat '{count}')\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > '{count}'\n[ \"$count\" -ne 2 ] || exit 91\nexec '{real_mv}' \"$@\"\n",
            count = mv_count.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_mv, fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = vec![fake_bin.clone()];
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(path).unwrap();
    let failed_version = "2.0.0";
    let failed_replace = Command::new(&setter)
        .arg(failed_version)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(!failed_replace.status.success());
    assert_eq!(
        fs::read(root.path().join("supported-codex-version.txt")).unwrap(),
        version_before_rejection
    );
    assert_eq!(
        fs::read(root.path().join("README.md")).unwrap(),
        readme_before_rejection
    );
    assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".set-supported-version.")
    }));

    let fake_mktemp = fake_bin.join("mktemp");
    fs::write(&fake_mktemp, "#!/bin/sh\nexit 92\n").unwrap();
    fs::set_permissions(&fake_mktemp, fs::Permissions::from_mode(0o755)).unwrap();
    let failed_stage = Command::new(&setter)
        .arg(failed_version)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(!failed_stage.status.success());
    assert_eq!(
        fs::read(root.path().join("supported-codex-version.txt")).unwrap(),
        version_before_rejection
    );
    assert_eq!(
        fs::read(root.path().join("README.md")).unwrap(),
        readme_before_rejection
    );
}

#[test]
fn integration_temp_roots_ignore_ambient_tmpdir() {
    const PROBE: &str = "CODEX_SESSION_CONTROL_PRIVATE_TMPDIR_PROBE";
    if std::env::var_os(PROBE).is_some() {
        let root = test_support::private_tempdir();
        let metadata = fs::metadata(root.path()).unwrap();
        assert!(root.path().starts_with(test_support::effective_user_home()));
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        assert!(!root.path().starts_with("/tmp"));
        return;
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "integration_temp_roots_ignore_ambient_tmpdir",
            "--exact",
            "--nocapture",
        ])
        .env(PROBE, "1")
        .env("TMPDIR", "/tmp")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "private TMPDIR probe failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn block_at_indent<'a>(document: &'a str, header: &str, indent: usize) -> &'a str {
    let expected_header = format!("{}{header}", " ".repeat(indent));
    let start = document
        .lines()
        .enumerate()
        .find_map(|(index, line)| (line == expected_header).then_some(index))
        .unwrap_or_else(|| panic!("missing block header: {expected_header}"));
    let lines = document.lines().collect::<Vec<_>>();
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| {
            let leading = line.len() - line.trim_start().len();
            (!line.trim().is_empty() && leading <= indent).then_some(index)
        })
        .unwrap_or(lines.len());
    let start_offset = lines[..start]
        .iter()
        .map(|line| line.len() + 1)
        .sum::<usize>();
    let end_offset = lines[..end]
        .iter()
        .map(|line| line.len() + 1)
        .sum::<usize>()
        .min(document.len());
    &document[start_offset..end_offset]
}

fn top_level_block<'a>(document: &'a str, header: &str) -> &'a str {
    block_at_indent(document, header, 0)
}

fn job_block<'a>(document: &'a str, job: &str) -> &'a str {
    block_at_indent(document, &format!("{job}:"), 2)
}

fn named_step_block<'a>(job: &'a str, name: &str) -> &'a str {
    block_at_indent(job, &format!("- name: {name}"), 6)
}

fn job_ids(document: &str) -> Vec<&str> {
    let jobs = top_level_block(document, "jobs:");
    jobs.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            (line.starts_with("  ")
                && !line.starts_with("   ")
                && trimmed.ends_with(':')
                && trimmed != "jobs:")
                .then(|| trimmed.trim_end_matches(':'))
        })
        .collect()
}

fn assert_required(haystack: &str, required: &[&str], context: &str) {
    for needle in required {
        assert!(
            haystack.contains(needle),
            "{context} is missing required contract marker: {needle}"
        );
    }
}

#[test]
fn all_workflow_actions_are_commit_pinned() {
    let ci = workflow("ci.yml");
    let mut checkout_count = 0;
    for line in ci.lines() {
        let Some(reference) = line.trim().strip_prefix("uses: ") else {
            continue;
        };
        let (action, revision_and_comment) = reference
            .split_once('@')
            .unwrap_or_else(|| panic!("CI action is missing a revision: {reference}"));
        let revision = revision_and_comment
            .split_ascii_whitespace()
            .next()
            .expect("action revision cannot be empty");
        assert!(
            revision.len() == 40
                && revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "CI action is not pinned to a lowercase 40-hex commit: {reference}"
        );
        assert_eq!(action, "actions/checkout", "CI uses an unapproved action");
        assert_eq!(
            reference, CHECKOUT_REFERENCE,
            "CI uses an unexpected checkout pin"
        );
        assert_eq!(
            revision, CHECKOUT_SHA,
            "CI checkout pin changed unexpectedly"
        );
        checkout_count += 1;
    }
    assert_eq!(
        checkout_count, 1,
        "the matrix must expand one YAML checkout declaration into both native jobs"
    );
}

#[test]
fn standard_checks_wrapper_contains_shared_checks_and_is_executable() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/check.sh");
    let metadata = fs::metadata(&path).unwrap_or_else(|error| {
        panic!(
            "cannot read standard checks wrapper {}: {error}",
            path.display()
        )
    });
    assert!(
        metadata.permissions().mode() & 0o111 != 0,
        "standard checks wrapper must be executable"
    );

    let checks = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "cannot read standard checks wrapper {}: {error}",
            path.display()
        )
    });
    assert_required(
        &checks,
        &[
            "cargo fmt --all -- --check",
            "shellcheck scripts/check.sh scripts/set-supported-codex-version.sh \\\n  scripts/install-local-plugin.sh",
            "bash -n scripts/check.sh scripts/set-supported-codex-version.sh \\\n  scripts/install-local-plugin.sh",
            "actionlint -version | grep -x '1\\.7\\.12'",
            "actionlint .github/workflows/ci.yml",
            "cargo fmt --version",
            "cargo clippy --version",
            "jq empty .agents/plugins/marketplace.json",
            "plugins/codex-session-control/.codex-plugin/plugin.json",
            "plugins/codex-session-control/.mcp.json",
            "cargo clippy --workspace --all-targets --all-features --locked -- -D warnings",
            "cargo test --workspace --all-features --locked",
        ],
        "standard checks wrapper",
    );
    for obsolete in [
        "install.sh",
        "scripts/ci/disposable-systemd-user-contract.sh",
        ".github/workflows/release.yml",
        ".github/workflows/publish.yml",
        "assets/marketplace/",
    ] {
        assert!(
            !checks.contains(obsolete),
            "standard checks wrapper validates obsolete lifecycle material: {obsolete}"
        );
    }
}

#[test]
fn native_ci_builds_stages_and_executes_both_supported_architectures() {
    let ci = workflow("ci.yml");
    assert_eq!(
        top_level_block(&ci, "on:").trim(),
        "on:\n  push:\n    branches:\n      - main\n  pull_request:\n  workflow_dispatch:",
        "CI must retain main push, pull-request, and manual triggers"
    );
    assert_eq!(
        top_level_block(&ci, "permissions:").trim(),
        "permissions:\n  contents: read",
        "CI must retain read-only permissions"
    );
    assert_required(
        &ci,
        &[
            "concurrency:\n  group: ci-${{ github.workflow }}-${{ github.ref }}\n  cancel-in-progress: true",
        ],
        "CI concurrency policy",
    );
    assert_eq!(
        job_ids(&ci),
        ["native-contract"],
        "CI must define exactly one matrix job"
    );

    let native = job_block(&ci, "native-contract");
    assert_required(
        native,
        &[
            "name: native-contract (${{ matrix.machine }})",
            "runs-on: ${{ matrix.runner }}",
            "timeout-minutes: 30",
            "strategy:",
            "matrix:",
            "include:",
            "          - runner: ubuntu-24.04\n            machine: X86-64\n            actionlint_arch: amd64\n            actionlint_sha256: 8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8",
            "          - runner: ubuntu-24.04-arm\n            machine: AArch64\n            actionlint_arch: arm64\n            actionlint_sha256: 325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6",
            CHECKOUT_REFERENCE,
            "Install validation prerequisites",
            "Install verified actionlint 1.7.12",
            "actionlint_1.7.12_linux_${{ matrix.actionlint_arch }}.tar.gz",
            "sha256sum --check",
        ],
        "native CI matrix",
    );
    let gate = named_step_block(native, "Run canonical repository gate");
    assert_eq!(
        gate.trim(),
        "- name: Run canonical repository gate\n        run: ./scripts/check.sh",
        "each native leg must delegate exactly once to the canonical repository gate"
    );
    for forbidden in [
        "systemd-integration",
        "cross",
        "release.yml",
        "publish.yml",
        "cargo build",
        "readelf",
        "plugin_packaging_contract",
        "mcp_contract",
        "setup update status enable disable uninstall codex",
    ] {
        assert!(
            !native.contains(forbidden),
            "native CI duplicates deleted or wrapper-owned behavior: {forbidden}"
        );
    }
}

#[test]
fn standard_checks_wrapper_owns_private_temporary_directory_lifecycle() {
    let ci = workflow("ci.yml");
    let checks = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/check.sh"))
        .expect("read standard checks wrapper");
    let native = job_block(&ci, "native-contract");
    let validation = named_step_block(native, "Run canonical repository gate");

    assert_required(
        &checks,
        &[
            "mktemp --directory",
            "trap cleanup_check_tmp EXIT",
            "export TMPDIR=\"$check_tmp\"",
        ],
        "standard checks private temporary-directory lifecycle",
    );
    assert!(
        !validation.contains("test_tmp=")
            && !validation.contains("TMPDIR=")
            && !validation.contains("GITHUB_ENV"),
        "native CI must delegate temporary-directory ownership to scripts/check.sh"
    );
}

#[test]
fn reader_facing_surfaces_do_not_advertise_removed_lifecycle() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let current_surfaces = [
        "README.md",
        "CONTRIBUTING.md",
        "SECURITY.md",
        "docs/architecture.md",
        "docs/desktop.md",
        "docs/security.md",
        "docs/troubleshooting.md",
        ".github/ISSUE_TEMPLATE/bug.yml",
    ];
    let removed_surface = [
        "codex-session-control setup",
        "codex-session-control status",
        "codex-session-control enable",
        "codex-session-control disable",
        "codex-session-control update",
        "codex-session-control uninstall",
        "codex-session-control codex",
        "codex-session-control mcp-server",
        "external-app-server-attachment",
        "app-server-attachment.json",
        "codex-session-control.service",
        "releases/latest/download/install.sh",
        "$HOME/.local/bin/codex-session-control",
        "$HOME/.config/codex-session-control",
        "| `setup` |",
        "| `update` |",
        "| `status` |",
        "| `enable` |",
        "| `disable` |",
        "| `uninstall` |",
        "| `codex` |",
    ];

    for path in current_surfaces {
        let text = fs::read_to_string(root.join(path)).unwrap();
        for removed in removed_surface {
            assert!(
                !text.contains(removed),
                "{path} advertises removed lifecycle surface: {removed}"
            );
        }
    }
}

#[test]
fn bug_form_requests_plugin_available_evidence() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bug_form = fs::read_to_string(root.join(".github/ISSUE_TEMPLATE/bug.yml")).unwrap();

    assert_required(
        &bug_form,
        &[
            "Host surface",
            "Plugin version and visibility",
            "Desktop shared-socket availability",
            "Exact stderr or MCP error",
            "Reproduction steps",
        ],
        "bug report evidence",
    );
    for removed_instruction in [
        "codex-session-control --version",
        "codex-session-control status",
    ] {
        assert!(
            !bug_form.contains(removed_instruction),
            "bug form asks for removed command evidence: {removed_instruction}"
        );
    }
}
