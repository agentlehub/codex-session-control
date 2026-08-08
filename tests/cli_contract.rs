#[path = "cli_contract/command_surface.rs"]
mod command_surface;
#[path = "cli_contract/installer.rs"]
mod installer;
#[path = "cli_contract/release_bundle.rs"]
mod release_bundle;
#[path = "support/private_tempdir.rs"]
mod test_support;

#[test]
#[ignore = "requires CODEX_SESSION_CONTROL_RELEASE_DIR"]
fn release_assets() {
    release_bundle::assert_release_assets();
}
