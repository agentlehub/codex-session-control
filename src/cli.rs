use std::{ffi::OsString, path::PathBuf};

use clap::{Parser, Subcommand};

#[cfg(target_arch = "x86_64")]
pub const BUILD_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (x86_64-unknown-linux-gnu)");
#[cfg(target_arch = "aarch64")]
pub const BUILD_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (aarch64-unknown-linux-gnu)");

#[derive(Debug, Parser)]
#[command(
    name = "codex-session-control",
    version = BUILD_VERSION,
    about = "Manage Codex Session Control",
    disable_help_subcommand = true
)]
pub(crate) struct Cli {
    #[arg(long, global = true, help = "Show diagnostic details")]
    pub(crate) verbose: bool,
    #[command(subcommand)]
    pub(crate) command: Command,
}

impl Cli {
    pub(crate) fn parse() -> Self {
        match Self::try_parse_from(std::env::args_os()) {
            Ok(cli) => cli,
            Err(error) => error.exit(),
        }
    }

    pub(crate) fn try_parse_from<I, T>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let mut args: Vec<OsString> = args.into_iter().map(Into::into).collect();
        if let Some(codex_index) = args.iter().position(|arg| arg == "codex")
            && args
                .get(codex_index + 1)
                .is_some_and(|arg| arg == "--verbose")
        {
            args.insert(codex_index + 1, OsString::from("--"));
        }

        <Self as Parser>::try_parse_from(args)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub(crate) enum Command {
    #[command(about = "Install Codex Session Control and start the shared app-server")]
    Setup {
        #[arg(
            long,
            value_name = "PATH",
            help = "Absolute path to the Codex Desktop executable when automatic discovery fails",
            value_parser = parse_absolute_launcher_path
        )]
        desktop_launcher: Option<PathBuf>,
    },
    #[command(about = "Install the latest release")]
    Update,
    #[command(about = "Check whether Codex Session Control is ready")]
    Status,
    #[command(about = "Start the service and turn on automatic startup")]
    Enable,
    #[command(about = "Stop the service and turn off automatic startup")]
    Disable,
    #[command(about = "Remove the service while keeping your Codex data")]
    Uninstall,
    #[command(name = "mcp-server", hide = true)]
    McpServer,
    #[command(
        about = "Start Codex CLI through the shared app-server",
        override_help = "Start Codex CLI through the shared app-server\n\nUsage: codex-session-control codex [ARGS]...\n\nArguments:\n  [ARGS]...  Arguments passed directly to Codex CLI\n\nOptions:\n  -h, --help  Print help\n"
    )]
    Codex {
        #[arg(
            help = "Arguments passed directly to Codex CLI",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        args: Vec<OsString>,
    },
}

fn parse_absolute_launcher_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        Err("must be an absolute path".to_owned())
    } else if path.extension() == Some(std::ffi::OsStr::new("desktop")) {
        Err("must be an absolute executable path, not a desktop entry".to_owned())
    } else {
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    use super::{Cli, Command};

    #[test]
    fn desktop_entry_override_is_rejected_at_the_parser_boundary() {
        assert!(
            Cli::try_parse_from([
                "codex-session-control",
                "setup",
                "--desktop-launcher",
                "/opt/codex-desktop.desktop",
            ])
            .is_err()
        );
    }

    #[test]
    fn codex_preserves_trailing_arguments_as_os_strings() {
        let user_args = vec![
            OsString::from("--model"),
            OsString::from("two words"),
            OsString::from_vec(b"--remote\xff".to_vec()),
            OsString::from("unix://user-supplied"),
        ];
        let cli = Cli::try_parse_from(
            std::iter::once(OsString::from("codex-session-control"))
                .chain(std::iter::once(OsString::from("codex")))
                .chain(user_args.clone()),
        )
        .unwrap();

        assert_eq!(cli.command, Command::Codex { args: user_args });
    }

    #[test]
    fn verbose_placement_and_codex_passthrough_are_exact() {
        let cli = Cli::try_parse_from(["csc", "--verbose", "codex", "--verbose"]).unwrap();
        assert!(cli.verbose);
        let Command::Codex { args } = cli.command else {
            panic!("expected codex")
        };
        assert_eq!(args, vec![OsString::from("--verbose")]);

        assert!(
            Cli::try_parse_from(["csc", "setup", "--verbose"])
                .unwrap()
                .verbose
        );
    }

    #[test]
    fn mcp_server_remains_callable_while_hidden() {
        assert!(matches!(
            Cli::try_parse_from(["csc", "mcp-server"]),
            Ok(Cli {
                command: Command::McpServer,
                ..
            })
        ));
    }
}
