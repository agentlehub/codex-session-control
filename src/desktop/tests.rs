use std::{collections::BTreeMap, ffi::OsString, fs, os::unix::fs::PermissionsExt, path::Path};

fn environment(home: &Path) -> BTreeMap<OsString, OsString> {
    BTreeMap::from([
        (OsString::from("HOME"), home.as_os_str().to_owned()),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ])
}

fn write_file(path: &Path, contents: impl AsRef<[u8]>, mode: u32) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn write_executable_fixture(path: &Path, contents: impl AsRef<[u8]>) {
    let stage = path.with_extension("stage");
    fs::write(&stage, contents).unwrap();
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
    fs::File::open(&stage).unwrap().sync_all().unwrap();
    fs::rename(stage, path).unwrap();
}

fn write_launcher(path: &Path, log: &Path, app_id: &str, capabilities: &str) {
    write_executable_fixture(
        path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{{\"appIdentity\":{{\"id\":\"{app_id}\"}},\"linuxCapabilities\":{capabilities}}}'\n",
            log.display()
        ),
    );
}

fn write_environment_launcher(path: &Path, argv_log: &Path, environment_log: &Path) {
    write_executable_fixture(
        path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'HOME=%s\\nXDG_CONFIG_HOME=%s\\n' \"$HOME\" \"$XDG_CONFIG_HOME\" > '{}'\nprintf '%s\\n' '{{\"appIdentity\":{{\"id\":\"codex-desktop\"}},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}}'\n",
            argv_log.display(),
            environment_log.display(),
        ),
    );
}

mod descriptor;
mod discovery;
mod entry;
