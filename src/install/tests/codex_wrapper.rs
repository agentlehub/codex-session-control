use std::{ffi::OsString, os::unix::ffi::OsStringExt, process::Command};

use serde_json::Value;

use super::support::{FakeAuthority, Fixture};
use super::*;

#[tokio::test]
async fn wrapper_preflight_builds_exact_native_argv_caller_cwd_and_selected_home_environment() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let caller_cwd = std::env::current_dir().unwrap();
    let user_args = vec![
        OsString::from("--model"),
        OsString::from("two words"),
        OsString::from_vec(b"\xffraw".to_vec()),
        OsString::from("--remote"),
        OsString::from("unix://user-supplied"),
    ];

    let command = prepare_codex_wrapper(&fixture.paths, user_args.clone())
        .await
        .unwrap();

    assert_eq!(
        command.get_program(),
        fixture.fake_bin.join("codex").as_os_str()
    );
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            OsStr::new("--remote"),
            OsStr::new(&format!("unix://{}", fixture.paths.socket.display())),
            OsStr::new("--cd"),
            caller_cwd.as_os_str(),
            user_args[0].as_os_str(),
            user_args[1].as_os_str(),
            user_args[2].as_os_str(),
            user_args[3].as_os_str(),
            user_args[4].as_os_str(),
        ]
    );
    assert_eq!(
        command
            .get_envs()
            .find_map(|(key, value)| (key == OsStr::new("CODEX_HOME")).then_some(value))
            .flatten(),
        Some(fixture.paths.codex_home.as_os_str())
    );
}

#[tokio::test]
async fn wrapper_rejects_unavailable_authority_before_exec_with_status_and_enable_guidance() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    drop(authority);
    fs::remove_file(&fixture.active).unwrap();
    fs::remove_file(&fixture.paths.socket).unwrap();

    let error = prepare_codex_wrapper(&fixture.paths, Vec::new())
        .await
        .unwrap_err();

    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("codex-session-control status"));
    assert!(error.to_string().contains("codex-session-control enable"));
}

#[tokio::test]
async fn wrapper_rejects_a_valid_but_contradictory_manifest_before_exec() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let contradictory_home = fixture.paths.home.join(".codex-other");
    fs::create_dir(&contradictory_home).unwrap();
    fs::set_permissions(&contradictory_home, fs::Permissions::from_mode(0o700)).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    manifest["codexHome"] = Value::String(contradictory_home.display().to_string());
    fs::write(
        &fixture.paths.manifest,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let error = prepare_codex_wrapper(&fixture.paths, Vec::new())
        .await
        .unwrap_err();

    assert_eq!(error.exit_code(), 1);
    assert!(
        error
            .to_string()
            .contains("coherent schema-2 configuration and manifest are required")
    );
}

#[test]
fn wrapper_exec_failure_is_operational_exit_one() {
    let error = exec_codex_wrapper_command(Command::new("/missing/native-codex")).unwrap_err();

    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("cannot exec configured Codex"));
}
