use std::io::{self, ErrorKind, Write};
use std::process::ExitCode;

use clap::Parser;
use nestbruno::cli::Cli;
use nestbruno::commands::{self, Outcome};
use nestbruno::diag::Reporter;
use nestbruno::error::{Error, exit_code};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let reporter = Reporter::new(cli.verbose);
    let mut stdout = io::BufWriter::new(io::stdout().lock());

    let result = commands::execute(&cli, &mut stdout, &reporter);
    let flushed = stdout.flush();

    match result {
        Ok(outcome) => match flushed {
            Ok(()) => match outcome {
                Outcome::Success => ExitCode::SUCCESS,
                Outcome::OutOfSync => ExitCode::from(exit_code::FAILURE),
                Outcome::Incomplete => ExitCode::from(exit_code::INCOMPLETE),
            },
            Err(err) => finish_with_error(&Error::Output(err)),
        },
        Err(err) => finish_with_error(&err),
    }
}

/// Print `err` and map it to an exit code. A closed stdout (`nestbruno ... | head`)
/// is not a failure of the tool, so it exits quietly.
fn finish_with_error(err: &Error) -> ExitCode {
    if let Error::Output(io_err) = err
        && io_err.kind() == ErrorKind::BrokenPipe
    {
        return ExitCode::SUCCESS;
    }
    // Nowhere to report if stderr itself is closed.
    let _ = writeln!(io::stderr().lock(), "error: {err}");
    ExitCode::from(err.exit_code())
}
