use std::path::Path;

use crate::commands::context::AppContext;
use crate::infra::shell::run_indented_command;
use crate::output::printer::Printer;

pub fn cmd_test(ctx: &AppContext) -> i32 {
    let scripts_nx = ctx.repo_root.join("scripts/nx");
    let steps: [(&str, &str, &[&str], Option<&Path>); 3] = [
        ("ruff", "ruff", &["check", "."], Some(&scripts_nx)),
        ("mypy", "mypy", &["."], Some(&scripts_nx)),
        (
            "tests",
            "python3",
            &["-m", "unittest", "discover", "-s", "scripts/nx/tests"],
            Some(&ctx.repo_root),
        ),
    ];

    for (label, program, args, cwd) in steps {
        if run_test_step(label, program, args, cwd, &ctx.printer).is_err() {
            return 1;
        }
    }

    0
}

fn run_test_step(
    label: &str,
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    printer: &Printer,
) -> Result<(), ()> {
    printer.action(&format!("Running {label}"));
    println!();

    let return_code = match run_indented_command(program, args, cwd, printer, "  ") {
        Ok(code) => code,
        Err(err) => {
            printer.error(&format!("{label} failed"));
            printer.error(&format!("{err:#}"));
            return Err(());
        }
    };

    if return_code != 0 {
        printer.error(&format!("{label} failed"));
        return Err(());
    }

    println!();
    printer.success(&format!("{label} passed"));
    Ok(())
}
