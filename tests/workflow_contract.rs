use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

#[path = "support/private_tempdir.rs"]
mod test_support;

const CHECKOUT_SHA: &str = "3d3c42e5aac5ba805825da76410c181273ba90b1";

fn workflow(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".github")
        .join("workflows")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read workflow {}: {error}", path.display()))
}

#[test]
fn supported_version_setter_validates_and_atomically_replaces_only_version_file() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = test_support::private_tempdir();
    let scripts = root.path().join("scripts");
    fs::create_dir(&scripts).unwrap();
    fs::copy(
        source_root.join("supported-codex-version.txt"),
        root.path().join("supported-codex-version.txt"),
    )
    .unwrap();
    let version_file = root.path().join("supported-codex-version.txt");
    fs::set_permissions(&version_file, fs::Permissions::from_mode(0o640)).unwrap();
    let setter = scripts.join("set-supported-codex-version.sh");
    fs::copy(
        source_root.join("scripts/set-supported-codex-version.sh"),
        &setter,
    )
    .unwrap();
    fs::set_permissions(&setter, fs::Permissions::from_mode(0o755)).unwrap();

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
        assert!(output.stderr.is_empty());
        assert_eq!(
            fs::read_to_string(&version_file).unwrap(),
            format!("{accepted_version}\n")
        );
        assert_eq!(
            fs::metadata(&version_file).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_no_version_stage(root.path());
    }

    let version_before_rejection = fs::read(&version_file).unwrap();
    for rejected_version in [
        "1.2.3-01",
        "1.2.3-alpha..1",
        "1.2.3-alpha/1",
        "1.2.3-alpha\\1",
        "1.2.3 ",
        "1.2.3\n",
        "1.2.3\x1b",
        "1.2.3+é",
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
        assert_eq!(fs::read(&version_file).unwrap(), version_before_rejection);
        assert_no_version_stage(root.path());
    }

    let missing_argument = Command::new(&setter).output().unwrap();
    assert!(!missing_argument.status.success());
    assert_eq!(fs::read(&version_file).unwrap(), version_before_rejection);

    let fake_bin = root.path().join("fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    let fake_mv = fake_bin.join("mv");
    fs::write(&fake_mv, "#!/bin/sh\nexit 91\n").unwrap();
    fs::set_permissions(&fake_mv, fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = vec![fake_bin.clone()];
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(path).unwrap();

    let failed_replace = Command::new(&setter)
        .arg("2.0.0")
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(!failed_replace.status.success());
    assert_eq!(fs::read(&version_file).unwrap(), version_before_rejection);
    assert_no_version_stage(root.path());

    let fake_mktemp = fake_bin.join("mktemp");
    fs::write(&fake_mktemp, "#!/bin/sh\nexit 92\n").unwrap();
    fs::set_permissions(&fake_mktemp, fs::Permissions::from_mode(0o755)).unwrap();
    let failed_stage = Command::new(&setter)
        .arg("2.0.0")
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(!failed_stage.status.success());
    assert_eq!(fs::read(&version_file).unwrap(), version_before_rejection);
    assert_no_version_stage(root.path());
}

fn assert_no_version_stage(root: &Path) {
    assert!(
        !fs::read_dir(root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".set-supported-version.")
        }),
        "version setter left a staging file behind"
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

fn matrix_entry<'a>(document: &'a str, runner: &str) -> &'a str {
    let marker = format!("          - runner: {runner}\n");
    let start = document
        .find(&marker)
        .unwrap_or_else(|| panic!("missing native runner: {runner}"));
    let remaining = &document[start + marker.len()..];
    let end = remaining
        .find("\n          - runner:")
        .map(|offset| start + marker.len() + offset)
        .unwrap_or(document.len());
    &document[start..end]
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
            revision, CHECKOUT_SHA,
            "CI checkout pin changed unexpectedly"
        );
        checkout_count += 1;
    }
    assert!(
        checkout_count > 0,
        "CI must run at least one approved commit-pinned action"
    );
}

#[test]
fn native_ci_runs_the_canonical_gate_on_both_supported_architectures() {
    let ci = workflow("ci.yml");
    let triggers = top_level_block(&ci, "on:");
    assert!(triggers.lines().any(|line| line.trim() == "push:"));
    assert!(
        block_at_indent(&ci, "push:", 2)
            .lines()
            .any(|line| line.trim() == "- main")
    );
    assert!(triggers.lines().any(|line| line.trim() == "pull_request:"));
    assert!(
        triggers
            .lines()
            .any(|line| line.trim() == "workflow_dispatch:")
    );

    let permissions = top_level_block(&ci, "permissions:");
    assert_eq!(
        permissions.trim(),
        "permissions:\n  contents: read",
        "CI must retain only read access to repository contents"
    );
    assert!(
        top_level_block(&ci, "concurrency:")
            .lines()
            .any(|line| line.trim() == "cancel-in-progress: true"),
        "CI must cancel superseded runs"
    );
    assert!(ci.lines().any(|line| line.trim() == "timeout-minutes: 30"));

    for (runner, machine, actionlint_arch, actionlint_sha256) in [
        (
            "ubuntu-24.04",
            "X86-64",
            "amd64",
            "8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8",
        ),
        (
            "ubuntu-24.04-arm",
            "AArch64",
            "arm64",
            "325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6",
        ),
    ] {
        let entry = matrix_entry(&ci, runner);
        assert!(
            entry
                .lines()
                .any(|line| line.trim() == format!("machine: {machine}"))
        );
        assert!(
            entry
                .lines()
                .any(|line| line.trim() == format!("actionlint_arch: {actionlint_arch}"))
        );
        assert!(
            entry
                .lines()
                .any(|line| { line.trim() == format!("actionlint_sha256: {actionlint_sha256}") })
        );
    }
    assert!(ci.contains("actionlint_1.7.12_linux_${{ matrix.actionlint_arch }}.tar.gz"));
    assert!(ci.lines().any(|line| line.contains("sha256sum --check")));
    assert_eq!(
        ci.lines()
            .filter(|line| line.trim() == "run: ./scripts/check.sh")
            .count(),
        1,
        "the matrix must invoke the canonical repository gate exactly once per leg"
    );
}
