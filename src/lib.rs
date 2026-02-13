use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;

mod app;
mod cli;
mod commands;
mod domain;
mod infra;
mod output;

#[must_use]
pub fn run() -> ExitCode {
    run_from(std::env::args_os())
}

#[must_use]
pub fn run_from<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let preprocessed = cli::preprocess_args(args);
    let parsed = match cli::Cli::try_parse_from(preprocessed) {
        Ok(parsed) => parsed,
        Err(err) => {
            let code = err.exit_code();
            let _ = err.print();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            return ExitCode::from(code.min(255) as u8);
        }
    };

    let code = app::execute(parsed);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    ExitCode::from(code.clamp(0, 255) as u8)
}
