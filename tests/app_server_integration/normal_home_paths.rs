use std::{
    error::Error,
    fs::{self, File},
    io::{self, Write},
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use uzers::os::unix::UserExt;

pub(super) const DISPOSABLE_CLI_CANARY_OPT_IN: &str = "CODEX_SESSION_CONTROL_DISPOSABLE_CLI_CANARY";
pub(super) const CONFIG_TEMPLATE: &str = r#"model = "session-control-test"
model_provider = "session-control-local"

[model_providers.session-control-local]
name = "Session control local test"
base_url = "http://127.0.0.1:__PORT__/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = 10000

[analytics]
enabled = false
"#;

pub(super) struct DisposablePaths {
    pub(super) config_dir: PathBuf,
    pub(super) data_root: PathBuf,
    pub(super) codex_home: PathBuf,
    pub(super) runtime_dir: PathBuf,
    pub(super) socket: PathBuf,
    pub(super) home: PathBuf,
    pub(super) runtime: PathBuf,
}

impl DisposablePaths {
    pub(super) fn normal_home(home: PathBuf, runtime: PathBuf) -> Self {
        let config_dir = home.join(".config/codex-session-control");
        let data_root = home.join(".local/share/codex-session-control");
        let codex_home = home.join(".codex");
        let runtime_dir = runtime.join("codex-session-control");
        let socket = runtime_dir.join("app-server.sock");
        Self {
            config_dir,
            data_root,
            codex_home,
            runtime_dir,
            socket,
            home,
            runtime,
        }
    }

    pub(super) fn claim(codex: &Path, endpoint_port: u16) -> Result<Self, Box<dyn Error>> {
        Self::claim_with_product_state(codex, endpoint_port, true)
    }

    pub(super) fn claim_for_product(
        codex: &Path,
        endpoint_port: u16,
    ) -> Result<Self, Box<dyn Error>> {
        Self::claim_with_product_state(codex, endpoint_port, false)
    }

    pub(super) fn claim_with_product_state(
        codex: &Path,
        endpoint_port: u16,
        create_product_state: bool,
    ) -> Result<Self, Box<dyn Error>> {
        if std::env::var_os(DISPOSABLE_CLI_CANARY_OPT_IN).as_deref()
            != Some(std::ffi::OsStr::new("1"))
            || std::env::var_os("CI").as_deref() != Some(std::ffi::OsStr::new("1"))
        {
            return Err("disposable CLI canary requires the explicit CI opt-in".into());
        }
        let euid = rustix::process::geteuid().as_raw();
        if euid == 0 {
            return Err("disposable CLI canary must not run as root".into());
        }
        let user = uzers::get_user_by_uid(euid).ok_or("disposable passwd user is unavailable")?;
        if user.name() != std::ffi::OsStr::new("codex-session-control-ci") {
            return Err("CLI canary is restricted to the CI disposable passwd user".into());
        }
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unavailable")?);
        if user.home_dir() != home {
            return Err("disposable CLI canary HOME does not match passwd".into());
        }
        let runtime =
            PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("runtime is unavailable")?);
        let expected_runtime = format!("/run/user/{euid}");
        if runtime != Path::new(&expected_runtime) {
            return Err("disposable CLI canary runtime is not canonical".into());
        }
        let bus = runtime.join("bus");
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS")
            != Some(format!("unix:path={}", bus.display()).into())
        {
            return Err("disposable CLI canary bus address is not canonical".into());
        }
        let bus_metadata = fs::symlink_metadata(&bus)?;
        if !bus_metadata.file_type().is_socket() || bus_metadata.uid() != euid {
            return Err("disposable CLI canary user bus is unavailable".into());
        }
        for parent in [
            home.clone(),
            home.join(".config"),
            home.join(".local"),
            home.join(".local/share"),
            runtime.clone(),
        ] {
            let metadata = fs::symlink_metadata(&parent)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.uid() != euid
                || metadata.mode() & 0o022 != 0
            {
                return Err(format!("unsafe disposable parent: {}", parent.display()).into());
            }
        }
        let fixture = Self::normal_home(home, runtime);
        for path in fixture.absent_paths() {
            match fs::symlink_metadata(path) {
                Ok(_) => {
                    return Err(format!(
                        "disposable canonical path was not absent: {}",
                        path.display()
                    )
                    .into());
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }

        let directories = if create_product_state {
            vec![
                fixture.config_dir.as_path(),
                fixture.data_root.as_path(),
                fixture.codex_home.as_path(),
                fixture.runtime_dir.as_path(),
            ]
        } else {
            vec![fixture.codex_home.as_path()]
        };
        for directory in directories {
            fs::create_dir(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        atomic_write(
            &fixture.codex_home.join("config.toml"),
            CONFIG_TEMPLATE
                .replace("__PORT__", &endpoint_port.to_string())
                .as_bytes(),
            0o600,
        )?;
        if create_product_state {
            let product_config = toml::Value::Table(toml::Table::from_iter([
                ("schema_version".to_owned(), toml::Value::Integer(2)),
                (
                    "codex_executable".to_owned(),
                    toml::Value::String(codex.to_string_lossy().into_owned()),
                ),
                (
                    "codex_home".to_owned(),
                    toml::Value::String(fixture.codex_home.to_string_lossy().into_owned()),
                ),
                (
                    "socket_path".to_owned(),
                    toml::Value::String(fixture.socket.to_string_lossy().into_owned()),
                ),
            ]));
            atomic_write(
                &fixture.config_dir.join("config.toml"),
                toml::to_string(&product_config)?.as_bytes(),
                0o600,
            )?;
        }
        Ok(fixture)
    }

    pub(super) fn absent_paths(&self) -> [&Path; 4] {
        [
            &self.config_dir,
            &self.data_root,
            &self.codex_home,
            &self.runtime_dir,
        ]
    }
}

impl Drop for DisposablePaths {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.runtime_dir);
        let _ = fs::remove_dir_all(&self.data_root);
        let _ = fs::remove_dir_all(&self.codex_home);
        let _ = fs::remove_dir_all(&self.config_dir);
    }
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().ok_or("atomic destination lacks parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
