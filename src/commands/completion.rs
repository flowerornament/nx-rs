use clap::CommandFactory;
use clap_complete::{Generator, Shell, generate};

use crate::cli::{Cli, CompletionArgs, CompletionShellArg};

pub fn cmd_completion(args: &CompletionArgs) -> i32 {
    let mut command = Cli::command();
    let bin_name = command.get_name().to_string();
    match args.shell {
        CompletionShellArg::Bash => generate_for(Shell::Bash, &mut command, &bin_name),
        CompletionShellArg::Elvish => generate_for(Shell::Elvish, &mut command, &bin_name),
        CompletionShellArg::Fish => generate_for(Shell::Fish, &mut command, &bin_name),
        CompletionShellArg::PowerShell => generate_for(Shell::PowerShell, &mut command, &bin_name),
        CompletionShellArg::Zsh => generate_for(Shell::Zsh, &mut command, &bin_name),
    }
    0
}

fn generate_for<G: Generator>(generator: G, command: &mut clap::Command, bin_name: &str) {
    generate(generator, command, bin_name, &mut std::io::stdout());
}
