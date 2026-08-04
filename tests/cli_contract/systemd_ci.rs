#[test]
fn systemd_ci_uses_the_native_disposable_user_manager() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    let transaction =
        std::fs::read_to_string(root.join("scripts/ci/disposable-systemd-user-contract.sh"))
            .unwrap();

    let prerequisites = workflow
        .find("- name: Install systemd-user prerequisites")
        .unwrap();
    let invocation = workflow
        .find(
            "- name: Run the disposable systemd-user contract\n        shell: bash\n        run: bash scripts/ci/disposable-systemd-user-contract.sh",
        )
        .unwrap();
    assert!(
        prerequisites < invocation,
        "systemd-user prerequisites must be installed before the repository-pinned transaction"
    );
    assert_eq!(
        workflow
            .matches("run: bash scripts/ci/disposable-systemd-user-contract.sh")
            .count(),
        1,
        "the workflow must contain exactly one repository-pinned transaction invocation"
    );
    assert!(
        !workflow.contains("user=codex-session-control-ci"),
        "the script, not the workflow, must own the disposable-user transaction"
    );
    assert!(
        transaction.starts_with("#!/usr/bin/env bash\nset -euo pipefail\n"),
        "the extracted transaction must start with the executable Bash header"
    );

    for required in [
        "cleanup() {",
        "trap cleanup EXIT",
        "manager=\"user@${uid}.service\"",
        "runtime_unit=\"user-runtime-dir@${uid}.service\"",
        "sudo loginctl enable-linger \"$user\" || return",
        "sudo systemctl start \"$manager\" || return",
        "sudo systemctl is-active --quiet \"$runtime_unit\" || return",
        "sudo systemctl is-active --quiet \"$manager\" || return",
        "for unit in \"$runtime_unit\" \"$manager\"; do",
        "sudo systemctl status \"$unit\" --no-pager --full",
        "sudo journalctl --unit \"$unit\" --boot --no-pager",
        "runtime=\"$(sudo loginctl show-user \"$user\" --property=RuntimePath --value)\"",
        "sudo test -S \"$runtime/bus\"",
        "DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\"",
        "systemctl --user list-units >/dev/null || return",
        "cargo test --bin codex-session-control --locked --no-run",
        "--message-format=json",
        ".reason == \"compiler-artifact\"",
        ".target.name == \"codex-session-control\"",
        ".target.kind == [\"bin\"]",
        ".profile.test == true",
        "(.executable | type == \"string\")",
        "if [[ \"$match_count\" -ne 1 ]]; then",
        "expected exactly one codex-session-control test harness, found $match_count",
        "test_harness=\"$home/codex-session-control-tests\"",
        "sudo install --owner \"$user\" --group \"$user\" --mode 0700",
        "test \"$(sudo stat --format=%u \"$test_harness\")\" = \"$uid\"",
        "test \"$(sudo stat --format=%a \"$test_harness\")\" = 700",
        "\"$test_harness\" \\",
        "--exact install::tests::disposable_systemd_user",
        "--ignored --nocapture",
        "sudo loginctl disable-linger \"$user\"",
        "sudo loginctl terminate-user \"$user\"",
        "sudo userdel --remove \"$user\"",
        "test ! -e \"$runtime\"",
        "test ! -e \"/var/lib/systemd/linger/$user\"",
        "CI=1",
        "CODEX_SESSION_CONTROL_DISPOSABLE_SYSTEMD_USER=1",
        "install::tests::disposable_systemd_user",
        "cargo test --test app_server_integration --locked --no-run",
        ".target.name == \"app_server_integration\"",
        ".target.kind == [\"test\"]",
        "expected exactly one app-server integration test harness, found $match_count",
        "cargo build --bin codex-session-control --locked",
        ".profile.test == false",
        "expected exactly one codex-session-control controller binary, found $match_count",
        "npm install --global @openai/codex@0.146.0",
        "expected exactly one npm Codex native executable, found $match_count",
        "codex-cli 0.146.0",
        "app_server_harness=\"$home/app-server-integration-tests\"",
        "controller_binary=\"$home/codex-session-control-controller\"",
        "native_codex_binary=\"$home/codex-0.146.0\"",
        "CODEX_SESSION_CONTROL_DISPOSABLE_CLI_CANARY=1",
        "CODEX_SESSION_CONTROL_CODEX_BIN=\"$native_codex_binary\"",
        "CODEX_SESSION_CONTROL_CONTROLLER_BIN=\"$controller_binary\"",
        "\"$app_server_harness\" live_normal_home_ \\",
        "config_dir=\"$home/.config/codex-session-control\"",
        "data_root=\"$home/.local/share/codex-session-control\"",
        "runtime_dir=\"$runtime/codex-session-control\"",
    ] {
        assert!(
            transaction.contains(required),
            "disposable systemd transaction is missing native-manager contract: {required}"
        );
    }

    for forbidden in [
        "dbus-run-session",
        "/usr/lib/systemd/systemd --user",
        "sudo install --directory --owner \"$user\" --group \"$user\" --mode 0700 \"$runtime\"",
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
            "disposable systemd transaction still contains manual-manager bootstrap: {forbidden}"
        );
    }

    let lifecycle = transaction
        .find("--exact install::tests::disposable_systemd_user")
        .unwrap();
    let composition = transaction
        .find("\"$app_server_harness\" live_normal_home_")
        .unwrap();
    assert!(
        lifecycle < composition,
        "the disposable lifecycle proof must complete before the CLI composition canary"
    );
    let codex_install = transaction
        .find("npm install --global @openai/codex@0.146.0")
        .unwrap();
    let codex_resolution = transaction
        .find("codex_command=\"$(command -v codex)\"")
        .unwrap();
    assert!(
        codex_install < codex_resolution,
        "Codex 0.146.0 must be installed before its native executable is resolved"
    );
    assert!(
        transaction
            .trim_end()
            .ends_with("! id \"$user\" >/dev/null 2>&1"),
        "the transaction must end with final disposable-user absence verification"
    );
}
