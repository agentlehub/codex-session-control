use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use uzers::os::unix::UserExt;

pub(crate) fn effective_user_home() -> PathBuf {
    let euid = rustix::process::geteuid().as_raw();
    uzers::get_user_by_uid(euid)
        .unwrap_or_else(|| panic!("effective uid {euid} has no passwd entry"))
        .home_dir()
        .to_owned()
}

pub(crate) fn private_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(".codex-session-control-test.")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(effective_user_home())
        .expect("create private test directory under effective user's passwd home")
}
