use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::Command;

#[path = "support/private_tempdir.rs"]
mod test_support;

const CHECKOUT_SHA: &str = "3d3c42e5aac5ba805825da76410c181273ba90b1";
const CHECKOUT_REFERENCE: &str =
    "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1";
const UPLOAD_ARTIFACT_SHA: &str = "043fb46d1a93c77aae656e7c1c64a875d1fc6a0a";
const UPLOAD_ARTIFACT_REFERENCE: &str =
    "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1";
const DOWNLOAD_ARTIFACT_SHA: &str = "3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c";
const DOWNLOAD_ARTIFACT_USE: &str =
    "actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c";
const DOWNLOAD_ARTIFACT_REFERENCE: &str =
    "actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1";

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

fn named_steps(job: &str) -> Vec<&str> {
    job.lines()
        .filter_map(|line| {
            line.strip_prefix("      - name: ")
                .map(str::trim)
                .filter(|name| !name.is_empty())
        })
        .collect()
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

fn keys_at_indent(document: &str, indent: usize) -> Vec<&str> {
    document
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            (line.starts_with(&" ".repeat(indent))
                && !line.starts_with(&" ".repeat(indent + 1))
                && trimmed.ends_with(':'))
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

fn assert_source_run_contract(job: &str, job_name: &str) {
    assert_required(
        job,
        &[
            r#"[[ "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]"#,
            r#"[[ "$candidate_sha" =~ ^[0-9a-f]{40}$ ]]"#,
            r#"[[ "$source_run_id" =~ ^[1-9][0-9]*$ ]]"#,
            r#"repos/$GITHUB_REPOSITORY/actions/runs/$source_run_id"#,
            r#".event == "push""#,
            r#".status == "completed""#,
            r#".conclusion == "success""#,
            r#".head_sha == $candidate_sha"#,
            r#".head_branch == $tag"#,
            r#".path == ".github/workflows/release.yml""#,
            r#".run_attempt"#,
            r#"[[ "$run_attempt" =~ ^[1-9][0-9]*$ ]]"#,
            r#"repos/$GITHUB_REPOSITORY/actions/runs/$source_run_id/attempts/$run_attempt/jobs?per_page=100"#,
            "--paginate",
            "assemble completed success",
            "build-arm64 completed success",
            "build-x86 completed success",
            "validate completed success",
            r#"test "$actual_jobs" = "$expected_jobs""#,
        ],
        &format!("{job_name} job"),
    );
}

#[test]
fn release_workflow_is_tag_triggered_assembly_only() {
    let release = workflow("release.yml");

    assert_eq!(
        top_level_block(&release, "on:").trim(),
        "on:\n  push:\n    tags:\n      - \"v[0-9]*.[0-9]*.[0-9]*\"",
        "release workflow must be triggered only by stable-version tag pushes"
    );
    assert_eq!(
        top_level_block(&release, "permissions:").trim(),
        "permissions:\n  contents: read",
        "release workflow top-level permissions must be exactly contents: read"
    );
    assert_eq!(
        job_ids(&release),
        ["validate", "build-x86", "build-arm64", "assemble"],
        "release workflow must contain exactly the four assembly jobs"
    );

    let assemble = job_block(&release, "assemble");
    let bundle_validation =
        named_step_block(assemble, "Assemble and validate exactly four release files");
    assert_required(
        assemble,
        &[
            r#"name: release-bundle-${{ github.sha }}"#,
            "retention-days: 7",
            "SHA256SUMS",
            "codex-session-control-aarch64-unknown-linux-gnu",
            "codex-session-control-x86_64-unknown-linux-gnu",
            "install.sh",
        ],
        "release assemble job",
    );
    assert_required(
        bundle_validation,
        &[
            "cargo test --test cli_contract --locked -- --ignored --list",
            r#"CODEX_SESSION_CONTROL_RELEASE_DIR="$PWD/release""#,
            "cargo test --test cli_contract release_assets --locked -- --exact --ignored --nocapture",
            "grep -q 'validated release bundle:'",
        ],
        "required release bundle validation",
    );

    for forbidden in [
        "\n  publish:",
        "workflow_dispatch:",
        "production-release",
        "contents: write",
        "gh release",
    ] {
        assert!(
            !release.contains(forbidden),
            "tag-triggered release workflow contains forbidden publication marker: {forbidden}"
        );
    }
}

#[test]
fn publish_workflow_is_manual_only_with_required_inputs() {
    let publish = workflow("publish.yml");
    let trigger = top_level_block(&publish, "on:");

    assert!(
        publish.lines().any(|line| {
            line == "run-name: Publish ${{ inputs.tag }} at ${{ inputs.candidate_sha }} from Release run ${{ inputs.source_run_id }}"
        }),
        "publish run name must expose the tag, candidate SHA, and source Release run"
    );

    assert!(
        trigger.starts_with("on:\n  workflow_dispatch:\n"),
        "publish workflow must use workflow_dispatch as its only trigger"
    );
    assert_eq!(
        keys_at_indent(trigger, 2),
        ["workflow_dispatch"],
        "workflow_dispatch must be the publish workflow's only trigger"
    );
    let inputs = block_at_indent(trigger, "inputs:", 4);
    assert_eq!(
        keys_at_indent(inputs, 6),
        ["tag", "candidate_sha", "source_run_id"],
        "publish workflow must expose exactly the three release identity inputs"
    );
    for input in ["tag", "candidate_sha", "source_run_id"] {
        let input = block_at_indent(trigger, &format!("{input}:"), 6);
        assert_required(
            input,
            &["required: true", "type: string"],
            &format!("publish input {input}"),
        );
    }
    for forbidden_trigger in [
        "\n  push:",
        "\n  pull_request:",
        "\n  workflow_run:",
        "\n  schedule:",
        "\n  repository_dispatch:",
    ] {
        assert!(
            !trigger.contains(forbidden_trigger),
            "publish workflow contains automatic trigger: {}",
            forbidden_trigger.trim()
        );
    }
    assert!(
        publish.lines().any(|line| line == "permissions: {}"),
        "publish workflow top-level permissions must be empty"
    );
    assert_eq!(
        job_ids(&publish),
        ["verify", "publish"],
        "publish workflow must contain exactly verify and publish jobs"
    );
}

#[test]
fn verify_job_is_read_only_and_binds_the_source() {
    let publish = workflow("publish.yml");
    let verify = job_block(&publish, "verify");

    assert_eq!(
        block_at_indent(verify, "permissions:", 4).trim(),
        "permissions:\n      actions: read\n      contents: read",
        "verify job permissions must be exactly actions: read and contents: read"
    );
    assert!(
        !verify.contains("environment:"),
        "verify job must not use an environment"
    );
    assert!(
        !verify.contains("contents: write"),
        "verify job must not receive contents write permission"
    );
    assert_eq!(
        named_steps(verify),
        [
            "Bind the immutable source evidence",
            "Check out the exact candidate",
            "Require the tag to peel to the candidate",
            "Download the bound source-run bundle",
            "Validate the exact release bundle",
            "Prove the release does not exist",
        ],
        "verify must bind immutable evidence before validating the downloaded candidate"
    );
    assert_required(
        verify,
        &[
            "outputs:\n      run_attempt: ${{ steps.bind.outputs.run_attempt }}\n      artifact_id: ${{ steps.bind.outputs.artifact_id }}\n      artifact_digest: ${{ steps.bind.outputs.artifact_digest }}",
            "id: bind",
        ],
        "verify immutable outputs",
    );
    assert_source_run_contract(verify, "verify");

    let bind = named_step_block(verify, "Bind the immutable source evidence");
    assert_required(
        bind,
        &[
            r#"artifact_name="release-bundle-$candidate_sha""#,
            r#"repos/$GITHUB_REPOSITORY/actions/runs/$source_run_id/artifacts?per_page=100"#,
            "--paginate",
            r#"test "$(jq 'length' <<<"$matching_artifacts")" -eq 1"#,
            r#"[[ "$artifact_id" =~ ^[1-9][0-9]*$ ]]"#,
            r#"[[ "$artifact_digest" =~ ^sha256:[0-9a-f]{64}$ ]]"#,
            r#".expired == false"#,
            r#".workflow_run.id"#,
            r#".workflow_run.head_sha == $candidate_sha"#,
            r#"echo "run_attempt=$run_attempt""#,
            r#"echo "artifact_id=$artifact_id""#,
            r#"echo "artifact_digest=$artifact_digest""#,
            r#"} >> "$GITHUB_OUTPUT""#,
        ],
        "verify immutable source binding step",
    );

    let tag = named_step_block(verify, "Require the tag to peel to the candidate");
    assert_required(
        tag,
        &[
            r#"git fetch --no-tags origin "refs/tags/$tag""#,
            r#"test "$(git rev-parse 'FETCH_HEAD^{commit}')" = "$candidate_sha""#,
        ],
        "verify tag binding step",
    );

    let download = named_step_block(verify, "Download the bound source-run bundle");
    assert_required(
        download,
        &[
            DOWNLOAD_ARTIFACT_USE,
            r#"artifact-ids: ${{ steps.bind.outputs.artifact_id }}"#,
            r#"run-id: ${{ inputs.source_run_id }}"#,
            r#"repository: ${{ github.repository }}"#,
            r#"github-token: ${{ github.token }}"#,
            "path: release",
            "merge-multiple: true",
        ],
        "verify bound artifact download",
    );

    let bundle = named_step_block(verify, "Validate the exact release bundle");
    assert_required(
        bundle,
        &[
            r#"test -f "$entry""#,
            r#"test ! -L "$entry""#,
            "chmod 0755",
            "chmod 0644",
            "(cd release && sha256sum --check SHA256SUMS)",
            "cargo test --test cli_contract --locked -- --ignored --list",
            "grep -q '^release_assets: test$'",
            r#"CODEX_SESSION_CONTROL_RELEASE_DIR="$PWD/release""#,
            "cargo test --test cli_contract release_assets --locked -- --exact --ignored --nocapture",
            "grep -q 'validated release bundle:'",
        ],
        "read-only candidate release_assets validation",
    );

    let absence = named_step_block(verify, "Prove the release does not exist");
    assert_required(
        absence,
        &[
            r#"repos/$GITHUB_REPOSITORY/releases/tags/$tag"#,
            "--include",
            r#"test "$release_status" -ne 0"#,
            r#"^HTTP/[^[:space:]]+[[:space:]]+404([[:space:]]|$)"#,
        ],
        "verify release absence proof",
    );
}

#[test]
fn publish_job_reverifies_after_approval_and_publishes_once() {
    let publish_workflow = workflow("publish.yml");
    let publish = job_block(&publish_workflow, "publish");

    assert_required(
        publish,
        &["needs: verify", "environment: production-release"],
        "publish job approval boundary",
    );
    assert_eq!(
        block_at_indent(publish, "permissions:", 4).trim(),
        "permissions:\n      actions: read\n      contents: write",
        "publish job permissions must be exactly actions: read and contents: write"
    );
    let expected_steps = [
        "Revalidate the immutable source evidence",
        "Reverify the tag target",
        "Download the bound source-run bundle",
        "Requery the bound artifact after download",
        "Validate the bundle without executing candidate code",
        "Prove the release still does not exist",
        "Publish the verified release once",
    ];
    assert_eq!(
        named_steps(publish),
        expected_steps,
        "publish must perform every post-approval check in the required order"
    );
    assert_source_run_contract(publish, "publish");

    let identity = named_step_block(publish, expected_steps[0]);
    assert_required(
        identity,
        &[
            r#"run_attempt: ${{ needs.verify.outputs.run_attempt }}"#,
            r#"artifact_id: ${{ needs.verify.outputs.artifact_id }}"#,
            r#"artifact_digest: ${{ needs.verify.outputs.artifact_digest }}"#,
            r#"test "$current_run_attempt" = "$run_attempt""#,
            r#"repos/$GITHUB_REPOSITORY/actions/runs/$source_run_id/artifacts?per_page=100"#,
            r#"test "$(jq 'length' <<<"$matching_artifacts")" -eq 1"#,
            r#"test "$(jq -r '.[0].id' <<<"$matching_artifacts")" = "$artifact_id""#,
            r#"test "$(jq -r '.[0].digest' <<<"$matching_artifacts")" = "$artifact_digest""#,
            r#"repos/$GITHUB_REPOSITORY/actions/artifacts/$artifact_id"#,
            r#".id"#,
            r#".name == $artifact_name"#,
            r#".digest == $artifact_digest"#,
            r#".expired == false"#,
            r#".workflow_run.id"#,
            r#".workflow_run.head_sha == $candidate_sha"#,
        ],
        "post-approval immutable source revalidation",
    );

    let tag = named_step_block(publish, expected_steps[1]);
    assert_required(
        tag,
        &[
            r#"repos/$GITHUB_REPOSITORY/git/ref/tags/$tag"#,
            r#"test "$tag_commit" = "$candidate_sha""#,
        ],
        "post-approval tag verification",
    );

    let download = named_step_block(publish, expected_steps[2]);
    assert_required(
        download,
        &[
            DOWNLOAD_ARTIFACT_USE,
            r#"artifact-ids: ${{ needs.verify.outputs.artifact_id }}"#,
            r#"run-id: ${{ inputs.source_run_id }}"#,
            r#"repository: ${{ github.repository }}"#,
            r#"github-token: ${{ github.token }}"#,
            r#"path: ${{ runner.temp }}/release"#,
            "merge-multiple: true",
        ],
        "post-approval immutable artifact download",
    );

    let artifact_after = named_step_block(publish, expected_steps[3]);
    assert_required(
        artifact_after,
        &[
            r#"repos/$GITHUB_REPOSITORY/actions/runs/$source_run_id/artifacts?per_page=100"#,
            r#"test "$(jq 'length' <<<"$matching_artifacts")" -eq 1"#,
            r#"test "$(jq -r '.[0].id' <<<"$matching_artifacts")" = "$artifact_id""#,
            r#"test "$(jq -r '.[0].digest' <<<"$matching_artifacts")" = "$artifact_digest""#,
            r#"repos/$GITHUB_REPOSITORY/actions/artifacts/$artifact_id"#,
            r#".id"#,
            r#".name == $artifact_name"#,
            r#".digest == $artifact_digest"#,
            r#".expired == false"#,
            r#".workflow_run.id"#,
            r#".workflow_run.head_sha == $candidate_sha"#,
        ],
        "post-download artifact metadata revalidation",
    );

    let bundle = named_step_block(publish, expected_steps[4]);
    assert_required(
        bundle,
        &[
            r#"release_dir="$RUNNER_TEMP/release""#,
            r#"/usr/bin/find "$release_dir""#,
            r#"test -f "$entry""#,
            r#"test ! -L "$entry""#,
            "/usr/bin/chmod 0755",
            "/usr/bin/chmod 0644",
            "/usr/bin/stat --format '%a'",
            "/usr/bin/sha256sum --check SHA256SUMS",
            "/usr/bin/file",
            "/usr/bin/readelf --file-header",
            "Advanced Micro Devices X86-64",
            "AArch64",
        ],
        "trusted post-approval bundle validation",
    );

    let absence = named_step_block(publish, expected_steps[5]);
    assert_required(
        absence,
        &[
            r#"repos/$GITHUB_REPOSITORY/releases/tags/$tag"#,
            "--include",
            r#"test "$release_status" -ne 0"#,
            r#"^HTTP/[^[:space:]]+[[:space:]]+404([[:space:]]|$)"#,
        ],
        "post-approval release absence proof",
    );

    let mutation = named_step_block(publish, expected_steps[6]);
    assert_eq!(
        publish.matches("gh release create").count(),
        1,
        "publish job must create the release exactly once"
    );
    assert_required(
        mutation,
        &["--verify-tag", r#"--repo "$GITHUB_REPOSITORY""#],
        "single release publication command",
    );
    let mutation_offset = publish
        .find("gh release create")
        .expect("publish mutation must exist");
    for step in &expected_steps[..expected_steps.len() - 1] {
        let step_offset = publish
            .find(&format!("- name: {step}"))
            .unwrap_or_else(|| panic!("missing ordered publish step: {step}"));
        assert!(
            step_offset < mutation_offset,
            "post-approval check must precede the release mutation: {step}"
        );
    }
    assert!(
        publish[..mutation_offset]
            .matches("gh release create")
            .count()
            == 0,
        "no pre-publication step may mutate a release"
    );
    for step in expected_steps
        .iter()
        .filter(|step| **step != "Download the bound source-run bundle")
    {
        assert!(
            named_step_block(publish, step).contains("PATH: /usr/local/bin:/usr/bin:/bin"),
            "publish shell step must use a sanitized PATH: {step}"
        );
    }
    for forbidden in [
        "actions/checkout@",
        "cargo",
        "CODEX_SESSION_CONTROL_RELEASE_DIR",
        "$GITHUB_WORKSPACE",
        "$GITHUB_ENV",
        "./install.sh",
        "bash release/",
        "sh release/",
    ] {
        assert!(
            !publish.contains(forbidden),
            "contents-write publish job contains candidate-controlled code execution marker: {forbidden}"
        );
    }
    assert_eq!(
        publish_workflow.matches("gh release create").count(),
        1,
        "manual workflow must contain exactly one release mutation"
    );
}

#[test]
fn publication_workflows_never_mutate_tags_or_replace_releases() {
    let workflows = format!("{}\n{}", workflow("release.yml"), workflow("publish.yml"));

    for forbidden in [
        "git tag",
        "git push",
        "git update-ref",
        "+refs/tags/",
        ":refs/tags/",
        "git/refs/tags",
        "gh release upload",
        "gh release edit",
        "gh release delete",
        "--clobber",
        "retry",
    ] {
        assert!(
            !workflows.contains(forbidden),
            "publication workflows contain forbidden mutation or retry marker: {forbidden}"
        );
    }
}

#[test]
fn all_workflow_actions_are_commit_pinned() {
    let mut checkout_count = 0;
    let mut upload_artifact_count = 0;
    let mut download_artifact_count = 0;
    for name in ["ci.yml", "release.yml", "publish.yml"] {
        let workflow = workflow(name);
        for line in workflow.lines() {
            let Some(reference) = line.trim().strip_prefix("uses: ") else {
                continue;
            };
            let (action, revision_and_comment) = reference
                .split_once('@')
                .unwrap_or_else(|| panic!("{name} action is missing a revision: {reference}"));
            let revision = revision_and_comment
                .split_ascii_whitespace()
                .next()
                .expect("action revision cannot be empty");
            assert!(
                revision.len() == 40
                    && revision
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "{name} action is not pinned to a lowercase 40-hex commit: {reference}"
            );

            let approved = match action {
                "actions/checkout" => {
                    checkout_count += 1;
                    assert_eq!(
                        reference, CHECKOUT_REFERENCE,
                        "{name} uses an unexpected checkout reference"
                    );
                    CHECKOUT_SHA
                }
                "actions/upload-artifact" => {
                    upload_artifact_count += 1;
                    assert_eq!(
                        reference, UPLOAD_ARTIFACT_REFERENCE,
                        "{name} uses an unexpected upload-artifact reference"
                    );
                    UPLOAD_ARTIFACT_SHA
                }
                "actions/download-artifact" => {
                    download_artifact_count += 1;
                    assert_eq!(
                        reference, DOWNLOAD_ARTIFACT_REFERENCE,
                        "{name} uses an unexpected download-artifact reference"
                    );
                    DOWNLOAD_ARTIFACT_SHA
                }
                _ => panic!("{name} uses an unapproved action: {action}"),
            };
            assert_eq!(
                revision, approved,
                "{name} uses an unapproved pin for {action}"
            );
        }
    }
    assert_eq!(
        checkout_count, 8,
        "CI, release, and publish workflows must contain exactly eight checkout actions"
    );
    assert_eq!(
        upload_artifact_count, 3,
        "release must contain exactly three upload-artifact actions"
    );
    assert_eq!(
        download_artifact_count, 4,
        "release and publish must contain exactly four download-artifact actions"
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
            "shellcheck install.sh scripts/check.sh scripts/set-supported-codex-version.sh",
            "scripts/ci/disposable-systemd-user-contract.sh",
            "sh -n install.sh",
            "bash -n scripts/check.sh",
            "bash -n scripts/set-supported-codex-version.sh",
            "bash -n scripts/ci/disposable-systemd-user-contract.sh",
            "actionlint -version | grep -x '1\\.7\\.12'",
            "actionlint .github/workflows/ci.yml",
            "cargo fmt --version",
            "cargo clippy --version",
            ".github/workflows/ci.yml",
            ".github/workflows/release.yml",
            ".github/workflows/publish.yml",
            "jq empty assets/marketplace/.agents/plugins/marketplace.json",
            "jq empty assets/marketplace/plugins/codex-session-control/.codex-plugin/plugin.json",
            "jq empty assets/marketplace/plugins/codex-session-control/.mcp.json",
            "cargo clippy --workspace --all-targets --all-features --locked -- -D warnings",
            "cargo test --workspace --all-features --locked",
        ],
        "standard checks wrapper",
    );
}

#[test]
fn shared_native_x86_and_release_validation_use_standard_checks() {
    let ci = workflow("ci.yml");
    let release = workflow("release.yml");
    let expected_command = "./scripts/check.sh";

    assert!(
        named_step_block(job_block(&ci, "contract-x86"), "Run locked contract gate")
            .contains(expected_command),
        "CI x86 contract gate must run the standard checks wrapper"
    );
    assert!(
        named_step_block(
            job_block(&release, "validate"),
            "Run the same-commit locked validation gate",
        )
        .contains(expected_command),
        "release validation gate must run the standard checks wrapper"
    );
    for (name, workflow) in [("ci.yml", ci), ("release.yml", release)] {
        assert_required(
            &workflow,
            &[
                "Install verified actionlint 1.7.12",
                "actionlint_1.7.12_linux_amd64.tar.gz",
            ],
            &format!("{name} actionlint gate"),
        );
    }
}

#[test]
fn standard_checks_wrapper_owns_private_temporary_directory_lifecycle() {
    let ci = workflow("ci.yml");
    let release = workflow("release.yml");
    let checks = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/check.sh"))
        .expect("read standard checks wrapper");
    let ci_validation =
        named_step_block(job_block(&ci, "contract-x86"), "Run locked contract gate");
    let release_validation = named_step_block(
        job_block(&release, "validate"),
        "Run the same-commit locked validation gate",
    );

    assert_required(
        &checks,
        &[
            "mktemp --directory",
            "trap cleanup_check_tmp EXIT",
            "export TMPDIR=\"$check_tmp\"",
        ],
        "standard checks private temporary-directory lifecycle",
    );
    for (surface, validation) in [
        ("CI x86 validation", ci_validation),
        ("release validation", release_validation),
    ] {
        assert!(
            !validation.contains("test_tmp=") && !validation.contains("printf 'TMPDIR="),
            "{surface} must delegate temporary-directory setup to scripts/check.sh"
        );
    }
}

#[test]
fn native_ci_and_release_binaries_expose_exactly_seven_visible_commands() {
    let exact_commands =
        "expected=\"$(printf '%s\\n' setup update status enable disable uninstall codex)\"";
    let ci = workflow("ci.yml");
    let release = workflow("release.yml");

    for (surface, contract) in [
        ("CI x86", job_block(&ci, "contract-x86")),
        ("CI ARM64", job_block(&ci, "contract-arm64")),
        ("release x86", job_block(&release, "build-x86")),
        ("release ARM64", job_block(&release, "build-arm64")),
    ] {
        assert!(
            contract.contains(exact_commands),
            "{surface} does not assert the exact seven-visible-command surface"
        );
    }
}

#[test]
fn systemd_integration_owns_complete_disposable_user_contract() {
    let ci = workflow("ci.yml");
    let integration = job_block(&ci, "systemd-integration");
    let transaction = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts/ci/disposable-systemd-user-contract.sh"),
    )
    .unwrap();

    assert!(
        transaction.starts_with("#!/usr/bin/env bash\nset -euo pipefail\n"),
        "disposable systemd transaction must use the strict Bash header"
    );
    let transaction_lines = transaction.lines().collect::<Vec<_>>();
    let exact_line = |expected: &str| {
        let matches = transaction_lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| (*line == expected).then_some(index))
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "disposable systemd transaction must contain exactly one `{expected}` line"
        );
        matches[0]
    };
    let cleanup_definition = exact_line("cleanup() {");
    let cleanup_end = transaction_lines
        .iter()
        .enumerate()
        .skip(cleanup_definition + 1)
        .find_map(|(index, line)| (*line == "}").then_some(index))
        .expect("cleanup function closing brace is missing");
    let trap_installation = exact_line("trap cleanup EXIT");
    let first_privileged_mutation =
        exact_line("sudo useradd --create-home --home-dir \"$home\" --shell /bin/bash \"$user\"");
    let explicit_cleanup = exact_line("cleanup");
    let trap_clearing = exact_line("trap - EXIT");
    let runtime_absence = exact_line("test ! -e \"$runtime\"");
    let home_absence = exact_line("test ! -e \"$home\"");
    let linger_absence = exact_line("test ! -e \"/var/lib/systemd/linger/$user\"");
    let manager_absence = exact_line("if sudo systemctl is-active --quiet \"$manager\"; then");
    assert!(
        cleanup_definition < cleanup_end
            && cleanup_end < trap_installation
            && trap_installation < first_privileged_mutation,
        "cleanup must be defined and trapped before the first privileged mutation"
    );
    assert!(
        first_privileged_mutation < explicit_cleanup
            && explicit_cleanup < trap_clearing
            && trap_clearing < runtime_absence
            && runtime_absence < home_absence
            && home_absence < linger_absence
            && linger_absence < manager_absence,
        "explicit cleanup and trap clearing must precede final disposable-state absence checks"
    );

    let checkout = integration.find(CHECKOUT_REFERENCE).unwrap();
    let prerequisites = integration
        .find("- name: Install systemd-user prerequisites")
        .unwrap();
    let invocation = integration
        .find(
            "- name: Run the disposable systemd-user contract\n        shell: bash\n        run: bash scripts/ci/disposable-systemd-user-contract.sh",
        )
        .unwrap();
    assert!(
        checkout < prerequisites && prerequisites < invocation,
        "checkout, prerequisites, and the repository-pinned transaction must remain ordered"
    );
    assert_eq!(
        integration
            .matches("bash scripts/ci/disposable-systemd-user-contract.sh")
            .count(),
        1,
        "systemd integration must invoke exactly one repository-owned transaction"
    );
    assert!(
        !integration.contains("user=codex-session-control-ci"),
        "the script, not the workflow, must own the disposable-user transaction"
    );
    assert_required(
        integration,
        &[
            CHECKOUT_REFERENCE,
            "sudo apt-get update",
            "sudo apt-get install --yes dbus-user-session jq",
            "test -x /usr/bin/loginctl",
            "test -x /usr/bin/systemctl",
            "test -x /usr/lib/systemd/systemd",
            "sudo systemctl is-active --quiet systemd-logind.service",
        ],
        "disposable systemd workflow wiring",
    );

    assert_required(
        &transaction,
        &[
            "user=codex-session-control-ci",
            "home=/home/codex-session-control-ci",
            "codex_home=\"$home/.codex\"",
            "probe_home=\"$home/.codex-version-probe\"",
            "CODEX_SESSION_CONTROL_DISPOSABLE_SYSTEMD_USER=1",
            "CODEX_SESSION_CONTROL_DISPOSABLE_CLI_CANARY=1",
            "live_count=\"$(",
            "grep -Ec '^live_normal_home_.*: test$'",
            "test \"$live_count\" -eq 4",
            r#""$app_server_harness" live_normal_home_ \
    --ignored --nocapture --test-threads=1"#,
            r#""$app_server_harness" --ignored \
    --nocapture --test-threads=1 --skip live_normal_home_"#,
        ],
        "disposable normal-home transaction",
    );
    assert_required(
        &transaction,
        &[
            "controller_version=\"$(\"$controller_executable\" --version)\"",
            "test \"$(sudo -u \"$user\" \"$controller_binary\" --version)\" = \"$controller_version\"",
        ],
        "disposable controller version contract",
    );
    assert_required(
        &transaction,
        &[
            "sudo loginctl enable-linger \"$user\" || return",
            "sudo systemctl start \"$manager\" || return",
            "runtime=\"$(sudo loginctl show-user \"$user\" --property=RuntimePath --value)\"",
            "sudo test -S \"$runtime/bus\" || return",
            "systemctl --user list-units >/dev/null || return",
        ],
        "native disposable user-manager bootstrap",
    );
    for forbidden in [
        "dbus-run-session",
        "/usr/lib/systemd/systemd --user",
        "GITHUB_WORKSPACE",
        "WORKSPACE=",
        "CARGO_PATH=",
        "RUSTUP_HOME=",
        "CARGO_HOME=",
        "CARGO_TARGET_DIR=",
        "cd \"$WORKSPACE\"",
        "setfacl",
        "rustup toolchain install",
    ] {
        assert!(
            !transaction.contains(forbidden),
            "disposable integration depends on forbidden runner bootstrap state: {forbidden}"
        );
    }
    assert!(
        !transaction
            .lines()
            .any(|line| line.contains("grep -x 'codex-session-control ")),
        "disposable integration must not pin a product release version"
    );

    let native_probe = r#"sudo install --directory --owner "$user" --group "$user" --mode 0700 \
  "$probe_home"
test "$(sudo stat --format=%F "$probe_home")" = directory
test "$(sudo stat --format=%u "$probe_home")" = "$uid"
test "$(sudo stat --format=%a "$probe_home")" = 700
test "$(
  sudo -u "$user" env \
    HOME="$home" \
    CODEX_HOME="$probe_home" \
    "$native_codex_binary" --version
)" = "codex-cli $supported_codex_version""#;
    assert!(
        transaction.contains(native_probe),
        "the native Codex probe must be one exact isolated, owned, mode-checked stanza"
    );

    let suffixed_run = transaction
        .find("--exact install::tests::disposable_systemd_user")
        .expect("suffixed disposable systemd run is missing");
    let codex_home_lines = transaction[..suffixed_run]
        .lines()
        .filter(|line| line.contains("codex_home"))
        .map(str::trim)
        .collect::<Vec<_>>();
    assert_eq!(
        codex_home_lines,
        [
            "codex_home=\"$home/.codex\"",
            "sudo -u \"$user\" test ! -e \"$codex_home\"",
            "sudo -u \"$user\" test ! -L \"$codex_home\"",
        ],
        "before the suffixed test, selected .codex references must be limited to assignment and read-only absence checks"
    );

    let lifecycle_run = transaction
        .find("--exact install::tests::disposable_systemd_user")
        .expect("disposable lifecycle run is missing");
    let normal_home_run = transaction
        .find("\"$app_server_harness\" live_normal_home_")
        .expect("normal-home namespace run is missing");
    let broad_regression = transaction
        .find("\"$app_server_harness\" --ignored")
        .expect("broad ignored regression is missing");
    assert!(
        lifecycle_run < normal_home_run && normal_home_run < broad_regression,
        "the lifecycle proof, four normal-home cases, and broad ignored regression must remain ordered"
    );

    assert!(
        transaction.contains(
            r#"cleanup
trap - EXIT
test ! -e "$runtime"
test ! -e "$home"
test ! -e "/var/lib/systemd/linger/$user"
if sudo systemctl is-active --quiet "$manager"; then"#,
        ),
        "cleanup must precede contiguous final runtime, home, linger, and manager absence verification"
    );
    assert!(
        transaction
            .trim_end()
            .ends_with("! id \"$user\" >/dev/null 2>&1"),
        "the transaction must end by proving the disposable user is absent"
    );

    for forbidden in [
        "/home/korty",
        "/run/user/1000",
        "Desktop configuration: ready",
        "production Desktop configuration",
    ] {
        assert!(
            !ci.contains(forbidden),
            "CI targets operator state or claims Desktop acceptance: {forbidden}"
        );
    }
}
