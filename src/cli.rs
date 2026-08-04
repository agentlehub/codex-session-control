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
    about,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum Command {
    Setup {
        #[arg(long, value_parser = parse_absolute_launcher_path)]
        desktop_launcher: Option<PathBuf>,
    },
    Update,
    Status,
    Enable,
    Disable,
    #[command(
        about = "Remove Codex session control while preserving the selected normal Codex home, authentication, tasks, and rollouts"
    )]
    Uninstall,
    #[command(name = "mcp-server")]
    McpServer,
    Codex {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
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

    use clap::Parser;

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
}
