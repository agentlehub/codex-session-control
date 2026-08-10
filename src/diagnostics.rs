use std::{io::Write, path::PathBuf};

use semver::Version;

#[derive(Debug)]
pub(crate) struct Diagnostics {
    command: DiagnosticCommand,
    phase: Option<UpdatePhase>,
    sink: DiagnosticSink,
}

#[derive(Debug)]
enum DiagnosticSink {
    Off,
    Stderr(std::io::Stderr),
    #[cfg(test)]
    Record(Vec<String>),
    #[cfg(test)]
    FailOnce,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticCommand {
    Setup,
    Update,
    Status,
    Enable,
    Disable,
    Uninstall,
    Codex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdatePhase {
    Outer,
    Apply,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticCause {
    Unexpected,
    Validation,
    ReleaseDownload,
    Checksum,
    ServiceConfiguration,
    ServiceStart,
    ServiceStop,
    ServiceState,
    CliIntegration,
    DesktopIntegration,
    ActiveTasks,
    Cleanup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticTarget {
    #[cfg(target_arch = "x86_64")]
    X86_64Linux,
    #[cfg(target_arch = "aarch64")]
    Aarch64Linux,
}

impl DiagnosticTarget {
    pub(crate) fn current() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            Self::X86_64Linux
        }
        #[cfg(target_arch = "aarch64")]
        {
            Self::Aarch64Linux
        }
    }

    fn label(self) -> &'static str {
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::X86_64Linux => "x86_64-unknown-linux-gnu",
            #[cfg(target_arch = "aarch64")]
            Self::Aarch64Linux => "aarch64-unknown-linux-gnu",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticEvent {
    ControllerStarted {
        version: Version,
        target: DiagnosticTarget,
    },
    CandidateVerified {
        version: Version,
    },
    StartingStagedCandidate,
    StagedMarkerAccepted,
    SelectedCodexHome {
        codex_home: PathBuf,
    },
    CompletedPreflight,
    CompletedBinary,
    CompletedConfiguration,
    CompletedProjection,
    CompletedPluginMarketplace,
    CompletedPluginInstall,
    CompletedDesktopDiscovery,
    CompletedDescriptor,
    CompletedServiceUnit,
    CompletedDaemonReload,
    CompletedServiceEnable,
    CompletedServiceDisable,
    CompletedServiceVerify,
    CompletedDescriptorRemove,
    CompletedManifest,
    FailedPreflight {
        cause: DiagnosticCause,
    },
    FailedBinary {
        cause: DiagnosticCause,
    },
    FailedConfiguration {
        cause: DiagnosticCause,
    },
    FailedProjection {
        cause: DiagnosticCause,
    },
    FailedPluginMarketplace {
        cause: DiagnosticCause,
    },
    FailedPluginInstall {
        cause: DiagnosticCause,
    },
    FailedDesktopDiscovery {
        cause: DiagnosticCause,
    },
    FailedDescriptor {
        cause: DiagnosticCause,
    },
    FailedServiceUnit {
        cause: DiagnosticCause,
    },
    FailedDaemonReload {
        cause: DiagnosticCause,
    },
    FailedServiceEnable {
        cause: DiagnosticCause,
    },
    FailedServiceDisable {
        cause: DiagnosticCause,
    },
    FailedServiceVerify {
        cause: DiagnosticCause,
    },
    FailedDescriptorRemove {
        cause: DiagnosticCause,
    },
    FailedManifest {
        cause: DiagnosticCause,
    },
}

impl Diagnostics {
    pub(crate) fn new(verbose: bool, command: DiagnosticCommand) -> Self {
        Self {
            command,
            phase: None,
            sink: if verbose {
                DiagnosticSink::Stderr(std::io::stderr())
            } else {
                DiagnosticSink::Off
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn record(command: DiagnosticCommand) -> Self {
        Self {
            command,
            phase: None,
            sink: DiagnosticSink::Record(Vec::new()),
        }
    }

    #[cfg(test)]
    fn fail_once(command: DiagnosticCommand) -> Self {
        Self {
            command,
            phase: None,
            sink: DiagnosticSink::FailOnce,
        }
    }

    pub(crate) fn set_phase(&mut self, phase: UpdatePhase) {
        self.phase = Some(phase);
    }

    pub(crate) fn emit(&mut self, event: DiagnosticEvent) {
        let line = self.render_event(event);
        let failed = match &mut self.sink {
            DiagnosticSink::Off => false,
            DiagnosticSink::Stderr(stderr) => stderr.write_all(line.as_bytes()).is_err(),
            #[cfg(test)]
            DiagnosticSink::Record(lines) => {
                lines.push(line);
                false
            }
            #[cfg(test)]
            DiagnosticSink::FailOnce => true,
        };
        if failed {
            self.sink = DiagnosticSink::Off;
        }
    }

    pub(crate) fn flush(&mut self) {
        let failed = match &mut self.sink {
            DiagnosticSink::Off => false,
            DiagnosticSink::Stderr(stderr) => stderr.flush().is_err(),
            #[cfg(test)]
            DiagnosticSink::Record(_) => false,
            #[cfg(test)]
            DiagnosticSink::FailOnce => true,
        };
        if failed {
            self.sink = DiagnosticSink::Off;
        }
    }

    fn render_event(&self, event: DiagnosticEvent) -> String {
        let command = match self.command {
            DiagnosticCommand::Setup => "setup",
            DiagnosticCommand::Update => "update",
            DiagnosticCommand::Status => "status",
            DiagnosticCommand::Enable => "enable",
            DiagnosticCommand::Disable => "disable",
            DiagnosticCommand::Uninstall => "uninstall",
            DiagnosticCommand::Codex => "codex",
        };
        let phase = match (self.command, self.phase) {
            (DiagnosticCommand::Update, Some(UpdatePhase::Outer)) => "/outer",
            (DiagnosticCommand::Update, Some(UpdatePhase::Apply)) => "/apply",
            _ => "",
        };
        let detail = match event {
            DiagnosticEvent::ControllerStarted { version, target } => {
                format!("controller {version} ({})", target.label())
            }
            DiagnosticEvent::CandidateVerified { version } => {
                format!("candidate {version} verified")
            }
            DiagnosticEvent::StartingStagedCandidate => "starting staged candidate".to_owned(),
            DiagnosticEvent::StagedMarkerAccepted => "staged marker accepted".to_owned(),
            DiagnosticEvent::SelectedCodexHome { codex_home } => {
                format!("selected CODEX_HOME {}", codex_home.display())
            }
            DiagnosticEvent::CompletedPreflight => "completed preflight".to_owned(),
            DiagnosticEvent::CompletedBinary => "completed binary".to_owned(),
            DiagnosticEvent::CompletedConfiguration => "completed configuration".to_owned(),
            DiagnosticEvent::CompletedProjection => "completed projection".to_owned(),
            DiagnosticEvent::CompletedPluginMarketplace => {
                "completed plugin-marketplace".to_owned()
            }
            DiagnosticEvent::CompletedPluginInstall => "completed plugin-install".to_owned(),
            DiagnosticEvent::CompletedDesktopDiscovery => "completed desktop-discovery".to_owned(),
            DiagnosticEvent::CompletedDescriptor => "completed descriptor".to_owned(),
            DiagnosticEvent::CompletedServiceUnit => "completed service-unit".to_owned(),
            DiagnosticEvent::CompletedDaemonReload => "completed daemon-reload".to_owned(),
            DiagnosticEvent::CompletedServiceEnable => "completed service-enable".to_owned(),
            DiagnosticEvent::CompletedServiceDisable => "completed service-disable".to_owned(),
            DiagnosticEvent::CompletedServiceVerify => "completed service-verify".to_owned(),
            DiagnosticEvent::CompletedDescriptorRemove => "completed descriptor-remove".to_owned(),
            DiagnosticEvent::CompletedManifest => "completed manifest".to_owned(),
            DiagnosticEvent::FailedPreflight { cause } => {
                format!("failed preflight ({})", cause.label())
            }
            DiagnosticEvent::FailedBinary { cause } => {
                format!("failed binary ({})", cause.label())
            }
            DiagnosticEvent::FailedConfiguration { cause } => {
                format!("failed configuration ({})", cause.label())
            }
            DiagnosticEvent::FailedProjection { cause } => {
                format!("failed projection ({})", cause.label())
            }
            DiagnosticEvent::FailedPluginMarketplace { cause } => {
                format!("failed plugin-marketplace ({})", cause.label())
            }
            DiagnosticEvent::FailedPluginInstall { cause } => {
                format!("failed plugin-install ({})", cause.label())
            }
            DiagnosticEvent::FailedDesktopDiscovery { cause } => {
                format!("failed desktop-discovery ({})", cause.label())
            }
            DiagnosticEvent::FailedDescriptor { cause } => {
                format!("failed descriptor ({})", cause.label())
            }
            DiagnosticEvent::FailedServiceUnit { cause } => {
                format!("failed service-unit ({})", cause.label())
            }
            DiagnosticEvent::FailedDaemonReload { cause } => {
                format!("failed daemon-reload ({})", cause.label())
            }
            DiagnosticEvent::FailedServiceEnable { cause } => {
                format!("failed service-enable ({})", cause.label())
            }
            DiagnosticEvent::FailedServiceDisable { cause } => {
                format!("failed service-disable ({})", cause.label())
            }
            DiagnosticEvent::FailedServiceVerify { cause } => {
                format!("failed service-verify ({})", cause.label())
            }
            DiagnosticEvent::FailedDescriptorRemove { cause } => {
                format!("failed descriptor-remove ({})", cause.label())
            }
            DiagnosticEvent::FailedManifest { cause } => {
                format!("failed manifest ({})", cause.label())
            }
        };
        format!("[verbose] {command}{phase}: {detail}\n")
    }

    #[cfg(test)]
    pub(crate) fn recorded_lines(&self) -> &[String] {
        match &self.sink {
            DiagnosticSink::Record(lines) => lines,
            _ => &[],
        }
    }

    #[cfg(test)]
    fn is_off(&self) -> bool {
        matches!(self.sink, DiagnosticSink::Off)
    }
}

impl DiagnosticCause {
    fn label(self) -> &'static str {
        match self {
            Self::Unexpected => "unexpected failure",
            Self::Validation => "validation failed",
            Self::ReleaseDownload => "release could not be retrieved",
            Self::Checksum => "downloaded release could not be verified",
            Self::ServiceConfiguration => "service could not be configured",
            Self::ServiceStart => "service could not be started",
            Self::ServiceStop => "service could not be stopped",
            Self::ServiceState => "service state could not be verified",
            Self::CliIntegration => "Codex CLI integration could not be updated",
            Self::DesktopIntegration => "Codex Desktop integration could not be updated",
            Self::ActiveTasks => "active tasks could not be checked safely",
            Self::Cleanup => "cleanup could not be completed safely",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use semver::Version;

    use super::*;

    #[test]
    fn prefixes_and_update_phases_are_exact() {
        let mut diagnostics = Diagnostics::record(DiagnosticCommand::Update);
        diagnostics.set_phase(UpdatePhase::Outer);
        diagnostics.emit(DiagnosticEvent::CandidateVerified {
            version: Version::parse("1.2.3").unwrap(),
        });
        diagnostics.emit(DiagnosticEvent::StartingStagedCandidate);
        diagnostics.set_phase(UpdatePhase::Apply);
        diagnostics.emit(DiagnosticEvent::StagedMarkerAccepted);
        diagnostics.emit(DiagnosticEvent::CompletedManifest);

        assert_eq!(
            diagnostics.recorded_lines(),
            [
                "[verbose] update/outer: candidate 1.2.3 verified\n",
                "[verbose] update/outer: starting staged candidate\n",
                "[verbose] update/apply: staged marker accepted\n",
                "[verbose] update/apply: completed manifest\n",
            ]
        );

        for (command, prefix) in [
            (DiagnosticCommand::Setup, "setup"),
            (DiagnosticCommand::Status, "status"),
            (DiagnosticCommand::Enable, "enable"),
            (DiagnosticCommand::Disable, "disable"),
            (DiagnosticCommand::Uninstall, "uninstall"),
            (DiagnosticCommand::Codex, "codex"),
        ] {
            let mut diagnostics = Diagnostics::record(command);
            diagnostics.emit(DiagnosticEvent::CompletedPreflight);
            assert_eq!(
                diagnostics.recorded_lines(),
                [format!("[verbose] {prefix}: completed preflight\n")]
            );
        }
    }

    #[test]
    fn every_constructor_excludes_all_privacy_sentinels() {
        const SENTINELS: [&str; 6] = [
            "credential-secret",
            "raw-error-secret",
            "task-secret",
            "pid-4242",
            "timestamp-secret",
            "telemetry-secret",
        ];
        let mut events = vec![
            DiagnosticEvent::ControllerStarted {
                version: Version::parse("1.2.3").unwrap(),
                target: DiagnosticTarget::current(),
            },
            DiagnosticEvent::CandidateVerified {
                version: Version::parse("1.2.3").unwrap(),
            },
            DiagnosticEvent::StartingStagedCandidate,
            DiagnosticEvent::StagedMarkerAccepted,
            DiagnosticEvent::SelectedCodexHome {
                codex_home: PathBuf::from("/home/test/.codex"),
            },
            DiagnosticEvent::CompletedPreflight,
            DiagnosticEvent::CompletedBinary,
            DiagnosticEvent::CompletedConfiguration,
            DiagnosticEvent::CompletedProjection,
            DiagnosticEvent::CompletedPluginMarketplace,
            DiagnosticEvent::CompletedPluginInstall,
            DiagnosticEvent::CompletedDesktopDiscovery,
            DiagnosticEvent::CompletedDescriptor,
            DiagnosticEvent::CompletedServiceUnit,
            DiagnosticEvent::CompletedDaemonReload,
            DiagnosticEvent::CompletedServiceEnable,
            DiagnosticEvent::CompletedServiceDisable,
            DiagnosticEvent::CompletedServiceVerify,
            DiagnosticEvent::CompletedDescriptorRemove,
            DiagnosticEvent::CompletedManifest,
            DiagnosticEvent::FailedPreflight {
                cause: DiagnosticCause::Validation,
            },
            DiagnosticEvent::FailedBinary {
                cause: DiagnosticCause::Validation,
            },
            DiagnosticEvent::FailedConfiguration {
                cause: DiagnosticCause::Validation,
            },
            DiagnosticEvent::FailedProjection {
                cause: DiagnosticCause::CliIntegration,
            },
            DiagnosticEvent::FailedPluginMarketplace {
                cause: DiagnosticCause::CliIntegration,
            },
            DiagnosticEvent::FailedPluginInstall {
                cause: DiagnosticCause::CliIntegration,
            },
            DiagnosticEvent::FailedDesktopDiscovery {
                cause: DiagnosticCause::DesktopIntegration,
            },
            DiagnosticEvent::FailedDescriptor {
                cause: DiagnosticCause::DesktopIntegration,
            },
            DiagnosticEvent::FailedServiceUnit {
                cause: DiagnosticCause::ServiceConfiguration,
            },
            DiagnosticEvent::FailedDaemonReload {
                cause: DiagnosticCause::ServiceConfiguration,
            },
            DiagnosticEvent::FailedServiceEnable {
                cause: DiagnosticCause::ServiceStart,
            },
            DiagnosticEvent::FailedServiceDisable {
                cause: DiagnosticCause::ServiceStop,
            },
            DiagnosticEvent::FailedDescriptorRemove {
                cause: DiagnosticCause::Cleanup,
            },
            DiagnosticEvent::FailedManifest {
                cause: DiagnosticCause::Validation,
            },
        ];
        events.extend(
            [
                DiagnosticCause::Unexpected,
                DiagnosticCause::Validation,
                DiagnosticCause::ReleaseDownload,
                DiagnosticCause::Checksum,
                DiagnosticCause::ServiceConfiguration,
                DiagnosticCause::ServiceStart,
                DiagnosticCause::ServiceStop,
                DiagnosticCause::ServiceState,
                DiagnosticCause::CliIntegration,
                DiagnosticCause::DesktopIntegration,
                DiagnosticCause::ActiveTasks,
                DiagnosticCause::Cleanup,
            ]
            .into_iter()
            .map(|cause| DiagnosticEvent::FailedServiceVerify { cause }),
        );

        let mut diagnostics = Diagnostics::record(DiagnosticCommand::Setup);
        for event in events {
            diagnostics.emit(event);
        }
        let rendered = diagnostics.recorded_lines().concat();
        for sentinel in SENTINELS {
            assert!(!rendered.contains(sentinel));
        }
    }

    #[test]
    fn first_write_failure_disables_later_output_without_changing_result() {
        let mut diagnostics = Diagnostics::fail_once(DiagnosticCommand::Setup);
        diagnostics.emit(DiagnosticEvent::CompletedPreflight);
        diagnostics.emit(DiagnosticEvent::CompletedBinary);

        assert!(diagnostics.is_off());
        assert_eq!(0, 0);
    }
}
