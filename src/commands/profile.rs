use crate::cli::ProfileArgs;
use crate::infra::timing::{
    read_recent_timings, timing_detail_lines, timing_summary_line, timings_path,
};
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
        Printer::body(&timing_summary_line(record));
        for line in timing_detail_lines(record) {
            Printer::sub_detail(&line);
        }
    }

    println!();
    Printer::detail(&format!("Source: {}", timings_path().display()));
    0
}
