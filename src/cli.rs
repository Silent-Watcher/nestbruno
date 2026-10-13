//! Command-line interface definition. Parsing only; behavior lives in
//! [`crate::commands`].

use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
    Discover,
    /// Print the routes extracted from the NestJS sources.
    Routes,
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
            default_value_t = 200,
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
        assert!(matches!(cli.command, Command::Sync { dry_run: true }));
        let cli = Cli::try_parse_from(["nestbruno", "sync"]).unwrap();
        assert!(matches!(cli.command, Command::Sync { dry_run: false }));
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
    fn unknown_subcommands_are_rejected() {
        assert!(Cli::try_parse_from(["nestbruno", "frobnicate"]).is_err());
    }
}
