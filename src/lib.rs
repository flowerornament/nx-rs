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
            return exit_code_from_i32(code);
        }
    };

    let code = app::execute(parsed);
    exit_code_from_i32(code)
}

fn exit_code_from_i32(code: i32) -> ExitCode {
    let clamped = code.clamp(i32::from(u8::MIN), i32::from(u8::MAX));
    let exit_code = u8::try_from(clamped).unwrap_or(u8::MAX);
    ExitCode::from(exit_code)
}
