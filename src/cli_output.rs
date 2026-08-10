use std::path::PathBuf;

use semver::Version;

use crate::install::shell_quote_path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RenderedCli {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) exit_code: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UserSuccess {
    Setup(SetupSuccess),
    Update(UpdateSuccess),
    Enable(EnableSuccess),
    Disable(DisableSuccess),
    Uninstall(UninstallSuccess),
    Status(StatusResult),
}

impl UserSuccess {
    pub(crate) fn render(&self) -> RenderedCli {
        match self {
            Self::Setup(success) => success.render(),
            Self::Update(success) => success.render(),
            Self::Enable(success) => success.render(),
            Self::Disable(success) => success.render(),
            Self::Uninstall(success) => success.render(),
            Self::Status(status) => status.render(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunningClientFacts {
    pub(crate) cli: bool,
    pub(crate) desktop: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DesktopAvailability {
    Available,
    Unavailable,
    CouldNotVerify,
    SetupRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UserNotice {
    Compatibility { codex: Version, product: Version },
    DesktopLauncherUnavailable,
    LocalBinMissingFromPath { local_bin: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SetupSuccess {
    version: Version,
    running: RunningClientFacts,
    desktop: DesktopAvailability,
    desktop_changed: bool,
    notices: Vec<UserNotice>,
}

impl SetupSuccess {
    pub(crate) fn new(
        version: Version,
        running: RunningClientFacts,
        desktop: DesktopAvailability,
        desktop_changed: bool,
        notices: Vec<UserNotice>,
    ) -> Option<Self> {
        if desktop_changed
            && matches!(
                desktop,
                DesktopAvailability::Unavailable | DesktopAvailability::CouldNotVerify
            )
        {
            return None;
        }
        Some(Self {
            version,
            running,
            desktop,
            desktop_changed,
            notices,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateState {
    Applied,
    AlreadyCurrent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpdateSuccess {
    state: UpdateState,
    version: Version,
    service_enabled: bool,
    desktop_changed: bool,
    notices: Vec<UserNotice>,
}

impl UpdateSuccess {
    pub(crate) fn new(
        state: UpdateState,
        version: Version,
        service_enabled: bool,
        desktop_changed: bool,
        notices: Vec<UserNotice>,
    ) -> Self {
        Self {
            state,
            version,
            service_enabled,
            desktop_changed,
            notices,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EnableSuccess {
    running: RunningClientFacts,
    desktop: DesktopAvailability,
    desktop_changed: bool,
    notices: Vec<UserNotice>,
}

impl EnableSuccess {
    pub(crate) fn new(
        running: RunningClientFacts,
        desktop: DesktopAvailability,
        desktop_changed: bool,
        notices: Vec<UserNotice>,
    ) -> Option<Self> {
        if desktop_changed
            && matches!(
                desktop,
                DesktopAvailability::Unavailable
                    | DesktopAvailability::CouldNotVerify
                    | DesktopAvailability::SetupRequired
            )
        {
            return None;
        }
        Some(Self {
            running,
            desktop,
            desktop_changed,
            notices,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DisableSuccess {
    desktop_removed: bool,
}

impl DisableSuccess {
    pub(crate) fn new(desktop_removed: bool) -> Self {
        Self { desktop_removed }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UninstallSuccess {
    desktop_removed: bool,
}

impl UninstallSuccess {
    pub(crate) fn new(desktop_removed: bool) -> Self {
        Self { desktop_removed }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UserFailure {
    Ordinary(OrdinaryFailure),
    RollbackIncomplete(RollbackIncomplete),
    StopThenRetry(StopThenRetry),
    ManualCleanup(ManualCleanup),
    VerifiedRelease(VerifiedReleaseRecovery),
    IndependentTerminal(IndependentTerminal),
    InteractiveTerminal,
    PartialDisable(PartialDisable),
    TerminalPartialUninstall(TerminalPartialUninstall),
    UpdateCompletionUnknown,
    Cancellation,
    WrapperUnavailable,
}

impl UserFailure {
    pub(crate) fn render(&self) -> RenderedCli {
        failure(match self {
            Self::Ordinary(failure) => render_ordinary_failure(failure),
            Self::RollbackIncomplete(failure) => failure.render(),
            Self::StopThenRetry(failure) => failure.render(),
            Self::ManualCleanup(failure) => failure.render(),
            Self::VerifiedRelease(failure) => failure.render(),
            Self::IndependentTerminal(failure) => failure.render(),
            Self::InteractiveTerminal => failure_block(
                "Codex Session Control could not be updated.",
                "The operation could not safely continue from this terminal.",
                "Run the update from an interactive terminal:\n  codex-session-control update\n",
            ),
            Self::PartialDisable(failure) => failure.render(),
            Self::TerminalPartialUninstall(failure) => failure.render(),
            Self::UpdateCompletionUnknown => concat!(
                "Codex Session Control could not confirm that the update completed.\n",
                "\n",
                "The installed Codex Session Control state could not be verified.\n",
                "\n",
                "Check what needs attention:\n",
                "  codex-session-control status\n",
            )
            .to_owned(),
            Self::Cancellation => concat!(
                "Codex Session Control was not updated.\n",
                "\n",
                "The update was canceled before installation files changed.\n",
            )
            .to_owned(),
            Self::WrapperUnavailable => concat!(
                "Codex CLI could not start because Codex Session Control is unavailable.\n",
                "\n",
                "Check what needs attention:\n",
                "  codex-session-control status\n",
            )
            .to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OrdinaryFailure {
    SetupUnsafeTerminalRetry,
    SetupUnexpectedRetry,
    SetupInstalledStateCheckStatus,
    SetupInstallationFilesRetryUpdate,
    SetupInstalledStateRepair {
        binary: PathBuf,
    },
    SetupCliIntegrationRetry,
    SetupCliIntegrationCheckStatus,
    SetupInstallationFilesRetry,
    SetupServiceConfigurationRetry,
    SetupDesktopIntegrationRetry,
    SetupDesktopIntegrationCheckStatus,
    SetupServiceStartRetry,
    SetupServiceStateRetryUpdate,
    UpdateUnexpectedRetry,
    UpdateInstalledStateCheckStatus,
    UpdateReleaseRetry,
    UpdateChecksumRetry,
    UpdateCliIntegrationRetry,
    UpdateCliIntegrationCheckStatus,
    UpdateServiceConfigurationRetry,
    UpdateServiceStateCheckStatus,
    UpdateActiveTasksRetry,
    UpdateInstallationFilesRetry,
    UpdateDesktopIntegrationRetry,
    UpdateDesktopIntegrationCheckStatus,
    UpdateServiceConfigurationLogs,
    UpdateServiceStartLogs,
    UpdateServiceStateLogs,
    UpdateInstalledStatePostMutationCheckStatus,
    EnableUnexpectedRetry,
    EnableInstalledStateRepairSetup,
    EnableServiceConfigurationRepairSetup,
    EnableDesktopIntegrationCheckStatus,
    EnableDesktopIntegrationRetry,
    EnableServiceStartRetry,
    EnableServiceStateRetry,
    #[cfg(test)]
    EnableUnexpectedCheckStatus,
    DisableUnexpectedRetry,
    DisableServiceStopRetry,
    #[cfg(test)]
    DisableUnexpectedCheckStatus,
    UninstallUnexpectedRetry,
    UninstallServiceStopRetry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StopThenRetry {
    UpdateServiceStateDisableUpdateEnable,
    EnableServiceStartStopThenEnable,
    EnableServiceStateStopThenEnable,
    DisableUnsafeStopThenDisable,
    DisableServiceStopThenDisable,
    DisableServiceStateStopThenDisable,
    UninstallUnsafeStopThenUninstall,
    UninstallServiceStateStopThenUninstall,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IndependentTerminal {
    Update,
    Disable,
    Uninstall,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeCleanupCommand {
    RemovePlugin,
    RemoveMarketplace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedPaths {
    first: PathBuf,
    rest: Vec<PathBuf>,
}

impl ManagedPaths {
    pub(crate) fn new(first: PathBuf, rest: Vec<PathBuf>) -> Self {
        Self { first, rest }
    }

    fn iter(&self) -> impl Iterator<Item = &PathBuf> {
        std::iter::once(&self.first).chain(&self.rest)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RollbackPrimary {
    SetupDesktopRetry,
    SetupServiceConfigurationRetry,
    SetupServiceStartRetry,
    SetupServiceStateRetryUpdate,
    UpdateDesktopRetry,
    EnableDesktopRetry,
    EnableServiceStateCheckStatus,
    UninstallInstalledStateCheckStatus,
    UninstallDesktopCheckStatus,
    UninstallCleanupRetry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RollbackIncomplete {
    primary: RollbackPrimary,
    paths: ManagedPaths,
}

impl RollbackIncomplete {
    pub(crate) fn new(primary: RollbackPrimary, paths: ManagedPaths) -> Self {
        Self { primary, paths }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManualCleanup {
    command: NativeCleanupCommand,
    codex_home: PathBuf,
    codex_executable: Option<PathBuf>,
}

impl ManualCleanup {
    pub(crate) fn new(
        command: NativeCleanupCommand,
        codex_home: PathBuf,
        codex_executable: Option<PathBuf>,
    ) -> Self {
        Self {
            command,
            codex_home,
            codex_executable,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedReleaseRecovery {
    release_url: String,
    checksums_url: String,
}

impl VerifiedReleaseRecovery {
    pub(crate) fn new(release_url: String, checksums_url: String) -> Self {
        Self {
            release_url,
            checksums_url,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PartialDisable {
    managed_path: Option<PathBuf>,
}

impl PartialDisable {
    pub(crate) fn new(managed_path: Option<PathBuf>) -> Self {
        Self { managed_path }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalPartialUninstall {
    remaining: ManagedPaths,
}

impl TerminalPartialUninstall {
    pub(crate) fn new(remaining: ManagedPaths) -> Self {
        Self { remaining }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatusState {
    Healthy,
    Disabled,
    NotInstalled,
    Unhealthy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntegrationState {
    Ready,
    Unavailable,
    Unhealthy,
    CouldNotVerify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServiceSummary {
    RunningAutomatic,
    StoppedAutomaticOff,
    StoppedUnexpectedAutomaticOn,
    CouldNotVerify,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StatusResult {
    state: StatusState,
    version: Option<Version>,
    service: Option<ServiceSummary>,
    cli: IntegrationState,
    desktop: IntegrationState,
    problems: Vec<StatusProblem>,
}

impl StatusResult {
    pub(crate) fn new(
        state: StatusState,
        version: Option<Version>,
        service: Option<ServiceSummary>,
        cli: IntegrationState,
        desktop: IntegrationState,
        problems: Vec<StatusProblem>,
    ) -> Self {
        Self {
            state,
            version,
            service,
            cli,
            desktop,
            problems,
        }
    }

    pub(crate) fn state(&self) -> StatusState {
        self.state
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatusProblem {
    InvocationContextCouldNotBeVerified,
    InstalledStateCouldNotBeVerified,
    NativeRegistrationFault,
    NativeRegistrationCouldNotBeVerified,
    ProjectionFault,
    ProjectionCouldNotBeVerified,
    ServiceEnablementCouldNotBeVerified,
    ServiceConfiguredButStopped,
    ServiceActivityCouldNotBeVerified,
    SocketMissing,
    SocketUnsafe,
    AppServerUnavailable,
    AppServerCouldNotBeVerified,
    DesktopDescriptorFault,
    DesktopCouldNotBeVerified,
}

impl SetupSuccess {
    fn render(&self) -> RenderedCli {
        let mut blocks = vec![format!("Codex Session Control {} is ready.", self.version)];
        blocks.push(if self.running.cli {
            concat!(
                "Codex CLI is already running without Codex Session Control.\n",
                "Exit it, then start it with:\n",
                "  codex-session-control codex",
            )
            .to_owned()
        } else {
            concat!(
                "To use Codex Session Control with Codex CLI, start the CLI with:\n",
                "  codex-session-control codex",
            )
            .to_owned()
        });
        if let Some(desktop) = desktop_guidance(
            self.desktop,
            self.running.desktop,
            self.desktop_changed,
            false,
        ) {
            blocks.push(desktop);
        }
        success(blocks, &self.notices)
    }
}

impl UpdateSuccess {
    fn render(&self) -> RenderedCli {
        let mut blocks = vec![match self.state {
            UpdateState::Applied => {
                format!("Codex Session Control was updated to {}.", self.version)
            }
            UpdateState::AlreadyCurrent => {
                format!(
                    "Codex Session Control {} is already up to date.",
                    self.version
                )
            }
        }];
        if !self.service_enabled {
            blocks.push(
                "The service remains disabled. Run `codex-session-control enable` when you want to use it."
                    .to_owned(),
            );
        } else if self.state == UpdateState::Applied {
            blocks.push("Start a new task to use the updated plugin.".to_owned());
        }
        if self.desktop_changed {
            blocks.push(
                "If Codex Desktop is already running, restart it to use the updated version of Codex Session Control."
                    .to_owned(),
            );
        }
        success(blocks, &self.notices)
    }
}

impl EnableSuccess {
    fn render(&self) -> RenderedCli {
        let mut blocks =
            vec!["Codex Session Control is running and will start automatically.".to_owned()];
        if self.running.cli {
            blocks.push(
                concat!(
                    "Codex CLI is already running without Codex Session Control.\n",
                    "Exit it, then start it with:\n",
                    "  codex-session-control codex",
                )
                .to_owned(),
            );
        }
        if let Some(desktop) = desktop_guidance(
            self.desktop,
            self.running.desktop,
            self.desktop_changed,
            true,
        ) {
            blocks.push(desktop);
        }
        success(blocks, &self.notices)
    }
}

impl DisableSuccess {
    fn render(&self) -> RenderedCli {
        let mut data = "Your Codex data is unchanged.".to_owned();
        if self.desktop_removed {
            data.push_str(
                "\nIf Codex Desktop is already running, restart it to continue without Codex Session Control.",
            );
        }
        success(
            vec![
                "Codex Session Control is stopped and will not start automatically.".to_owned(),
                data,
            ],
            &[],
        )
    }
}

impl UninstallSuccess {
    fn render(&self) -> RenderedCli {
        let mut data = "Your Codex data is unchanged.".to_owned();
        if self.desktop_removed {
            data.push_str(
                "\nIf Codex Desktop is already running, restart it to continue without Codex Session Control.",
            );
        }
        success(
            vec!["Codex Session Control was uninstalled.".to_owned(), data],
            &[],
        )
    }
}

impl StatusResult {
    fn render(&self) -> RenderedCli {
        match self.state {
            StatusState::Healthy => status_rendered(
                format!(
                    "Status: healthy\nVersion: {}\nService: {}\nCodex CLI integration: {}\nCodex Desktop integration: {}\n",
                    self.version.as_ref().expect("healthy status has a version"),
                    service_summary(self.service.expect("healthy status has service evidence")),
                    integration_state(self.cli),
                    integration_state(self.desktop),
                ),
                0,
            ),
            StatusState::Disabled => status_rendered(
                format!(
                    "Status: disabled\nVersion: {}\nService: stopped, automatic startup is off\nCodex CLI integration: unavailable\nCodex Desktop integration: unavailable\n\nRun `codex-session-control enable` to start Codex Session Control.\n",
                    self.version
                        .as_ref()
                        .expect("disabled status has a version"),
                ),
                0,
            ),
            StatusState::NotInstalled => status_rendered(
                concat!(
                    "Status: not installed\n",
                    "Codex CLI integration: unavailable\n",
                    "Codex Desktop integration: unavailable\n",
                    "\n",
                    "Install Codex Session Control by running:\n",
                    "  codex-session-control setup\n",
                )
                .to_owned(),
                1,
            ),
            StatusState::Unhealthy => status_rendered(self.render_unhealthy(), 1),
        }
    }

    fn render_unhealthy(&self) -> String {
        let mut output = "Status: unhealthy\n".to_owned();
        if let Some(version) = &self.version {
            output.push_str(&format!("Version: {version}\n"));
        }
        if let Some(service) = self.service {
            output.push_str(&format!("Service: {}\n", service_summary(service)));
        }
        output.push_str(&format!(
            "Codex CLI integration: {}\nCodex Desktop integration: {}\n",
            integration_state(self.cli),
            integration_state(self.desktop),
        ));
        if !self.problems.is_empty() {
            output.push_str("\nProblems:\n");
            for problem in &self.problems {
                output.push_str("- ");
                output.push_str(status_problem(*problem));
                output.push('\n');
            }
            output.push('\n');
            if self.problems.iter().all(is_service_log_problem) {
                let qualifier = if self.problems.len() > 1 {
                    " for both problems"
                } else {
                    ""
                };
                output.push_str(&format!(
                    "Check the service logs{qualifier}:\n  journalctl --user -u codex-session-control.service\n"
                ));
            } else {
                output.push_str("Check what needs attention:\n  codex-session-control status\n");
            }
        }
        output
    }
}

impl RollbackIncomplete {
    fn render(&self) -> String {
        let ordinary = match self.primary {
            RollbackPrimary::SetupDesktopRetry => OrdinaryFailure::SetupDesktopIntegrationRetry,
            RollbackPrimary::SetupServiceConfigurationRetry => {
                OrdinaryFailure::SetupServiceConfigurationRetry
            }
            RollbackPrimary::SetupServiceStartRetry => OrdinaryFailure::SetupServiceStartRetry,
            RollbackPrimary::SetupServiceStateRetryUpdate => {
                OrdinaryFailure::SetupServiceStateRetryUpdate
            }
            RollbackPrimary::UpdateDesktopRetry => OrdinaryFailure::UpdateDesktopIntegrationRetry,
            RollbackPrimary::EnableDesktopRetry => OrdinaryFailure::EnableDesktopIntegrationRetry,
            RollbackPrimary::EnableServiceStateCheckStatus => {
                return rollback_block(
                    failure_block(
                        "Codex Session Control could not be started.",
                        "The service state could not be verified.",
                        "Check what needs attention:\n  codex-session-control status\n",
                    ),
                    &self.paths,
                );
            }
            RollbackPrimary::UninstallInstalledStateCheckStatus => {
                return rollback_block(
                    failure_block(
                        "Codex Session Control could not be uninstalled.",
                        "The installed Codex Session Control state could not be verified.",
                        "Check what needs attention:\n  codex-session-control status\n",
                    ),
                    &self.paths,
                );
            }
            RollbackPrimary::UninstallDesktopCheckStatus => {
                return rollback_block(
                    failure_block(
                        "Codex Session Control could not be uninstalled.",
                        "Codex Desktop integration could not be updated.",
                        "Check what needs attention:\n  codex-session-control status\n",
                    ),
                    &self.paths,
                );
            }
            RollbackPrimary::UninstallCleanupRetry => {
                return rollback_block(
                    failure_block(
                        "Codex Session Control could not be uninstalled.",
                        "Cleanup could not be completed safely.",
                        "Try again:\n  codex-session-control uninstall\n",
                    ),
                    &self.paths,
                );
            }
        };
        rollback_block(render_ordinary_failure(&ordinary), &self.paths)
    }
}

impl StopThenRetry {
    fn render(&self) -> String {
        let (headline, problem, commands) = match self {
            Self::UpdateServiceStateDisableUpdateEnable => (
                "Codex Session Control could not be updated.",
                "The service state could not be verified.",
                "  codex-session-control disable\n  codex-session-control update\n  codex-session-control enable\n",
            ),
            Self::EnableServiceStartStopThenEnable => (
                "Codex Session Control could not be started.",
                "The service could not be started.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control enable\n",
            ),
            Self::EnableServiceStateStopThenEnable => (
                "Codex Session Control could not be started.",
                "The service state could not be verified.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control enable\n",
            ),
            Self::DisableUnsafeStopThenDisable => (
                "Codex Session Control could not be stopped.",
                "The operation could not safely continue from this terminal.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control disable\n",
            ),
            Self::DisableServiceStopThenDisable => (
                "Codex Session Control could not be stopped.",
                "The service could not be stopped.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control disable\n",
            ),
            Self::DisableServiceStateStopThenDisable => (
                "Codex Session Control could not be stopped.",
                "The service state could not be verified.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control disable\n",
            ),
            Self::UninstallUnsafeStopThenUninstall => (
                "Codex Session Control could not be uninstalled.",
                "The operation could not safely continue from this terminal.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control uninstall\n",
            ),
            Self::UninstallServiceStateStopThenUninstall => (
                "Codex Session Control could not be uninstalled.",
                "The service state could not be verified.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control uninstall\n",
            ),
        };
        failure_block(
            headline,
            problem,
            &format!("From an independent terminal, stop the service and try again:\n{commands}"),
        )
    }
}

impl IndependentTerminal {
    fn render(&self) -> String {
        let (headline, command) = match self {
            Self::Update => (
                "Codex Session Control could not be updated.",
                "codex-session-control update",
            ),
            Self::Disable => (
                "Codex Session Control could not be stopped.",
                "codex-session-control disable",
            ),
            Self::Uninstall => (
                "Codex Session Control could not be uninstalled.",
                "codex-session-control uninstall",
            ),
        };
        failure_block(
            headline,
            "The operation could not safely continue from this terminal.",
            &format!("Run the command from an independent terminal:\n  {command}\n"),
        )
    }
}

impl ManualCleanup {
    fn render(&self) -> String {
        let executable = self.codex_executable.as_ref().map_or_else(
            || "codex".to_owned(),
            |path| {
                shell_quote_path(path).expect("validated Codex executable path is shell-quotable")
            },
        );
        let arguments = match self.command {
            NativeCleanupCommand::RemovePlugin => {
                "plugin remove codex-session-control@codex-session-control-local --json"
            }
            NativeCleanupCommand::RemoveMarketplace => {
                "plugin marketplace remove codex-session-control-local --json"
            }
        };
        failure_block(
            "Codex Session Control could not be uninstalled.",
            "Codex CLI integration could not be updated.",
            &format!(
                "Complete Codex CLI cleanup manually:\n  CODEX_HOME={} {executable} {arguments}\n",
                shell_quote_path(&self.codex_home)
                    .expect("validated Codex home path is shell-quotable")
            ),
        )
    }
}

impl VerifiedReleaseRecovery {
    fn render(&self) -> String {
        failure_block(
            "Codex Session Control could not be installed.",
            "The installed Codex Session Control state could not be verified.",
            &format!(
                "Recover the existing installation with the verified release:\n  Release: {}\n  Checksums: {}\n",
                self.release_url, self.checksums_url
            ),
        )
    }
}

impl PartialDisable {
    fn render(&self) -> String {
        let mut output = concat!(
            "Codex Session Control is stopped and will not start automatically.\n",
            "\n",
            "Codex Desktop integration could not be removed safely.\n",
            "Your Codex data is unchanged.\n",
            "\n",
            "Complete the remaining cleanup:\n",
            "  codex-session-control disable\n",
        )
        .to_owned();
        if let Some(path) = &self.managed_path {
            output.push_str(&format!(
                "\nManaged paths requiring attention:\n  {}\n",
                path.display()
            ));
        }
        output
    }
}

impl TerminalPartialUninstall {
    fn render(&self) -> String {
        let paths = render_paths(&self.remaining);
        format!(
            "Codex Session Control was only partially uninstalled.\n\nCleanup could not be completed safely.\n\nInspect these remaining managed paths:\n{paths}\nDo not rerun `codex-session-control uninstall`; the installed identity has already been removed.\n"
        )
    }
}

fn desktop_guidance(
    availability: DesktopAvailability,
    running: bool,
    changed: bool,
    enable: bool,
) -> Option<String> {
    match availability {
        DesktopAvailability::SetupRequired if enable => Some(concat!(
            "Codex Desktop integration is unavailable.\n",
            "Run `codex-session-control setup` to set it up.",
        ).to_owned()),
        DesktopAvailability::Unavailable
        | DesktopAvailability::CouldNotVerify
        | DesktopAvailability::SetupRequired => None,
        DesktopAvailability::Available if running => Some(concat!(
            "Codex Desktop is already running without Codex Session Control.\n",
            "Restart Codex Desktop to use Codex Session Control there.",
        ).to_owned()),
        DesktopAvailability::Available if changed => Some(
            "If Codex Desktop is already running, restart it to make Codex Session Control available there."
                .to_owned(),
        ),
        DesktopAvailability::Available => None,
    }
}

fn success(blocks: Vec<String>, notices: &[UserNotice]) -> RenderedCli {
    RenderedCli {
        stdout: compose_blocks(blocks),
        stderr: compose_blocks(notices.iter().map(render_notice).collect()),
        exit_code: 0,
    }
}

fn status_rendered(stdout: String, exit_code: u8) -> RenderedCli {
    RenderedCli {
        stdout,
        stderr: String::new(),
        exit_code,
    }
}

fn failure(stderr: String) -> RenderedCli {
    RenderedCli {
        stdout: String::new(),
        stderr,
        exit_code: 1,
    }
}

fn compose_blocks(blocks: Vec<String>) -> String {
    if blocks.is_empty() {
        String::new()
    } else {
        format!("{}\n", blocks.join("\n\n"))
    }
}

fn render_notice(notice: &UserNotice) -> String {
    match notice {
        UserNotice::Compatibility { codex, product } => format!(
            "Warning: Codex {codex} has not been tested with Codex Session Control {product}.\nSome features may not work as expected."
        ),
        UserNotice::DesktopLauncherUnavailable =>
            "Codex Desktop integration is unavailable because a compatible Desktop launcher was not found."
                .to_owned(),
        UserNotice::LocalBinMissingFromPath { local_bin } => format!(
            "Note: `{}` is not on your PATH.\nAdd it to your PATH to use the short `codex-session-control` command.",
            local_bin.display()
        ),
    }
}

fn render_ordinary_failure(failure: &OrdinaryFailure) -> String {
    const SETUP: &str = "Codex Session Control could not be installed.";
    const UPDATE: &str = "Codex Session Control could not be updated.";
    const ENABLE: &str = "Codex Session Control could not be started.";
    const DISABLE: &str = "Codex Session Control could not be stopped.";
    const UNINSTALL: &str = "Codex Session Control could not be uninstalled.";
    const SETUP_RETRY: &str = "Try again:\n  codex-session-control setup\n";
    const UPDATE_RETRY: &str = "Try again:\n  codex-session-control update\n";
    const ENABLE_RETRY: &str = "Try again:\n  codex-session-control enable\n";
    const DISABLE_RETRY: &str = "Try again:\n  codex-session-control disable\n";
    const UNINSTALL_RETRY: &str = "Try again:\n  codex-session-control uninstall\n";
    const STATUS: &str = "Check what needs attention:\n  codex-session-control status\n";
    const LOGS: &str =
        "Check the service logs:\n  journalctl --user -u codex-session-control.service\n";
    if let OrdinaryFailure::SetupInstalledStateRepair { binary } = failure {
        return failure_block(
            SETUP,
            "The installed Codex Session Control state could not be verified.",
            &format!(
                "Repair Codex Session Control:\n  {} setup\n",
                binary.display()
            ),
        );
    }
    let (headline, problem, recovery) = match failure {
        OrdinaryFailure::SetupUnsafeTerminalRetry => (
            SETUP,
            "The operation could not safely continue from this terminal.",
            SETUP_RETRY,
        ),
        OrdinaryFailure::SetupUnexpectedRetry => {
            (SETUP, "The operation failed unexpectedly.", SETUP_RETRY)
        }
        OrdinaryFailure::SetupInstalledStateCheckStatus => (
            SETUP,
            "The installed Codex Session Control state could not be verified.",
            STATUS,
        ),
        OrdinaryFailure::SetupInstallationFilesRetryUpdate => (
            SETUP,
            "The installation files could not be updated.",
            UPDATE_RETRY,
        ),
        OrdinaryFailure::SetupCliIntegrationRetry => (
            SETUP,
            "Codex CLI integration could not be updated.",
            SETUP_RETRY,
        ),
        OrdinaryFailure::SetupCliIntegrationCheckStatus => {
            (SETUP, "Codex CLI integration could not be updated.", STATUS)
        }
        OrdinaryFailure::SetupInstallationFilesRetry => (
            SETUP,
            "The installation files could not be updated.",
            SETUP_RETRY,
        ),
        OrdinaryFailure::SetupServiceConfigurationRetry => {
            (SETUP, "The service could not be configured.", SETUP_RETRY)
        }
        OrdinaryFailure::SetupDesktopIntegrationRetry => (
            SETUP,
            "Codex Desktop integration could not be updated.",
            SETUP_RETRY,
        ),
        OrdinaryFailure::SetupDesktopIntegrationCheckStatus => (
            SETUP,
            "Codex Desktop integration could not be updated.",
            STATUS,
        ),
        OrdinaryFailure::SetupServiceStartRetry => {
            (SETUP, "The service could not be started.", SETUP_RETRY)
        }
        OrdinaryFailure::SetupServiceStateRetryUpdate => (
            SETUP,
            "The service state could not be verified.",
            UPDATE_RETRY,
        ),
        OrdinaryFailure::UpdateUnexpectedRetry => {
            (UPDATE, "The operation failed unexpectedly.", UPDATE_RETRY)
        }
        OrdinaryFailure::UpdateInstalledStateCheckStatus
        | OrdinaryFailure::UpdateInstalledStatePostMutationCheckStatus => (
            UPDATE,
            "The installed Codex Session Control state could not be verified.",
            STATUS,
        ),
        OrdinaryFailure::UpdateReleaseRetry => (
            UPDATE,
            "The latest release could not be retrieved.",
            UPDATE_RETRY,
        ),
        OrdinaryFailure::UpdateChecksumRetry => (
            UPDATE,
            "The downloaded release could not be verified.",
            UPDATE_RETRY,
        ),
        OrdinaryFailure::UpdateCliIntegrationRetry => (
            UPDATE,
            "Codex CLI integration could not be updated.",
            UPDATE_RETRY,
        ),
        OrdinaryFailure::UpdateCliIntegrationCheckStatus => (
            UPDATE,
            "Codex CLI integration could not be updated.",
            STATUS,
        ),
        OrdinaryFailure::UpdateServiceConfigurationRetry => {
            (UPDATE, "The service could not be configured.", UPDATE_RETRY)
        }
        OrdinaryFailure::UpdateServiceStateCheckStatus => {
            (UPDATE, "The service state could not be verified.", STATUS)
        }
        OrdinaryFailure::UpdateActiveTasksRetry => (
            UPDATE,
            "Active tasks could not be checked safely.",
            UPDATE_RETRY,
        ),
        OrdinaryFailure::UpdateInstallationFilesRetry => (
            UPDATE,
            "The installation files could not be updated.",
            UPDATE_RETRY,
        ),
        OrdinaryFailure::UpdateDesktopIntegrationRetry => (
            UPDATE,
            "Codex Desktop integration could not be updated.",
            UPDATE_RETRY,
        ),
        OrdinaryFailure::UpdateDesktopIntegrationCheckStatus => (
            UPDATE,
            "Codex Desktop integration could not be updated.",
            STATUS,
        ),
        OrdinaryFailure::UpdateServiceConfigurationLogs => {
            (UPDATE, "The service could not be configured.", LOGS)
        }
        OrdinaryFailure::UpdateServiceStartLogs => {
            (UPDATE, "The service could not be started.", LOGS)
        }
        OrdinaryFailure::UpdateServiceStateLogs => {
            (UPDATE, "The service state could not be verified.", LOGS)
        }
        OrdinaryFailure::EnableUnexpectedRetry => {
            (ENABLE, "The operation failed unexpectedly.", ENABLE_RETRY)
        }
        OrdinaryFailure::EnableInstalledStateRepairSetup => (
            ENABLE,
            "The installed Codex Session Control state could not be verified.",
            "Repair Codex Session Control:\n  codex-session-control setup\n",
        ),
        OrdinaryFailure::EnableServiceConfigurationRepairSetup => (
            ENABLE,
            "The service could not be configured.",
            "Repair Codex Session Control:\n  codex-session-control setup\n",
        ),
        OrdinaryFailure::EnableDesktopIntegrationCheckStatus => (
            ENABLE,
            "Codex Desktop integration could not be updated.",
            STATUS,
        ),
        OrdinaryFailure::EnableDesktopIntegrationRetry => (
            ENABLE,
            "Codex Desktop integration could not be updated.",
            ENABLE_RETRY,
        ),
        OrdinaryFailure::EnableServiceStartRetry => {
            (ENABLE, "The service could not be started.", ENABLE_RETRY)
        }
        OrdinaryFailure::EnableServiceStateRetry => (
            ENABLE,
            "The service state could not be verified.",
            ENABLE_RETRY,
        ),
        #[cfg(test)]
        OrdinaryFailure::EnableUnexpectedCheckStatus => {
            (ENABLE, "The operation failed unexpectedly.", STATUS)
        }
        OrdinaryFailure::DisableUnexpectedRetry => {
            (DISABLE, "The operation failed unexpectedly.", DISABLE_RETRY)
        }
        OrdinaryFailure::DisableServiceStopRetry => {
            (DISABLE, "The service could not be stopped.", DISABLE_RETRY)
        }
        #[cfg(test)]
        OrdinaryFailure::DisableUnexpectedCheckStatus => {
            (DISABLE, "The operation failed unexpectedly.", STATUS)
        }
        OrdinaryFailure::UninstallUnexpectedRetry => (
            UNINSTALL,
            "The operation failed unexpectedly.",
            UNINSTALL_RETRY,
        ),
        OrdinaryFailure::UninstallServiceStopRetry => (
            UNINSTALL,
            "The service could not be stopped.",
            UNINSTALL_RETRY,
        ),
        OrdinaryFailure::SetupInstalledStateRepair { .. } => unreachable!(),
    };
    failure_block(headline, problem, recovery)
}

fn failure_block(headline: &str, problem: &str, recovery: &str) -> String {
    format!("{headline}\n\n{problem}\n\n{recovery}")
}

fn rollback_block(mut primary: String, paths: &ManagedPaths) -> String {
    primary.push_str("\nCleanup could not be completed safely.\n\nInspect these managed paths:\n");
    primary.push_str(&render_paths(paths));
    primary
}

fn render_paths(paths: &ManagedPaths) -> String {
    paths
        .iter()
        .map(|path| format!("  {}\n", path.display()))
        .collect()
}

fn service_summary(summary: ServiceSummary) -> &'static str {
    match summary {
        ServiceSummary::RunningAutomatic => "running, starts automatically",
        ServiceSummary::StoppedAutomaticOff => "stopped, automatic startup is off",
        ServiceSummary::StoppedUnexpectedAutomaticOn => {
            "stopped unexpectedly, automatic startup is on"
        }
        ServiceSummary::CouldNotVerify => "could not verify",
    }
}

fn integration_state(state: IntegrationState) -> &'static str {
    match state {
        IntegrationState::Ready => "ready",
        IntegrationState::Unavailable => "unavailable",
        IntegrationState::Unhealthy => "unhealthy",
        IntegrationState::CouldNotVerify => "could not verify",
    }
}

fn status_problem(problem: StatusProblem) -> &'static str {
    match problem {
        StatusProblem::InvocationContextCouldNotBeVerified => {
            "The invocation context could not be verified."
        }
        StatusProblem::InstalledStateCouldNotBeVerified => {
            "The installed Codex Session Control state could not be verified."
        }
        StatusProblem::NativeRegistrationFault => "Codex CLI native registration is incorrect.",
        StatusProblem::NativeRegistrationCouldNotBeVerified => {
            "Codex CLI native registration could not be verified."
        }
        StatusProblem::ProjectionFault => "Codex CLI integration files are incorrect.",
        StatusProblem::ProjectionCouldNotBeVerified => {
            "Codex CLI integration files could not be verified."
        }
        StatusProblem::ServiceEnablementCouldNotBeVerified => {
            "Automatic service startup could not be verified."
        }
        StatusProblem::ServiceConfiguredButStopped => {
            "The service is configured to run but is stopped."
        }
        StatusProblem::ServiceActivityCouldNotBeVerified => {
            "The service state could not be verified."
        }
        StatusProblem::SocketMissing => "The service connection is unavailable.",
        StatusProblem::SocketUnsafe => "The service connection is unsafe.",
        StatusProblem::AppServerUnavailable => "The app-server is unavailable.",
        StatusProblem::AppServerCouldNotBeVerified => "The app-server could not be verified.",
        StatusProblem::DesktopDescriptorFault => {
            "Codex Desktop integration is incorrectly configured."
        }
        StatusProblem::DesktopCouldNotBeVerified => {
            "Codex Desktop integration could not be verified."
        }
    }
}

fn is_service_log_problem(problem: &StatusProblem) -> bool {
    matches!(
        problem,
        StatusProblem::ServiceConfiguredButStopped
            | StatusProblem::SocketMissing
            | StatusProblem::AppServerUnavailable
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use semver::Version;

    use super::*;

    const UPDATE_COMPLETION_UNKNOWN: &str = concat!(
        "Codex Session Control could not confirm that the update completed.\n",
        "\n",
        "The installed Codex Session Control state could not be verified.\n",
        "\n",
        "Check what needs attention:\n",
        "  codex-session-control status\n",
    );

    fn version() -> Version {
        Version::parse("1.2.3").unwrap()
    }

    fn failure_oracle(headline: &str, problem: &str, recovery: &str) -> String {
        format!("{headline}\n\n{problem}\n\n{recovery}")
    }

    fn ordinary_literal_cases() -> Vec<(OrdinaryFailure, &'static str)> {
        vec![
            (
                OrdinaryFailure::SetupUnsafeTerminalRetry,
                "Codex Session Control could not be installed.\n\nThe operation could not safely continue from this terminal.\n\nTry again:\n  codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::SetupUnexpectedRetry,
                "Codex Session Control could not be installed.\n\nThe operation failed unexpectedly.\n\nTry again:\n  codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::SetupInstalledStateCheckStatus,
                "Codex Session Control could not be installed.\n\nThe installed Codex Session Control state could not be verified.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::SetupInstallationFilesRetryUpdate,
                "Codex Session Control could not be installed.\n\nThe installation files could not be updated.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::SetupInstalledStateRepair {
                    binary: PathBuf::from("/opt/codex-session-control"),
                },
                "Codex Session Control could not be installed.\n\nThe installed Codex Session Control state could not be verified.\n\nRepair Codex Session Control:\n  /opt/codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::SetupCliIntegrationRetry,
                "Codex Session Control could not be installed.\n\nCodex CLI integration could not be updated.\n\nTry again:\n  codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::SetupCliIntegrationCheckStatus,
                "Codex Session Control could not be installed.\n\nCodex CLI integration could not be updated.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::SetupInstallationFilesRetry,
                "Codex Session Control could not be installed.\n\nThe installation files could not be updated.\n\nTry again:\n  codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::SetupServiceConfigurationRetry,
                "Codex Session Control could not be installed.\n\nThe service could not be configured.\n\nTry again:\n  codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::SetupDesktopIntegrationRetry,
                "Codex Session Control could not be installed.\n\nCodex Desktop integration could not be updated.\n\nTry again:\n  codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::SetupDesktopIntegrationCheckStatus,
                "Codex Session Control could not be installed.\n\nCodex Desktop integration could not be updated.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::SetupServiceStartRetry,
                "Codex Session Control could not be installed.\n\nThe service could not be started.\n\nTry again:\n  codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::SetupServiceStateRetryUpdate,
                "Codex Session Control could not be installed.\n\nThe service state could not be verified.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::UpdateUnexpectedRetry,
                "Codex Session Control could not be updated.\n\nThe operation failed unexpectedly.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::UpdateInstalledStateCheckStatus,
                "Codex Session Control could not be updated.\n\nThe installed Codex Session Control state could not be verified.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::UpdateReleaseRetry,
                "Codex Session Control could not be updated.\n\nThe latest release could not be retrieved.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::UpdateChecksumRetry,
                "Codex Session Control could not be updated.\n\nThe downloaded release could not be verified.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::UpdateCliIntegrationRetry,
                "Codex Session Control could not be updated.\n\nCodex CLI integration could not be updated.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::UpdateCliIntegrationCheckStatus,
                "Codex Session Control could not be updated.\n\nCodex CLI integration could not be updated.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::UpdateServiceConfigurationRetry,
                "Codex Session Control could not be updated.\n\nThe service could not be configured.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::UpdateServiceStateCheckStatus,
                "Codex Session Control could not be updated.\n\nThe service state could not be verified.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::UpdateActiveTasksRetry,
                "Codex Session Control could not be updated.\n\nActive tasks could not be checked safely.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::UpdateInstallationFilesRetry,
                "Codex Session Control could not be updated.\n\nThe installation files could not be updated.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::UpdateDesktopIntegrationRetry,
                "Codex Session Control could not be updated.\n\nCodex Desktop integration could not be updated.\n\nTry again:\n  codex-session-control update\n",
            ),
            (
                OrdinaryFailure::UpdateDesktopIntegrationCheckStatus,
                "Codex Session Control could not be updated.\n\nCodex Desktop integration could not be updated.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::UpdateServiceConfigurationLogs,
                "Codex Session Control could not be updated.\n\nThe service could not be configured.\n\nCheck the service logs:\n  journalctl --user -u codex-session-control.service\n",
            ),
            (
                OrdinaryFailure::UpdateServiceStartLogs,
                "Codex Session Control could not be updated.\n\nThe service could not be started.\n\nCheck the service logs:\n  journalctl --user -u codex-session-control.service\n",
            ),
            (
                OrdinaryFailure::UpdateServiceStateLogs,
                "Codex Session Control could not be updated.\n\nThe service state could not be verified.\n\nCheck the service logs:\n  journalctl --user -u codex-session-control.service\n",
            ),
            (
                OrdinaryFailure::UpdateInstalledStatePostMutationCheckStatus,
                "Codex Session Control could not be updated.\n\nThe installed Codex Session Control state could not be verified.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::EnableUnexpectedRetry,
                "Codex Session Control could not be started.\n\nThe operation failed unexpectedly.\n\nTry again:\n  codex-session-control enable\n",
            ),
            (
                OrdinaryFailure::EnableInstalledStateRepairSetup,
                "Codex Session Control could not be started.\n\nThe installed Codex Session Control state could not be verified.\n\nRepair Codex Session Control:\n  codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::EnableServiceConfigurationRepairSetup,
                "Codex Session Control could not be started.\n\nThe service could not be configured.\n\nRepair Codex Session Control:\n  codex-session-control setup\n",
            ),
            (
                OrdinaryFailure::EnableDesktopIntegrationCheckStatus,
                "Codex Session Control could not be started.\n\nCodex Desktop integration could not be updated.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::EnableDesktopIntegrationRetry,
                "Codex Session Control could not be started.\n\nCodex Desktop integration could not be updated.\n\nTry again:\n  codex-session-control enable\n",
            ),
            (
                OrdinaryFailure::EnableServiceStartRetry,
                "Codex Session Control could not be started.\n\nThe service could not be started.\n\nTry again:\n  codex-session-control enable\n",
            ),
            (
                OrdinaryFailure::EnableServiceStateRetry,
                "Codex Session Control could not be started.\n\nThe service state could not be verified.\n\nTry again:\n  codex-session-control enable\n",
            ),
            (
                OrdinaryFailure::EnableUnexpectedCheckStatus,
                "Codex Session Control could not be started.\n\nThe operation failed unexpectedly.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::DisableUnexpectedRetry,
                "Codex Session Control could not be stopped.\n\nThe operation failed unexpectedly.\n\nTry again:\n  codex-session-control disable\n",
            ),
            (
                OrdinaryFailure::DisableServiceStopRetry,
                "Codex Session Control could not be stopped.\n\nThe service could not be stopped.\n\nTry again:\n  codex-session-control disable\n",
            ),
            (
                OrdinaryFailure::DisableUnexpectedCheckStatus,
                "Codex Session Control could not be stopped.\n\nThe operation failed unexpectedly.\n\nCheck what needs attention:\n  codex-session-control status\n",
            ),
            (
                OrdinaryFailure::UninstallUnexpectedRetry,
                "Codex Session Control could not be uninstalled.\n\nThe operation failed unexpectedly.\n\nTry again:\n  codex-session-control uninstall\n",
            ),
            (
                OrdinaryFailure::UninstallServiceStopRetry,
                "Codex Session Control could not be uninstalled.\n\nThe service could not be stopped.\n\nTry again:\n  codex-session-control uninstall\n",
            ),
        ]
    }

    fn ordinary_expected(failure: &OrdinaryFailure) -> &'static str {
        ordinary_literal_cases()
            .into_iter()
            .find_map(|(candidate, expected)| (candidate == *failure).then_some(expected))
            .expect("ordinary failure has one literal expected block")
    }

    fn rendered_failure(stderr: impl Into<String>) -> RenderedCli {
        RenderedCli {
            stdout: String::new(),
            stderr: stderr.into(),
            exit_code: 1,
        }
    }

    fn rollback_oracle(primary: String, paths: &[&str]) -> String {
        let mut expected = primary;
        expected
            .push_str("\nCleanup could not be completed safely.\n\nInspect these managed paths:\n");
        for path in paths {
            expected.push_str(&format!("  {path}\n"));
        }
        expected
    }

    fn rollback_primary_oracle(primary: RollbackPrimary) -> String {
        match primary {
            RollbackPrimary::SetupDesktopRetry => {
                ordinary_expected(&OrdinaryFailure::SetupDesktopIntegrationRetry).to_owned()
            }
            RollbackPrimary::SetupServiceConfigurationRetry => {
                ordinary_expected(&OrdinaryFailure::SetupServiceConfigurationRetry).to_owned()
            }
            RollbackPrimary::SetupServiceStartRetry => {
                ordinary_expected(&OrdinaryFailure::SetupServiceStartRetry).to_owned()
            }
            RollbackPrimary::SetupServiceStateRetryUpdate => {
                ordinary_expected(&OrdinaryFailure::SetupServiceStateRetryUpdate).to_owned()
            }
            RollbackPrimary::UpdateDesktopRetry => {
                ordinary_expected(&OrdinaryFailure::UpdateDesktopIntegrationRetry).to_owned()
            }
            RollbackPrimary::EnableDesktopRetry => {
                ordinary_expected(&OrdinaryFailure::EnableDesktopIntegrationRetry).to_owned()
            }
            RollbackPrimary::EnableServiceStateCheckStatus => failure_oracle(
                "Codex Session Control could not be started.",
                "The service state could not be verified.",
                "Check what needs attention:\n  codex-session-control status\n",
            ),
            RollbackPrimary::UninstallInstalledStateCheckStatus => failure_oracle(
                "Codex Session Control could not be uninstalled.",
                "The installed Codex Session Control state could not be verified.",
                "Check what needs attention:\n  codex-session-control status\n",
            ),
            RollbackPrimary::UninstallDesktopCheckStatus => failure_oracle(
                "Codex Session Control could not be uninstalled.",
                "Codex Desktop integration could not be updated.",
                "Check what needs attention:\n  codex-session-control status\n",
            ),
            RollbackPrimary::UninstallCleanupRetry => failure_oracle(
                "Codex Session Control could not be uninstalled.",
                "Cleanup could not be completed safely.",
                "Try again:\n  codex-session-control uninstall\n",
            ),
        }
    }

    fn stop_then_retry_oracle(failure: StopThenRetry) -> String {
        let (headline, problem, commands) = match failure {
            StopThenRetry::UpdateServiceStateDisableUpdateEnable => (
                "Codex Session Control could not be updated.",
                "The service state could not be verified.",
                "  codex-session-control disable\n  codex-session-control update\n  codex-session-control enable\n",
            ),
            StopThenRetry::EnableServiceStartStopThenEnable => (
                "Codex Session Control could not be started.",
                "The service could not be started.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control enable\n",
            ),
            StopThenRetry::EnableServiceStateStopThenEnable => (
                "Codex Session Control could not be started.",
                "The service state could not be verified.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control enable\n",
            ),
            StopThenRetry::DisableUnsafeStopThenDisable => (
                "Codex Session Control could not be stopped.",
                "The operation could not safely continue from this terminal.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control disable\n",
            ),
            StopThenRetry::DisableServiceStopThenDisable => (
                "Codex Session Control could not be stopped.",
                "The service could not be stopped.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control disable\n",
            ),
            StopThenRetry::DisableServiceStateStopThenDisable => (
                "Codex Session Control could not be stopped.",
                "The service state could not be verified.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control disable\n",
            ),
            StopThenRetry::UninstallUnsafeStopThenUninstall => (
                "Codex Session Control could not be uninstalled.",
                "The operation could not safely continue from this terminal.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control uninstall\n",
            ),
            StopThenRetry::UninstallServiceStateStopThenUninstall => (
                "Codex Session Control could not be uninstalled.",
                "The service state could not be verified.",
                "  systemctl --user stop codex-session-control.service\n  codex-session-control uninstall\n",
            ),
        };
        failure_oracle(
            headline,
            problem,
            &format!("From an independent terminal, stop the service and try again:\n{commands}"),
        )
    }

    fn failure_render_cases() -> Vec<(UserFailure, RenderedCli)> {
        let rollback_primaries = [
            RollbackPrimary::SetupDesktopRetry,
            RollbackPrimary::SetupServiceConfigurationRetry,
            RollbackPrimary::SetupServiceStartRetry,
            RollbackPrimary::SetupServiceStateRetryUpdate,
            RollbackPrimary::UpdateDesktopRetry,
            RollbackPrimary::EnableDesktopRetry,
            RollbackPrimary::EnableServiceStateCheckStatus,
            RollbackPrimary::UninstallInstalledStateCheckStatus,
            RollbackPrimary::UninstallDesktopCheckStatus,
            RollbackPrimary::UninstallCleanupRetry,
        ];
        let mut cases = rollback_primaries
            .into_iter()
            .map(|primary| {
                (
                    UserFailure::RollbackIncomplete(RollbackIncomplete::new(
                        primary,
                        ManagedPaths::new(PathBuf::from("/managed/one"), Vec::new()),
                    )),
                    rendered_failure(rollback_oracle(
                        rollback_primary_oracle(primary),
                        &["/managed/one"],
                    )),
                )
            })
            .collect::<Vec<_>>();
        cases.push((
            UserFailure::RollbackIncomplete(RollbackIncomplete::new(
                RollbackPrimary::UpdateDesktopRetry,
                ManagedPaths::new(
                    PathBuf::from("/managed/one"),
                    vec![PathBuf::from("/managed/two")],
                ),
            )),
            rendered_failure(rollback_oracle(
                ordinary_expected(&OrdinaryFailure::UpdateDesktopIntegrationRetry).to_owned(),
                &["/managed/one", "/managed/two"],
            )),
        ));

        for failure in [
            StopThenRetry::UpdateServiceStateDisableUpdateEnable,
            StopThenRetry::EnableServiceStartStopThenEnable,
            StopThenRetry::EnableServiceStateStopThenEnable,
            StopThenRetry::DisableUnsafeStopThenDisable,
            StopThenRetry::DisableServiceStopThenDisable,
            StopThenRetry::DisableServiceStateStopThenDisable,
            StopThenRetry::UninstallUnsafeStopThenUninstall,
            StopThenRetry::UninstallServiceStateStopThenUninstall,
        ] {
            cases.push((
                UserFailure::StopThenRetry(failure),
                rendered_failure(stop_then_retry_oracle(failure)),
            ));
        }

        for (failure, headline, command) in [
            (
                IndependentTerminal::Update,
                "Codex Session Control could not be updated.",
                "codex-session-control update",
            ),
            (
                IndependentTerminal::Disable,
                "Codex Session Control could not be stopped.",
                "codex-session-control disable",
            ),
            (
                IndependentTerminal::Uninstall,
                "Codex Session Control could not be uninstalled.",
                "codex-session-control uninstall",
            ),
        ] {
            cases.push((
                UserFailure::IndependentTerminal(failure),
                rendered_failure(failure_oracle(
                    headline,
                    "The operation could not safely continue from this terminal.",
                    &format!("Run the command from an independent terminal:\n  {command}\n"),
                )),
            ));
        }

        let cleanup_rows = [
            (
                NativeCleanupCommand::RemovePlugin,
                None,
                "CODEX_HOME='/home/test/.codex' codex plugin remove codex-session-control@codex-session-control-local --json",
            ),
            (
                NativeCleanupCommand::RemovePlugin,
                Some(PathBuf::from("/opt/Codex CLI/codex")),
                "CODEX_HOME='/home/test/.codex' '/opt/Codex CLI/codex' plugin remove codex-session-control@codex-session-control-local --json",
            ),
            (
                NativeCleanupCommand::RemoveMarketplace,
                None,
                "CODEX_HOME='/home/test/.codex' codex plugin marketplace remove codex-session-control-local --json",
            ),
            (
                NativeCleanupCommand::RemoveMarketplace,
                Some(PathBuf::from("/opt/Codex CLI/codex")),
                "CODEX_HOME='/home/test/.codex' '/opt/Codex CLI/codex' plugin marketplace remove codex-session-control-local --json",
            ),
        ];
        for (command, executable, rendered_command) in cleanup_rows {
            cases.push((
                UserFailure::ManualCleanup(ManualCleanup::new(
                    command,
                    PathBuf::from("/home/test/.codex"),
                    executable,
                )),
                rendered_failure(failure_oracle(
                    "Codex Session Control could not be uninstalled.",
                    "Codex CLI integration could not be updated.",
                    &format!("Complete Codex CLI cleanup manually:\n  {rendered_command}\n"),
                )),
            ));
        }

        cases.extend([
            (
                UserFailure::VerifiedRelease(VerifiedReleaseRecovery::new(
                    "https://github.com/example/releases/tag/v1.2.3".to_owned(),
                    "https://github.com/example/releases/download/v1.2.3/checksums.txt".to_owned(),
                )),
                rendered_failure(concat!(
                    "Codex Session Control could not be installed.\n\n",
                    "The installed Codex Session Control state could not be verified.\n\n",
                    "Recover the existing installation with the verified release:\n",
                    "  Release: https://github.com/example/releases/tag/v1.2.3\n",
                    "  Checksums: https://github.com/example/releases/download/v1.2.3/checksums.txt\n",
                )),
            ),
            (
                UserFailure::InteractiveTerminal,
                rendered_failure(concat!(
                    "Codex Session Control could not be updated.\n\n",
                    "The operation could not safely continue from this terminal.\n\n",
                    "Run the update from an interactive terminal:\n",
                    "  codex-session-control update\n",
                )),
            ),
            (
                UserFailure::PartialDisable(PartialDisable::new(None)),
                rendered_failure(concat!(
                    "Codex Session Control is stopped and will not start automatically.\n\n",
                    "Codex Desktop integration could not be removed safely.\n",
                    "Your Codex data is unchanged.\n\n",
                    "Complete the remaining cleanup:\n",
                    "  codex-session-control disable\n",
                )),
            ),
            (
                UserFailure::PartialDisable(PartialDisable::new(Some(PathBuf::from(
                    "/managed/desktop.desktop",
                )))),
                rendered_failure(concat!(
                    "Codex Session Control is stopped and will not start automatically.\n\n",
                    "Codex Desktop integration could not be removed safely.\n",
                    "Your Codex data is unchanged.\n\n",
                    "Complete the remaining cleanup:\n",
                    "  codex-session-control disable\n\n",
                    "Managed paths requiring attention:\n",
                    "  /managed/desktop.desktop\n",
                )),
            ),
            (
                UserFailure::TerminalPartialUninstall(TerminalPartialUninstall::new(
                    ManagedPaths::new(PathBuf::from("/managed/one"), Vec::new()),
                )),
                rendered_failure(concat!(
                    "Codex Session Control was only partially uninstalled.\n\n",
                    "Cleanup could not be completed safely.\n\n",
                    "Inspect these remaining managed paths:\n",
                    "  /managed/one\n\n",
                    "Do not rerun `codex-session-control uninstall`; the installed identity has already been removed.\n",
                )),
            ),
            (
                UserFailure::TerminalPartialUninstall(TerminalPartialUninstall::new(
                    ManagedPaths::new(
                        PathBuf::from("/managed/one"),
                        vec![PathBuf::from("/managed/two")],
                    ),
                )),
                rendered_failure(concat!(
                    "Codex Session Control was only partially uninstalled.\n\n",
                    "Cleanup could not be completed safely.\n\n",
                    "Inspect these remaining managed paths:\n",
                    "  /managed/one\n",
                    "  /managed/two\n\n",
                    "Do not rerun `codex-session-control uninstall`; the installed identity has already been removed.\n",
                )),
            ),
            (
                UserFailure::Cancellation,
                rendered_failure(concat!(
                    "Codex Session Control was not updated.\n\n",
                    "The update was canceled before installation files changed.\n",
                )),
            ),
            (
                UserFailure::WrapperUnavailable,
                rendered_failure(concat!(
                    "Codex CLI could not start because Codex Session Control is unavailable.\n\n",
                    "Check what needs attention:\n",
                    "  codex-session-control status\n",
                )),
            ),
        ]);
        cases
    }

    fn rendered_success(stdout: impl Into<String>, stderr: impl Into<String>) -> RenderedCli {
        RenderedCli {
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code: 0,
        }
    }

    fn setup_success(
        running: RunningClientFacts,
        desktop: DesktopAvailability,
        desktop_changed: bool,
        notices: Vec<UserNotice>,
    ) -> UserSuccess {
        UserSuccess::Setup(
            SetupSuccess::new(version(), running, desktop, desktop_changed, notices).unwrap(),
        )
    }

    fn enable_success(
        running: RunningClientFacts,
        desktop: DesktopAvailability,
        desktop_changed: bool,
        notices: Vec<UserNotice>,
    ) -> UserSuccess {
        UserSuccess::Enable(EnableSuccess::new(running, desktop, desktop_changed, notices).unwrap())
    }

    fn status_success(
        state: StatusState,
        version: Option<Version>,
        service: Option<ServiceSummary>,
        cli: IntegrationState,
        desktop: IntegrationState,
        problems: Vec<StatusProblem>,
    ) -> UserSuccess {
        UserSuccess::Status(StatusResult::new(
            state, version, service, cli, desktop, problems,
        ))
    }

    fn service_summary_oracle(service: ServiceSummary) -> &'static str {
        match service {
            ServiceSummary::RunningAutomatic => "running, starts automatically",
            ServiceSummary::StoppedAutomaticOff => "stopped, automatic startup is off",
            ServiceSummary::StoppedUnexpectedAutomaticOn => {
                "stopped unexpectedly, automatic startup is on"
            }
            ServiceSummary::CouldNotVerify => "could not verify",
        }
    }

    fn integration_state_oracle(state: IntegrationState) -> &'static str {
        match state {
            IntegrationState::Ready => "ready",
            IntegrationState::Unavailable => "unavailable",
            IntegrationState::Unhealthy => "unhealthy",
            IntegrationState::CouldNotVerify => "could not verify",
        }
    }

    fn unhealthy_status_oracle(
        service: ServiceSummary,
        cli: IntegrationState,
        desktop: IntegrationState,
        problems: &[(&str, bool)],
    ) -> String {
        let mut expected = format!(
            "Status: unhealthy\nVersion: 1.2.3\nService: {}\nCodex CLI integration: {}\nCodex Desktop integration: {}\n\nProblems:\n",
            service_summary_oracle(service),
            integration_state_oracle(cli),
            integration_state_oracle(desktop),
        );
        for (problem, _) in problems {
            expected.push_str(&format!("- {problem}\n"));
        }
        expected.push('\n');
        if problems.iter().all(|(_, logs)| *logs) {
            let qualifier = if problems.len() > 1 {
                " for both problems"
            } else {
                ""
            };
            expected.push_str(&format!(
                "Check the service logs{qualifier}:\n  journalctl --user -u codex-session-control.service\n"
            ));
        } else {
            expected.push_str("Check what needs attention:\n  codex-session-control status\n");
        }
        expected
    }

    fn success_render_cases() -> Vec<(UserSuccess, RenderedCli)> {
        const SETUP_PRIMARY: &str = "Codex Session Control 1.2.3 is ready.";
        const CLI_GENERIC: &str = concat!(
            "To use Codex Session Control with Codex CLI, start the CLI with:\n",
            "  codex-session-control codex",
        );
        const CLI_RUNNING: &str = concat!(
            "Codex CLI is already running without Codex Session Control.\n",
            "Exit it, then start it with:\n",
            "  codex-session-control codex",
        );
        const DESKTOP_RUNNING: &str = concat!(
            "Codex Desktop is already running without Codex Session Control.\n",
            "Restart Codex Desktop to use Codex Session Control there.",
        );
        const DESKTOP_RESTART: &str = "If Codex Desktop is already running, restart it to make Codex Session Control available there.";
        const DESKTOP_WARNING: &str = "Codex Desktop integration is unavailable because a compatible Desktop launcher was not found.\n";
        const COMPATIBILITY_WARNING: &str = concat!(
            "Warning: Codex 9.9.9 has not been tested with Codex Session Control 1.2.3.\n",
            "Some features may not work as expected.\n",
        );
        const PATH_NOTICE: &str = concat!(
            "Note: `/home/test/.local/bin` is not on your PATH.\n",
            "Add it to your PATH to use the short `codex-session-control` command.\n",
        );

        let no_clients = RunningClientFacts::default();
        let mut cases = vec![
            (
                setup_success(
                    no_clients,
                    DesktopAvailability::Available,
                    false,
                    Vec::new(),
                ),
                rendered_success(format!("{SETUP_PRIMARY}\n\n{CLI_GENERIC}\n"), ""),
            ),
            (
                setup_success(
                    RunningClientFacts {
                        cli: true,
                        desktop: false,
                    },
                    DesktopAvailability::Available,
                    false,
                    Vec::new(),
                ),
                rendered_success(format!("{SETUP_PRIMARY}\n\n{CLI_RUNNING}\n"), ""),
            ),
            (
                setup_success(
                    RunningClientFacts {
                        cli: false,
                        desktop: true,
                    },
                    DesktopAvailability::Available,
                    false,
                    Vec::new(),
                ),
                rendered_success(
                    format!("{SETUP_PRIMARY}\n\n{CLI_GENERIC}\n\n{DESKTOP_RUNNING}\n"),
                    "",
                ),
            ),
            (
                setup_success(no_clients, DesktopAvailability::Available, true, Vec::new()),
                rendered_success(
                    format!("{SETUP_PRIMARY}\n\n{CLI_GENERIC}\n\n{DESKTOP_RESTART}\n"),
                    "",
                ),
            ),
            (
                setup_success(
                    no_clients,
                    DesktopAvailability::Unavailable,
                    false,
                    vec![UserNotice::DesktopLauncherUnavailable],
                ),
                rendered_success(
                    format!("{SETUP_PRIMARY}\n\n{CLI_GENERIC}\n"),
                    DESKTOP_WARNING,
                ),
            ),
            (
                setup_success(
                    no_clients,
                    DesktopAvailability::CouldNotVerify,
                    false,
                    vec![UserNotice::Compatibility {
                        codex: Version::parse("9.9.9").unwrap(),
                        product: version(),
                    }],
                ),
                rendered_success(
                    format!("{SETUP_PRIMARY}\n\n{CLI_GENERIC}\n"),
                    COMPATIBILITY_WARNING,
                ),
            ),
            (
                setup_success(
                    no_clients,
                    DesktopAvailability::Available,
                    false,
                    vec![UserNotice::LocalBinMissingFromPath {
                        local_bin: PathBuf::from("/home/test/.local/bin"),
                    }],
                ),
                rendered_success(format!("{SETUP_PRIMARY}\n\n{CLI_GENERIC}\n"), PATH_NOTICE),
            ),
            (
                setup_success(
                    RunningClientFacts {
                        cli: true,
                        desktop: true,
                    },
                    DesktopAvailability::Available,
                    true,
                    vec![
                        UserNotice::Compatibility {
                            codex: Version::parse("9.9.9").unwrap(),
                            product: version(),
                        },
                        UserNotice::LocalBinMissingFromPath {
                            local_bin: PathBuf::from("/home/test/.local/bin"),
                        },
                    ],
                ),
                rendered_success(
                    format!("{SETUP_PRIMARY}\n\n{CLI_RUNNING}\n\n{DESKTOP_RUNNING}\n"),
                    format!("{}\n\n{}", COMPATIBILITY_WARNING.trim_end(), PATH_NOTICE),
                ),
            ),
        ];

        let update_rows = [
            (
                UpdateState::Applied,
                true,
                false,
                concat!(
                    "Codex Session Control was updated to 1.2.3.\n\n",
                    "Start a new task to use the updated plugin.\n",
                ),
            ),
            (
                UpdateState::AlreadyCurrent,
                true,
                false,
                "Codex Session Control 1.2.3 is already up to date.\n",
            ),
            (
                UpdateState::Applied,
                false,
                false,
                concat!(
                    "Codex Session Control was updated to 1.2.3.\n\n",
                    "The service remains disabled. Run `codex-session-control enable` when you want to use it.\n",
                ),
            ),
            (
                UpdateState::AlreadyCurrent,
                false,
                false,
                concat!(
                    "Codex Session Control 1.2.3 is already up to date.\n\n",
                    "The service remains disabled. Run `codex-session-control enable` when you want to use it.\n",
                ),
            ),
            (
                UpdateState::Applied,
                true,
                true,
                concat!(
                    "Codex Session Control was updated to 1.2.3.\n\n",
                    "Start a new task to use the updated plugin.\n\n",
                    "If Codex Desktop is already running, restart it to use the updated version of Codex Session Control.\n",
                ),
            ),
        ];
        for (state, service_enabled, desktop_changed, expected) in update_rows {
            cases.push((
                UserSuccess::Update(UpdateSuccess::new(
                    state,
                    version(),
                    service_enabled,
                    desktop_changed,
                    Vec::new(),
                )),
                rendered_success(expected, ""),
            ));
        }
        cases.push((
            UserSuccess::Update(UpdateSuccess::new(
                UpdateState::Applied,
                version(),
                true,
                true,
                vec![UserNotice::Compatibility {
                    codex: Version::parse("9.9.9").unwrap(),
                    product: version(),
                }],
            )),
            rendered_success(
                concat!(
                    "Codex Session Control was updated to 1.2.3.\n\n",
                    "Start a new task to use the updated plugin.\n\n",
                    "If Codex Desktop is already running, restart it to use the updated version of Codex Session Control.\n",
                ),
                COMPATIBILITY_WARNING,
            ),
        ));

        cases.extend([
            (
                enable_success(
                    no_clients,
                    DesktopAvailability::Available,
                    false,
                    Vec::new(),
                ),
                rendered_success(
                    "Codex Session Control is running and will start automatically.\n",
                    "",
                ),
            ),
            (
                enable_success(
                    RunningClientFacts {
                        cli: true,
                        desktop: false,
                    },
                    DesktopAvailability::Available,
                    false,
                    Vec::new(),
                ),
                rendered_success(
                    format!(
                        "Codex Session Control is running and will start automatically.\n\n{CLI_RUNNING}\n"
                    ),
                    "",
                ),
            ),
            (
                enable_success(
                    RunningClientFacts {
                        cli: false,
                        desktop: true,
                    },
                    DesktopAvailability::Available,
                    false,
                    Vec::new(),
                ),
                rendered_success(
                    format!(
                        "Codex Session Control is running and will start automatically.\n\n{DESKTOP_RUNNING}\n"
                    ),
                    "",
                ),
            ),
            (
                enable_success(
                    no_clients,
                    DesktopAvailability::Available,
                    true,
                    Vec::new(),
                ),
                rendered_success(
                    format!(
                        "Codex Session Control is running and will start automatically.\n\n{DESKTOP_RESTART}\n"
                    ),
                    "",
                ),
            ),
            (
                enable_success(
                    no_clients,
                    DesktopAvailability::SetupRequired,
                    false,
                    Vec::new(),
                ),
                rendered_success(
                    concat!(
                        "Codex Session Control is running and will start automatically.\n\n",
                        "Codex Desktop integration is unavailable.\n",
                        "Run `codex-session-control setup` to set it up.\n",
                    ),
                    "",
                ),
            ),
            (
                enable_success(
                    no_clients,
                    DesktopAvailability::Unavailable,
                    false,
                    vec![UserNotice::DesktopLauncherUnavailable],
                ),
                rendered_success(
                    "Codex Session Control is running and will start automatically.\n",
                    DESKTOP_WARNING,
                ),
            ),
            (
                enable_success(
                    RunningClientFacts {
                        cli: true,
                        desktop: true,
                    },
                    DesktopAvailability::Available,
                    true,
                    vec![UserNotice::LocalBinMissingFromPath {
                        local_bin: PathBuf::from("/home/test/.local/bin"),
                    }],
                ),
                rendered_success(
                    format!(
                        "Codex Session Control is running and will start automatically.\n\n{CLI_RUNNING}\n\n{DESKTOP_RUNNING}\n"
                    ),
                    PATH_NOTICE,
                ),
            ),
            (
                UserSuccess::Disable(DisableSuccess::new(false)),
                rendered_success(
                    concat!(
                        "Codex Session Control is stopped and will not start automatically.\n\n",
                        "Your Codex data is unchanged.\n",
                    ),
                    "",
                ),
            ),
            (
                UserSuccess::Disable(DisableSuccess::new(true)),
                rendered_success(
                    concat!(
                        "Codex Session Control is stopped and will not start automatically.\n\n",
                        "Your Codex data is unchanged.\n",
                        "If Codex Desktop is already running, restart it to continue without Codex Session Control.\n",
                    ),
                    "",
                ),
            ),
            (
                UserSuccess::Uninstall(UninstallSuccess::new(false)),
                rendered_success(
                    "Codex Session Control was uninstalled.\n\nYour Codex data is unchanged.\n",
                    "",
                ),
            ),
            (
                UserSuccess::Uninstall(UninstallSuccess::new(true)),
                rendered_success(
                    concat!(
                        "Codex Session Control was uninstalled.\n\n",
                        "Your Codex data is unchanged.\n",
                        "If Codex Desktop is already running, restart it to continue without Codex Session Control.\n",
                    ),
                    "",
                ),
            ),
            (
                status_success(
                    StatusState::Healthy,
                    Some(version()),
                    Some(ServiceSummary::RunningAutomatic),
                    IntegrationState::Ready,
                    IntegrationState::Unavailable,
                    Vec::new(),
                ),
                rendered_success(
                    concat!(
                        "Status: healthy\n",
                        "Version: 1.2.3\n",
                        "Service: running, starts automatically\n",
                        "Codex CLI integration: ready\n",
                        "Codex Desktop integration: unavailable\n",
                    ),
                    "",
                ),
            ),
            (
                status_success(
                    StatusState::Disabled,
                    Some(version()),
                    Some(ServiceSummary::StoppedAutomaticOff),
                    IntegrationState::Unavailable,
                    IntegrationState::Unavailable,
                    Vec::new(),
                ),
                rendered_success(
                    concat!(
                        "Status: disabled\n",
                        "Version: 1.2.3\n",
                        "Service: stopped, automatic startup is off\n",
                        "Codex CLI integration: unavailable\n",
                        "Codex Desktop integration: unavailable\n\n",
                        "Run `codex-session-control enable` to start Codex Session Control.\n",
                    ),
                    "",
                ),
            ),
            (
                status_success(
                    StatusState::NotInstalled,
                    None,
                    None,
                    IntegrationState::Unavailable,
                    IntegrationState::Unavailable,
                    Vec::new(),
                ),
                RenderedCli {
                    stdout: concat!(
                        "Status: not installed\n",
                        "Codex CLI integration: unavailable\n",
                        "Codex Desktop integration: unavailable\n\n",
                        "Install Codex Session Control by running:\n",
                        "  codex-session-control setup\n",
                    )
                    .to_owned(),
                    stderr: String::new(),
                    exit_code: 1,
                },
            ),
        ]);

        use StatusProblem::*;
        let status_problems = [
            (
                InvocationContextCouldNotBeVerified,
                "The invocation context could not be verified.",
                false,
            ),
            (
                InstalledStateCouldNotBeVerified,
                "The installed Codex Session Control state could not be verified.",
                false,
            ),
            (
                NativeRegistrationFault,
                "Codex CLI native registration is incorrect.",
                false,
            ),
            (
                NativeRegistrationCouldNotBeVerified,
                "Codex CLI native registration could not be verified.",
                false,
            ),
            (
                ProjectionFault,
                "Codex CLI integration files are incorrect.",
                false,
            ),
            (
                ProjectionCouldNotBeVerified,
                "Codex CLI integration files could not be verified.",
                false,
            ),
            (
                ServiceEnablementCouldNotBeVerified,
                "Automatic service startup could not be verified.",
                false,
            ),
            (
                ServiceConfiguredButStopped,
                "The service is configured to run but is stopped.",
                true,
            ),
            (
                ServiceActivityCouldNotBeVerified,
                "The service state could not be verified.",
                false,
            ),
            (
                SocketMissing,
                "The service connection is unavailable.",
                true,
            ),
            (SocketUnsafe, "The service connection is unsafe.", false),
            (AppServerUnavailable, "The app-server is unavailable.", true),
            (
                AppServerCouldNotBeVerified,
                "The app-server could not be verified.",
                false,
            ),
            (
                DesktopDescriptorFault,
                "Codex Desktop integration is incorrectly configured.",
                false,
            ),
            (
                DesktopCouldNotBeVerified,
                "Codex Desktop integration could not be verified.",
                false,
            ),
        ];
        for (problem, prose, logs) in status_problems {
            cases.push((
                status_success(
                    StatusState::Unhealthy,
                    Some(version()),
                    Some(ServiceSummary::CouldNotVerify),
                    IntegrationState::CouldNotVerify,
                    IntegrationState::Unhealthy,
                    vec![problem],
                ),
                RenderedCli {
                    stdout: unhealthy_status_oracle(
                        ServiceSummary::CouldNotVerify,
                        IntegrationState::CouldNotVerify,
                        IntegrationState::Unhealthy,
                        &[(prose, logs)],
                    ),
                    stderr: String::new(),
                    exit_code: 1,
                },
            ));
        }
        cases.push((
            status_success(
                StatusState::Unhealthy,
                Some(version()),
                Some(ServiceSummary::StoppedUnexpectedAutomaticOn),
                IntegrationState::Unhealthy,
                IntegrationState::Unhealthy,
                vec![
                    StatusProblem::ServiceConfiguredButStopped,
                    StatusProblem::SocketMissing,
                ],
            ),
            RenderedCli {
                stdout: unhealthy_status_oracle(
                    ServiceSummary::StoppedUnexpectedAutomaticOn,
                    IntegrationState::Unhealthy,
                    IntegrationState::Unhealthy,
                    &[
                        ("The service is configured to run but is stopped.", true),
                        ("The service connection is unavailable.", true),
                    ],
                ),
                stderr: String::new(),
                exit_code: 1,
            },
        ));
        cases
    }

    #[test]
    fn update_completion_unknown_is_stderr_only_exit_one_without_retry() {
        let rendered = UserFailure::UpdateCompletionUnknown.render();
        assert!(rendered.stdout.is_empty());
        assert_eq!(rendered.stderr, UPDATE_COMPLETION_UNKNOWN);
        assert_eq!(rendered.exit_code, 1);
        assert!(!rendered.stderr.contains("Try again"));
    }

    #[test]
    fn every_materially_distinct_failure_block_is_exact() {
        for (ordinary, expected) in ordinary_literal_cases() {
            assert_eq!(
                UserFailure::Ordinary(ordinary).render(),
                rendered_failure(expected)
            );
        }
        for (failure, expected) in failure_render_cases() {
            assert_eq!(failure.render(), expected);
            assert!(expected.stdout.is_empty());
            assert_eq!(expected.exit_code, 1);
            assert!(expected.stderr.ends_with('\n'));
        }
    }

    #[test]
    fn every_materially_distinct_success_and_notice_block_is_exact() {
        for (success, expected) in success_render_cases() {
            assert_eq!(success.render(), expected);
            assert!(expected.stdout.ends_with('\n'));
        }

        assert!(
            SetupSuccess::new(
                version(),
                RunningClientFacts::default(),
                DesktopAvailability::Unavailable,
                true,
                Vec::new(),
            )
            .is_none()
        );
        assert!(
            SetupSuccess::new(
                version(),
                RunningClientFacts::default(),
                DesktopAvailability::CouldNotVerify,
                true,
                Vec::new(),
            )
            .is_none()
        );
        assert!(
            EnableSuccess::new(
                RunningClientFacts::default(),
                DesktopAvailability::SetupRequired,
                true,
                Vec::new(),
            )
            .is_none()
        );
    }

    #[test]
    fn status_renderer_and_exit_matrix_are_exact() {
        use StatusState::*;
        let cases = success_render_cases();
        for (state, exit) in [
            (Healthy, 0),
            (Disabled, 0),
            (NotInstalled, 1),
            (Unhealthy, 1),
        ] {
            assert!(cases.iter().any(|(success, rendered)| matches!(success,
                UserSuccess::Status(status) if status.state == state && rendered.exit_code == exit
            )));
        }
    }
}
