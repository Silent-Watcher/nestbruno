//! Command-line interface definition. Parsing only; behavior lives in
//! [`crate::commands`].

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

/// Output format of a command that produces structured results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text (the default).
    Text,
    /// One complete JSON document on stdout; diagnostics are also structured
    /// there, progress messages stay on stderr.
    Json,
}

#[derive(Debug, Parser)]
#[command(
    name = "nestbruno",
    version,
    about = "Synchronize NestJS controllers into a Bruno API collection",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Use this directory as the project root instead of searching upwards
    /// from the current directory.
    #[arg(short = 'C', long = "project", global = true, value_name = "DIR")]
    pub project: Option<PathBuf>,

    /// Print additional diagnostics to stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a nestbruno.toml with the default settings.
    Init,
    /// Validate the configuration and project layout.
    Check,
    /// List the files that may contain NestJS controllers.
    Discover {
        /// Select the output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },
    /// Print the routes extracted from the NestJS sources.
    Routes {
        /// Select the output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },
    /// Generate Bruno request files from the NestJS routes.
    ///
    /// Existing files are never modified unless --force is given. Requests
    /// for routes that no longer exist are not removed.
    Generate {
        /// Replace existing request files whose content differs.
        #[arg(long)]
        force: bool,
    },
    /// Synchronize the NestJS routes with the Bruno collection.
    ///
    /// Creates requests for new routes, updates unmodified generated requests,
    /// deletes unmodified generated requests of removed routes, and never
    /// touches requests you wrote or modified (those are reported as conflicts).
    Sync {
        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Synchronize the state represented by the git index (staged files)
        /// instead of the working tree.
        #[arg(long)]
        staged: bool,
        /// Select the output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },
    /// Print shell completion scripts to stdout.
    ///
    /// The script is generated from the actual command tree; nothing is
    /// written to disk and no shell configuration is touched. Redirect the
    /// output to a file to install it.
    Completions {
        /// The shell to generate completions for.
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Check whether the collection matches the sources, without writing.
    ///
    /// Exits 1 when the collection is out of sync (suitable for CI), 4 on
    /// conflicts or an incomplete analysis. Nothing is ever modified.
    Verify {
        /// Compare against the git index (staged files) instead of the
        /// working tree.
        #[arg(long)]
        staged: bool,
        /// Select the output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },
    /// Watch the sources and keep the collection synchronized.
    ///
    /// Only the changed files are re-analyzed. Stops cleanly on Ctrl-C.
    Watch {
        /// Quiet time after the last file event before synchronizing, in
        /// milliseconds.
        #[arg(
            long,
            value_name = "MS",
            default_value_t = crate::watch::WatchOptions::DEFAULT_DEBOUNCE_MS,
            value_parser = clap::value_parser!(u64).range(10..=60_000)
        )]
        debounce: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_are_accepted_after_the_subcommand() {
        let cli = Cli::try_parse_from(["nestbruno", "check", "-v", "-C", "some/dir"]).unwrap();
        assert!(cli.verbose);
        assert_eq!(cli.project, Some(PathBuf::from("some/dir")));
        assert!(matches!(cli.command, Command::Check));
    }

    #[test]
    fn generate_accepts_force() {
        let cli = Cli::try_parse_from(["nestbruno", "generate"]).unwrap();
        assert!(matches!(cli.command, Command::Generate { force: false }));
        let cli = Cli::try_parse_from(["nestbruno", "generate", "--force"]).unwrap();
        assert!(matches!(cli.command, Command::Generate { force: true }));
        assert!(Cli::try_parse_from(["nestbruno", "--force", "check"]).is_err());
    }

    #[test]
    fn sync_accepts_dry_run() {
        let cli = Cli::try_parse_from(["nestbruno", "sync", "--dry-run"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Sync {
                dry_run: true,
                staged: false,
                format: OutputFormat::Text
            }
        ));
        let cli = Cli::try_parse_from(["nestbruno", "sync"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Sync {
                dry_run: false,
                staged: false,
                format: OutputFormat::Text
            }
        ));
        let cli = Cli::try_parse_from(["nestbruno", "sync", "--staged", "--dry-run"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Sync {
                dry_run: true,
                staged: true,
                format: OutputFormat::Text
            }
        ));
    }

    #[test]
    fn format_defaults_to_text_and_accepts_json() {
        let cli = Cli::try_parse_from(["nestbruno", "routes"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Routes {
                format: OutputFormat::Text
            }
        ));
        let cli = Cli::try_parse_from(["nestbruno", "routes", "--format", "json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Routes {
                format: OutputFormat::Json
            }
        ));
        let cli = Cli::try_parse_from(["nestbruno", "discover", "--format", "json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Discover {
                format: OutputFormat::Json
            }
        ));
        let cli = Cli::try_parse_from(["nestbruno", "sync", "--format", "json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Sync {
                format: OutputFormat::Json,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["nestbruno", "routes", "--format", "yaml"]).is_err());
    }

    #[test]
    fn watch_accepts_a_bounded_debounce() {
        let cli = Cli::try_parse_from(["nestbruno", "watch"]).unwrap();
        assert!(matches!(cli.command, Command::Watch { debounce: 200 }));
        let cli = Cli::try_parse_from(["nestbruno", "watch", "--debounce", "50"]).unwrap();
        assert!(matches!(cli.command, Command::Watch { debounce: 50 }));
        for bad in ["0", "9", "60001", "abc", "-5"] {
            assert!(
                Cli::try_parse_from(["nestbruno", "watch", "--debounce", bad]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn verify_accepts_staged_and_format() {
        let cli = Cli::try_parse_from(["nestbruno", "verify"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Verify {
                staged: false,
                format: OutputFormat::Text
            }
        ));
        let cli =
            Cli::try_parse_from(["nestbruno", "verify", "--staged", "--format", "json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Verify {
                staged: true,
                format: OutputFormat::Json
            }
        ));
    }

    #[test]
    fn unknown_subcommands_are_rejected() {
        assert!(Cli::try_parse_from(["nestbruno", "frobnicate"]).is_err());
    }
}
