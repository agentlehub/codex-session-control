use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

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
            "cargo test --test cli_contract --locked -- --list",
            "grep -q '^release_assets: test$'",
            r#"CODEX_SESSION_CONTROL_RELEASE_DIR="$PWD/release""#,
            "cargo test --test cli_contract release_assets --locked -- --nocapture",
            "grep -q 'validated release bundle:'",
            "! grep -q 'skipped release bundle:'",
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
            "shellcheck install.sh scripts/check.sh scripts/ci/disposable-systemd-user-contract.sh",
            "sh -n install.sh",
            "bash -n scripts/check.sh",
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
fn native_ci_and_release_binaries_expose_exactly_eight_commands() {
    let exact_commands = "expected=\"$(printf '%s\\n' setup update status enable disable uninstall mcp-server codex)\"";
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
            "{surface} does not assert the exact eight-command surface"
        );
    }
}

#[test]
fn disposable_ci_owns_complete_normal_home_composition() {
    let ci = workflow("ci.yml");
    let integration = job_block(&ci, "systemd-integration");
    let transaction = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts/ci/disposable-systemd-user-contract.sh"),
    )
    .unwrap();

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
)" = "codex-cli 0.146.0""#;
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

    let normal_home_run = transaction
        .find("\"$app_server_harness\" live_normal_home_")
        .expect("normal-home namespace run is missing");
    let broad_regression = transaction
        .find("\"$app_server_harness\" --ignored")
        .expect("broad ignored regression is missing");
    assert!(
        normal_home_run < broad_regression,
        "the four normal-home cases must pass before the broad ignored regression"
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
        "cleanup must precede final runtime, home, linger, and manager absence verification"
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
        "Desktop attachment: available",
        "production Desktop attachment",
    ] {
        assert!(
            !ci.contains(forbidden),
            "CI targets operator state or claims Desktop acceptance: {forbidden}"
        );
    }
}
