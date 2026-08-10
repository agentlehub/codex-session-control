use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;
use uzers::os::unix::UserExt;

use crate::{
    error::ControllerError,
    model::{InstalledRelease, ProductConfig},
};

use super::{
    native::{marketplace_roots, product_plugins, run_codex_json},
    paths::{
        FileKind, StatusFileError, read_product_evidence_file, validate_existing,
        validate_selected_codex_home,
    },
    product_target,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct DeferredFirstInstallSelectionError {
    field: &'static str,
    reason: &'static str,
}

impl From<DeferredFirstInstallSelectionError> for ControllerError {
    fn from(value: DeferredFirstInstallSelectionError) -> Self {
        Self::InvalidData {
            field: value.field,
            reason: value.reason,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedUserPaths {
    pub euid: u32,
    pub home: PathBuf,
    pub runtime: PathBuf,
    pub binary: PathBuf,
    pub config: PathBuf,
    pub unit: PathBuf,
    pub data_root: PathBuf,
    pub codex_home: PathBuf,
    pub marketplace: PathBuf,
    pub manifest: PathBuf,
    pub runtime_dir: PathBuf,
    pub socket: PathBuf,
    first_install_selection_error: Option<DeferredFirstInstallSelectionError>,
    native_product_residue: NativeProductResidue,
    pub(super) ambient_codex_home: Option<PathBuf>,
    pub(super) native_selection_pending: bool,
}

impl ResolvedUserPaths {
    pub(crate) fn from_effective_user() -> Result<Self, ControllerError> {
        Self::from_effective_user_with_environment(
            std::env::var_os("HOME").as_deref(),
            std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        )
    }

    fn from_effective_user_with_environment(
        home_environment: Option<&OsStr>,
        runtime_environment: Option<&OsStr>,
    ) -> Result<Self, ControllerError> {
        let euid = rustix::process::geteuid().as_raw();
        let user = uzers::get_user_by_uid(euid).ok_or(ControllerError::InvalidData {
            field: "effective_user",
            reason: "passwd entry is unavailable",
        })?;
        let mut paths = Self::resolved(
            euid,
            user.home_dir().to_path_buf(),
            PathBuf::from(format!("/run/user/{euid}")),
        );
        paths.validate_invocation_identity(home_environment, runtime_environment)?;
        paths.capture_first_install_selection(None)?;
        paths.ambient_codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from);
        paths.native_selection_pending =
            classify_selected_home_evidence(&paths).case == InstalledEvidenceCase::FirstInstall;
        Ok(paths)
    }

    #[cfg(test)]
    pub(super) fn from_injected_effective_user(
        euid: u32,
        home: PathBuf,
        runtime: PathBuf,
        home_environment: Option<&OsStr>,
        runtime_environment: Option<&OsStr>,
        ambient_codex_home: Option<&OsStr>,
        native_product_residue: NativeProductResidue,
    ) -> Result<Self, ControllerError> {
        let mut paths = Self::resolved(euid, home, runtime);
        paths.resolve_first_install_selection_with_environment(
            home_environment,
            runtime_environment,
            ambient_codex_home,
            native_product_residue,
        )?;
        Ok(paths)
    }

    #[cfg(test)]
    pub(super) fn resolve_first_install_selection_with_environment(
        &mut self,
        home_environment: Option<&OsStr>,
        runtime_environment: Option<&OsStr>,
        ambient_codex_home: Option<&OsStr>,
        native_product_residue: NativeProductResidue,
    ) -> Result<(), ControllerError> {
        self.validate_invocation_identity(home_environment, runtime_environment)?;
        self.native_product_residue = native_product_residue;
        self.capture_first_install_selection(ambient_codex_home)
    }

    fn capture_first_install_selection(
        &mut self,
        ambient_codex_home: Option<&OsStr>,
    ) -> Result<(), ControllerError> {
        match selected_codex_home(self, ambient_codex_home) {
            Ok(Some(selected_home)) => self.codex_home = selected_home,
            Ok(None) => {}
            Err(ControllerError::InvalidData { field, reason }) => {
                self.first_install_selection_error =
                    Some(DeferredFirstInstallSelectionError { field, reason });
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }

    fn resolved(euid: u32, home: PathBuf, runtime: PathBuf) -> Self {
        let data_root = home.join(".local/share/codex-session-control");
        let runtime_dir = runtime.join("codex-session-control");
        Self {
            euid,
            binary: home.join(".local/bin/codex-session-control"),
            config: home.join(".config/codex-session-control/config.toml"),
            unit: home.join(".config/systemd/user/codex-session-control.service"),
            codex_home: home.join(".codex"),
            marketplace: data_root.join("marketplace"),
            manifest: data_root.join("installed-release.json"),
            socket: runtime_dir.join("app-server.sock"),
            home,
            runtime,
            data_root,
            runtime_dir,
            first_install_selection_error: None,
            native_product_residue: NativeProductResidue::Absent,
            ambient_codex_home: None,
            native_selection_pending: false,
        }
    }

    pub(super) fn validate_invocation_identity(
        &self,
        home: Option<&OsStr>,
        runtime: Option<&OsStr>,
    ) -> Result<(), ControllerError> {
        if home != Some(self.home.as_os_str()) {
            return Err(ControllerError::InvalidData {
                field: "HOME",
                reason: "does not match effective-user passwd home",
            });
        }
        if runtime != Some(self.runtime.as_os_str()) {
            return Err(ControllerError::InvalidData {
                field: "XDG_RUNTIME_DIR",
                reason: "does not match systemd user runtime",
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn for_test(euid: u32, home: PathBuf, runtime: PathBuf) -> Self {
        Self::resolved(euid, home, runtime)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InstalledEvidenceCase {
    Coherent,
    ConfigurationOnly,
    ManifestOnly,
    FirstInstall,
    PartialArtifactsWithoutIdentity,
    InvalidConfiguration,
    InvalidManifest,
    Contradictory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectedHomeOperation {
    Setup,
    Update,
    Enable,
    Disable,
    Uninstall,
    Mcp,
    Codex,
}

impl SelectedHomeOperation {
    const fn rejection_reason(self, case: InstalledEvidenceCase) -> Option<&'static str> {
        const GENERIC: &str = "selected-home identity is unavailable or contradictory";
        const COHERENT_INSTALLATION: &str = "coherent schema-2 configuration and supported schema-2 or schema-3 installed manifest are required";
        const VALID_CONFIGURATION: &str = "valid schema-2 configuration is required";

        match self {
            Self::Setup => match case {
                InstalledEvidenceCase::Coherent
                | InstalledEvidenceCase::ConfigurationOnly
                | InstalledEvidenceCase::ManifestOnly
                | InstalledEvidenceCase::FirstInstall
                | InstalledEvidenceCase::PartialArtifactsWithoutIdentity => None,
                InstalledEvidenceCase::InvalidConfiguration
                | InstalledEvidenceCase::InvalidManifest
                | InstalledEvidenceCase::Contradictory => Some(GENERIC),
            },
            Self::Update => match case {
                InstalledEvidenceCase::Coherent | InstalledEvidenceCase::ManifestOnly => None,
                InstalledEvidenceCase::ConfigurationOnly => {
                    Some("installed configuration requires same-release setup repair")
                }
                InstalledEvidenceCase::FirstInstall
                | InstalledEvidenceCase::PartialArtifactsWithoutIdentity
                | InstalledEvidenceCase::InvalidConfiguration
                | InstalledEvidenceCase::InvalidManifest
                | InstalledEvidenceCase::Contradictory => Some(GENERIC),
            },
            Self::Enable | Self::Codex => match case {
                InstalledEvidenceCase::Coherent => None,
                InstalledEvidenceCase::ConfigurationOnly
                | InstalledEvidenceCase::ManifestOnly
                | InstalledEvidenceCase::FirstInstall
                | InstalledEvidenceCase::PartialArtifactsWithoutIdentity
                | InstalledEvidenceCase::InvalidConfiguration
                | InstalledEvidenceCase::InvalidManifest
                | InstalledEvidenceCase::Contradictory => Some(COHERENT_INSTALLATION),
            },
            Self::Disable | Self::Uninstall => match case {
                InstalledEvidenceCase::Coherent | InstalledEvidenceCase::ManifestOnly => None,
                InstalledEvidenceCase::ConfigurationOnly
                | InstalledEvidenceCase::FirstInstall
                | InstalledEvidenceCase::PartialArtifactsWithoutIdentity
                | InstalledEvidenceCase::InvalidConfiguration
                | InstalledEvidenceCase::InvalidManifest
                | InstalledEvidenceCase::Contradictory => Some(GENERIC),
            },
            Self::Mcp => match case {
                InstalledEvidenceCase::Coherent | InstalledEvidenceCase::ConfigurationOnly => None,
                InstalledEvidenceCase::ManifestOnly
                | InstalledEvidenceCase::FirstInstall
                | InstalledEvidenceCase::PartialArtifactsWithoutIdentity
                | InstalledEvidenceCase::InvalidConfiguration
                | InstalledEvidenceCase::InvalidManifest
                | InstalledEvidenceCase::Contradictory => Some(VALID_CONFIGURATION),
            },
        }
    }

    pub(super) fn require_permitted_case(
        self,
        case: InstalledEvidenceCase,
    ) -> Result<(), ControllerError> {
        match self.rejection_reason(case) {
            Some(reason) => Err(ControllerError::InvalidData {
                field: "installed_evidence",
                reason,
            }),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NativeProductResidue {
    Absent,
    ExactRegistration,
    ExactCache,
    ExactRegistrationAndCache,
}

impl NativeProductResidue {
    fn from_exact_parts(registration_present: bool, cache_present: bool) -> Self {
        match (registration_present, cache_present) {
            (false, false) => Self::Absent,
            (true, false) => Self::ExactRegistration,
            (false, true) => Self::ExactCache,
            (true, true) => Self::ExactRegistrationAndCache,
        }
    }

    pub(super) fn is_present(self) -> bool {
        self != Self::Absent
    }
}

#[derive(Clone, Debug)]
pub(super) struct NativeProductState {
    pub(super) residue: NativeProductResidue,
    pub(super) marketplace_roots: Vec<String>,
    pub(super) plugins: Vec<Value>,
}

fn inspect_native_product_state(
    codex: &Path,
    codex_home: &Path,
    euid: u32,
) -> Result<NativeProductState, ControllerError> {
    match fs::symlink_metadata(codex_home) {
        Ok(_) => validate_existing(codex_home, FileKind::Directory, euid)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(NativeProductState {
                residue: NativeProductResidue::Absent,
                marketplace_roots: Vec::new(),
                plugins: Vec::new(),
            });
        }
        Err(_) => {
            return Err(ControllerError::InvalidData {
                field: "codex_home",
                reason: "cannot inspect selected home",
            });
        }
    }

    let marketplaces = run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("marketplace"),
            OsStr::new("list"),
            OsStr::new("--json"),
        ],
    )?;
    let marketplace_roots = marketplace_roots(&marketplaces)?
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let plugins = run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("list"),
            OsStr::new("--json"),
        ],
    )?;
    let plugins = product_plugins(&plugins)
        .map(|plugins| plugins.into_iter().cloned().collect::<Vec<_>>())?;
    Ok(NativeProductState {
        residue: NativeProductResidue::from_exact_parts(
            !marketplace_roots.is_empty(),
            !plugins.is_empty(),
        ),
        marketplace_roots,
        plugins,
    })
}

pub(super) fn resolve_setup_selected_home(
    paths: &mut ResolvedUserPaths,
    codex: &Path,
) -> Result<NativeProductState, ControllerError> {
    let mut native = inspect_native_product_state(codex, &paths.codex_home, paths.euid)?;
    paths.native_product_residue = native.residue;

    if !paths.native_selection_pending {
        return Ok(native);
    }

    if classify_selected_home_evidence(paths).case != InstalledEvidenceCase::FirstInstall {
        paths.native_selection_pending = false;
        return Ok(native);
    }

    let selected_home = select_first_install_codex_home(
        paths.ambient_codex_home.as_deref().map(Path::as_os_str),
        &paths.home,
        paths,
    )?;
    if native.residue.is_present() {
        paths.native_selection_pending = false;
        return Ok(native);
    }

    paths.codex_home = selected_home;
    paths.first_install_selection_error = None;
    paths.native_selection_pending = false;
    native = inspect_native_product_state(codex, &paths.codex_home, paths.euid)?;
    paths.native_product_residue = native.residue;
    Ok(native)
}

#[derive(Clone, Debug)]
pub(super) struct SelectedHomeEvidence {
    pub(super) case: InstalledEvidenceCase,
    pub(super) selected_home: Option<PathBuf>,
    pub(super) configuration: Option<ProductConfig>,
    pub(super) manifest: Option<InstalledRelease>,
    pub(super) native_product_residue: NativeProductResidue,
}

pub(super) enum StoredEvidence<T> {
    Missing,
    Valid(T),
    Invalid,
}

pub(super) fn selected_codex_home(
    paths: &ResolvedUserPaths,
    ambient_codex_home: Option<&OsStr>,
) -> Result<Option<PathBuf>, ControllerError> {
    let evidence = classify_selected_home_evidence(paths);
    match evidence.case {
        InstalledEvidenceCase::Coherent
        | InstalledEvidenceCase::ConfigurationOnly
        | InstalledEvidenceCase::ManifestOnly => Ok(evidence.selected_home),
        InstalledEvidenceCase::FirstInstall => {
            select_first_install_codex_home(ambient_codex_home, &paths.home, paths).map(Some)
        }
        InstalledEvidenceCase::PartialArtifactsWithoutIdentity
        | InstalledEvidenceCase::InvalidConfiguration
        | InstalledEvidenceCase::InvalidManifest
        | InstalledEvidenceCase::Contradictory => Ok(None),
    }
}

pub(super) fn select_first_install_codex_home(
    ambient_codex_home: Option<&OsStr>,
    passwd_home: &Path,
    paths: &ResolvedUserPaths,
) -> Result<PathBuf, ControllerError> {
    let selected_home = ambient_codex_home
        .map(PathBuf::from)
        .unwrap_or_else(|| passwd_home.join(".codex"));
    validate_selected_codex_home(
        &selected_home,
        &paths.config,
        &paths.home,
        &paths.data_root,
        &paths.runtime_dir,
        paths.euid,
    )?;
    Ok(selected_home)
}

pub(super) fn classify_selected_home_evidence(paths: &ResolvedUserPaths) -> SelectedHomeEvidence {
    classify_selected_home_evidence_with_native_product_artifact(
        paths,
        paths.native_product_residue,
    )
}

pub(super) fn classify_selected_home_evidence_with_native_product_artifact(
    paths: &ResolvedUserPaths,
    native_product_residue: NativeProductResidue,
) -> SelectedHomeEvidence {
    let configuration = read_configuration_evidence(paths);
    let manifest = read_manifest_evidence(paths);
    let selected_home = match (&configuration, &manifest) {
        (StoredEvidence::Valid(configuration), StoredEvidence::Valid(manifest))
            if configuration.codex_executable == manifest.codex_executable
                && configuration.codex_home == manifest.codex_home
                && configuration.socket_path == manifest.socket_path =>
        {
            Some(configuration.codex_home.clone())
        }
        (StoredEvidence::Valid(_), StoredEvidence::Valid(_)) => None,
        (StoredEvidence::Valid(configuration), StoredEvidence::Missing) => {
            Some(configuration.codex_home.clone())
        }
        (StoredEvidence::Missing, StoredEvidence::Valid(manifest)) => {
            Some(manifest.codex_home.clone())
        }
        _ => None,
    };
    let case = match (&configuration, &manifest) {
        (StoredEvidence::Invalid, _) => InstalledEvidenceCase::InvalidConfiguration,
        (_, StoredEvidence::Invalid) => InstalledEvidenceCase::InvalidManifest,
        (StoredEvidence::Valid(_), StoredEvidence::Valid(_)) if selected_home.is_some() => {
            InstalledEvidenceCase::Coherent
        }
        (StoredEvidence::Valid(_), StoredEvidence::Valid(_)) => {
            InstalledEvidenceCase::Contradictory
        }
        (StoredEvidence::Valid(_), StoredEvidence::Missing) => {
            InstalledEvidenceCase::ConfigurationOnly
        }
        (StoredEvidence::Missing, StoredEvidence::Valid(_)) => InstalledEvidenceCase::ManifestOnly,
        (StoredEvidence::Missing, StoredEvidence::Missing)
            if product_artifact_is_present(paths) || native_product_residue.is_present() =>
        {
            InstalledEvidenceCase::PartialArtifactsWithoutIdentity
        }
        (StoredEvidence::Missing, StoredEvidence::Missing) => InstalledEvidenceCase::FirstInstall,
    };
    SelectedHomeEvidence {
        case,
        selected_home,
        configuration: match &configuration {
            StoredEvidence::Valid(configuration) => Some(configuration.clone()),
            _ => None,
        },
        manifest: match &manifest {
            StoredEvidence::Valid(manifest) => Some(manifest.clone()),
            _ => None,
        },
        native_product_residue,
    }
}

pub(super) fn require_selected_home_evidence(
    paths: &ResolvedUserPaths,
    operation: SelectedHomeOperation,
) -> Result<SelectedHomeEvidence, ControllerError> {
    let evidence = classify_selected_home_evidence(paths);
    if evidence.case == InstalledEvidenceCase::FirstInstall
        && let Some(error) = paths.first_install_selection_error
    {
        return Err(error.into());
    }
    operation.require_permitted_case(evidence.case)?;
    if let Some(selected_home) = &evidence.selected_home
        && selected_home != &paths.codex_home
    {
        return Err(ControllerError::InvalidData {
            field: "codex_home",
            reason: "does not match persisted selected-home identity",
        });
    }
    if evidence.case == InstalledEvidenceCase::FirstInstall {
        validate_selected_codex_home(
            &paths.codex_home,
            &paths.config,
            &paths.home,
            &paths.data_root,
            &paths.runtime_dir,
            paths.euid,
        )?;
    }
    Ok(evidence)
}

pub(super) fn read_configuration_evidence(
    paths: &ResolvedUserPaths,
) -> StoredEvidence<ProductConfig> {
    let bytes = match read_product_evidence_file(&paths.home, paths.euid, &paths.config, 0o600) {
        Ok(bytes) => bytes,
        Err(StatusFileError::Missing) => return StoredEvidence::Missing,
        Err(_) => return StoredEvidence::Invalid,
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return StoredEvidence::Invalid;
    };
    if let Ok(configuration) = toml::from_str::<ProductConfig>(text) {
        return if configuration
            .validate(&configuration.codex_home, &paths.socket)
            .and_then(|()| {
                validate_selected_codex_home(
                    &configuration.codex_home,
                    &paths.config,
                    &paths.home,
                    &paths.data_root,
                    &paths.runtime_dir,
                    paths.euid,
                )
            })
            .is_ok()
        {
            StoredEvidence::Valid(configuration)
        } else {
            StoredEvidence::Invalid
        };
    }
    StoredEvidence::Invalid
}

pub(super) fn read_manifest_evidence(
    paths: &ResolvedUserPaths,
) -> StoredEvidence<InstalledRelease> {
    let bytes = match read_product_evidence_file(&paths.home, paths.euid, &paths.manifest, 0o600) {
        Ok(bytes) => bytes,
        Err(StatusFileError::Missing) => return StoredEvidence::Missing,
        Err(_) => return StoredEvidence::Invalid,
    };
    if let Ok(manifest) = serde_json::from_slice::<InstalledRelease>(&bytes) {
        return if manifest
            .validate(&manifest.codex_home, &paths.socket)
            .and_then(|()| {
                validate_selected_codex_home(
                    &manifest.codex_home,
                    &paths.config,
                    &paths.home,
                    &paths.data_root,
                    &paths.runtime_dir,
                    paths.euid,
                )
            })
            .is_ok()
            && manifest.target == product_target()
        {
            StoredEvidence::Valid(manifest)
        } else {
            StoredEvidence::Invalid
        };
    }
    StoredEvidence::Invalid
}

fn product_artifact_is_present(paths: &ResolvedUserPaths) -> bool {
    [
        paths.binary.as_path(),
        paths.unit.as_path(),
        paths.marketplace.as_path(),
    ]
    .into_iter()
    .any(|path| fs::symlink_metadata(path).is_ok())
}

pub(crate) fn load_installed_config() -> Result<ProductConfig, ControllerError> {
    let paths = ResolvedUserPaths::from_effective_user()?;
    load_config_from_paths(&paths)
}

pub(super) fn load_config_from_paths(
    paths: &ResolvedUserPaths,
) -> Result<ProductConfig, ControllerError> {
    let evidence = require_selected_home_evidence(paths, SelectedHomeOperation::Mcp)?;
    let expected = evidence.configuration.ok_or(ControllerError::InvalidData {
        field: "config",
        reason: "invalid installed configuration",
    })?;
    let bytes = read_product_evidence_file(&paths.home, paths.euid, &paths.config, 0o600).map_err(
        |_| ControllerError::InvalidData {
            field: "config",
            reason: "cannot read installed configuration",
        },
    )?;
    let text = std::str::from_utf8(&bytes).map_err(|_| ControllerError::InvalidData {
        field: "config",
        reason: "invalid installed configuration",
    })?;
    let config: ProductConfig = toml::from_str(text).map_err(|_| ControllerError::InvalidData {
        field: "config",
        reason: "invalid installed configuration",
    })?;
    config.validate(&paths.codex_home, &paths.socket)?;
    if config != expected {
        return Err(ControllerError::InvalidData {
            field: "config",
            reason: "changed during validation",
        });
    }
    Ok(config)
}
