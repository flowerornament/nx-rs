use crate::cli::ProfileArgs;
use crate::infra::timing::{read_recent_timings, short_hash, timings_path};
use crate::output::printer::Printer;

pub fn cmd_profile(args: &ProfileArgs) -> i32 {
    let records = match read_recent_timings(args.limit) {
        Ok(records) => records,
        Err(err) => {
            eprintln!("{err:#}");
            return 1;
        }
    };

    if args.json {
        match serde_json::to_string_pretty(&records) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("failed to render profile JSON: {err}");
                return 1;
            }
        }
        return 0;
    }

    if records.is_empty() {
        Printer::detail(&format!(
            "No rebuild timings recorded yet at {}",
            timings_path().display()
        ));
        return 0;
    }

    Printer::heading(&format!("Recent Rebuild Timings ({})", records.len()));
    for record in records.iter().rev() {
        println!();
        Printer::body(&format!(
            "{} {}ms ({})",
            record.command, record.total_ms, record.status
        ));
        if let Some(head) = &record.repo_head {
            Printer::sub_detail(&format!("git: {}", short_hash(head)));
        }
        if let Some(hash) = &record.flake_lock_hash {
            Printer::sub_detail(&format!("flake.lock: {hash}"));
        }
        for phase in &record.phases {
            Printer::sub_detail(&format!(
                "{}: {}ms ({})",
                phase.name, phase.duration_ms, phase.status
            ));
        }
    }

    println!();
    Printer::detail(&format!("Source: {}", timings_path().display()));
    0
}
