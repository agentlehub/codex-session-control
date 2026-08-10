use assert_cmd::Command;
use predicates::prelude::*;

const COMMANDS: [&str; 7] = [
    "setup",
    "update",
    "status",
    "enable",
    "disable",
    "uninstall",
    "codex",
];

const APPROVED_ROOT_HELP: &str = concat!(
    "Manage Codex Session Control\n",
    "\n",
    "Usage: codex-session-control [OPTIONS] <COMMAND>\n",
    "\n",
    "Commands:\n",
    "  setup      Install Codex Session Control and start the shared app-server\n",
    "  update     Install the latest release\n",
    "  status     Check whether Codex Session Control is ready\n",
    "  enable     Start the service and turn on automatic startup\n",
    "  disable    Stop the service and turn off automatic startup\n",
    "  uninstall  Remove the service while keeping your Codex data\n",
    "  codex      Start Codex CLI through the shared app-server\n",
    "\n",
    "Options:\n",
    "      --verbose  Show diagnostic details\n",
    "  -h, --help     Print help\n",
    "  -V, --version  Print version\n",
);

const APPROVED_SETUP_HELP: &str = concat!(
    "Install Codex Session Control and start the shared app-server\n",
    "\n",
    "Usage: codex-session-control setup [OPTIONS]\n",
    "\n",
    "Options:\n",
    "      --desktop-launcher <PATH>  Absolute path to the Codex Desktop executable when automatic discovery fails\n",
    "      --verbose                  Show diagnostic details\n",
    "  -h, --help                     Print help\n",
);

const APPROVED_UPDATE_HELP: &str = concat!(
    "Install the latest release\n",
    "\n",
    "Usage: codex-session-control update [OPTIONS]\n",
    "\n",
    "Options:\n",
    "      --verbose  Show diagnostic details\n",
    "  -h, --help     Print help\n",
);

const APPROVED_STATUS_HELP: &str = concat!(
    "Check whether Codex Session Control is ready\n",
    "\n",
    "Usage: codex-session-control status [OPTIONS]\n",
    "\n",
    "Options:\n",
    "      --verbose  Show diagnostic details\n",
    "  -h, --help     Print help\n",
);

const APPROVED_ENABLE_HELP: &str = concat!(
    "Start the service and turn on automatic startup\n",
    "\n",
    "Usage: codex-session-control enable [OPTIONS]\n",
    "\n",
    "Options:\n",
    "      --verbose  Show diagnostic details\n",
    "  -h, --help     Print help\n",
);

const APPROVED_DISABLE_HELP: &str = concat!(
    "Stop the service and turn off automatic startup\n",
    "\n",
    "Usage: codex-session-control disable [OPTIONS]\n",
    "\n",
    "Options:\n",
    "      --verbose  Show diagnostic details\n",
    "  -h, --help     Print help\n",
);

const APPROVED_UNINSTALL_HELP: &str = concat!(
    "Remove the service while keeping your Codex data\n",
    "\n",
    "Usage: codex-session-control uninstall [OPTIONS]\n",
    "\n",
    "Options:\n",
    "      --verbose  Show diagnostic details\n",
    "  -h, --help     Print help\n",
);

const APPROVED_CODEX_HELP: &str = concat!(
    "Start Codex CLI through the shared app-server\n",
    "\n",
    "Usage: codex-session-control codex [ARGS]...\n",
    "\n",
    "Arguments:\n",
    "  [ARGS]...  Arguments passed directly to Codex CLI\n",
    "\n",
    "Options:\n",
    "  -h, --help  Print help\n",
);

#[test]
fn root_help_matches_approved_contract_and_hides_mcp_server() {
    let output = Command::cargo_bin("codex-session-control")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout, APPROVED_ROOT_HELP);
    let listed: Vec<&str> = stdout
        .lines()
        .filter_map(|line| {
            let token = line.split_whitespace().next()?;
            COMMANDS.contains(&token).then_some(token)
        })
        .collect();
    assert_eq!(listed, COMMANDS);
    for forbidden in [
        "install", "repair", "start", "stop", "restart", "logs", "doctor", "help",
    ] {
        assert!(
            !stdout
                .lines()
                .any(|line| line.trim().starts_with(forbidden))
        );
    }
}

#[test]
fn detailed_help_matches_the_approved_contract() {
    for (args, expected) in [
        (&["setup", "--help"][..], APPROVED_SETUP_HELP),
        (&["update", "--help"][..], APPROVED_UPDATE_HELP),
        (&["status", "--help"][..], APPROVED_STATUS_HELP),
        (&["enable", "--help"][..], APPROVED_ENABLE_HELP),
        (&["disable", "--help"][..], APPROVED_DISABLE_HELP),
        (&["uninstall", "--help"][..], APPROVED_UNINSTALL_HELP),
        (&["codex", "--help"][..], APPROVED_CODEX_HELP),
    ] {
        let output = Command::cargo_bin("codex-session-control")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn clap_rejects_aliases_with_exit_code_two() {
    for alias in [
        "install", "repair", "start", "stop", "restart", "logs", "doctor", "help",
    ] {
        Command::cargo_bin("codex-session-control")
            .unwrap()
            .arg(alias)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unrecognized subcommand"));
    }
}

#[test]
fn setup_exposes_no_path_unit_or_reconciliation_controls() {
    Command::cargo_bin("codex-session-control")
        .unwrap()
        .args(["setup", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Usage: codex-session-control setup",
        ));

    for forbidden in [
        "--home",
        "--runtime-dir",
        "--codex-home",
        "--socket",
        "--unit",
        "--unit-name",
        "--path",
        "--force",
    ] {
        Command::cargo_bin("codex-session-control")
            .unwrap()
            .args(["setup", forbidden])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
    }
}

#[test]
fn setup_desktop_launcher_accepts_only_one_absolute_executable_path() {
    Command::cargo_bin("codex-session-control")
        .unwrap()
        .args([
            "setup",
            "--desktop-launcher",
            "/opt/codex-desktop",
            "--help",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("--desktop-launcher <PATH>"));

    for rejected in [
        "relative/codex-desktop",
        "codex-desktop --print-build-info",
        "../codex-desktop",
        "/opt/codex-desktop.desktop",
    ] {
        Command::cargo_bin("codex-session-control")
            .unwrap()
            .args(["setup", "--desktop-launcher", rejected, "--help"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("invalid value"));
    }

    Command::cargo_bin("codex-session-control")
        .unwrap()
        .args([
            "setup",
            "--desktop-launcher",
            "/opt/codex-desktop",
            "extra",
            "--help",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected argument"));
}

#[test]
fn desktop_start_lifecycle_is_parser_only_at_the_cli_boundary() {
    Command::cargo_bin("codex-session-control")
        .unwrap()
        .args([
            "setup",
            "--desktop-launcher",
            "/opt/codex-desktop",
            "--help",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("--desktop-launcher <PATH>"));
}

#[test]
fn status_has_no_mutation_or_target_selection_controls() {
    Command::cargo_bin("codex-session-control")
        .unwrap()
        .args(["status", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Usage: codex-session-control status",
        ));

    for forbidden in [
        "--repair",
        "--force",
        "--unit",
        "--unit-name",
        "--codex-home",
        "--socket",
        "--path",
    ] {
        Command::cargo_bin("codex-session-control")
            .unwrap()
            .args(["status", forbidden])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
    }
}

#[test]
fn enable_disable_have_no_target_or_bypass_controls() {
    for command in ["enable", "disable"] {
        Command::cargo_bin("codex-session-control")
            .unwrap()
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains(format!(
                "Usage: codex-session-control {command}"
            )));

        for forbidden in [
            "--force",
            "--unit",
            "--unit-name",
            "--codex-home",
            "--socket",
            "--path",
        ] {
            Command::cargo_bin("codex-session-control")
                .unwrap()
                .args([command, forbidden])
                .assert()
                .code(2)
                .stderr(predicate::str::contains("unexpected argument"));
        }
    }
}

#[test]
fn mcp_server_has_no_selected_home_or_socket_override() {
    for forbidden in [
        "--force",
        "--unit",
        "--unit-name",
        "--codex-home",
        "--socket",
        "--path",
    ] {
        Command::cargo_bin("codex-session-control")
            .unwrap()
            .args(["mcp-server", forbidden])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
    }
}

#[test]
fn uninstall_exposes_the_preservation_boundary_and_no_bypass_controls() {
    Command::cargo_bin("codex-session-control")
        .unwrap()
        .args(["uninstall", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Usage: codex-session-control uninstall").and(
                predicate::str::contains("Remove the service while keeping your Codex data"),
            ),
        );

    for forbidden in [
        "--force",
        "--purge",
        "--remove-sessions",
        "--unit",
        "--unit-name",
        "--codex-home",
        "--socket",
        "--path",
    ] {
        Command::cargo_bin("codex-session-control")
            .unwrap()
            .args(["uninstall", forbidden])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
    }
}

#[test]
fn update_exposes_no_public_staging_restart_or_release_controls() {
    Command::cargo_bin("codex-session-control")
        .unwrap()
        .args(["update", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Usage: codex-session-control update",
        ));

    for forbidden in [
        "--staged",
        "--force",
        "--yes",
        "--restart",
        "--release",
        "--tag",
        "--asset",
        "--url",
        "--unit",
        "--codex-home",
        "--socket",
    ] {
        Command::cargo_bin("codex-session-control")
            .unwrap()
            .args(["update", forbidden])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
    }
}

#[test]
fn lifecycle_failure_retry_has_no_public_failpoint_control() {
    for command in [
        "setup",
        "update",
        "status",
        "enable",
        "disable",
        "uninstall",
        "mcp-server",
    ] {
        Command::cargo_bin("codex-session-control")
            .unwrap()
            .args([command, "--fail-after-completed-stage", "binary"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
    }
}
