#[path = "cli_contract/command_surface.rs"]
mod command_surface;
#[path = "cli_contract/installer.rs"]
mod installer;
#[path = "cli_contract/release_bundle.rs"]
mod release_bundle;
#[path = "cli_contract/systemd_ci.rs"]
mod systemd_ci;
#[path = "support/private_tempdir.rs"]
mod test_support;

#[test]
fn release_assets() {
    release_bundle::assert_release_assets();
}
