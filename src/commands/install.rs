use crate::cli::InstallArgs;
use crate::commands::context::AppContext;
use crate::commands::shared::relative_location;
use crate::infra::finder::find_package;

pub fn cmd_install(args: &InstallArgs, ctx: &AppContext) -> i32 {
    if args.packages.is_empty() {
        ctx.printer.error("No packages specified");
        return 1;
    }

    if args.dry_run {
        ctx.printer.dry_run_banner();
    }

    ctx.printer
        .action(&format!("Installing {}", args.packages[0]));

    for package in &args.packages {
        match find_package(package, &ctx.repo_root) {
            Ok(Some(location)) => {
                println!();
                ctx.printer.success(&format!(
                    "{package} already installed ({})",
                    relative_location(&location, &ctx.repo_root)
                ));
            }
            Ok(None) => {
                ctx.printer.error(&format!("{package} not found"));
                return 1;
            }
            Err(err) => {
                ctx.printer.error(&format!("install lookup failed: {err}"));
                return 1;
            }
        }
    }

    0
}
