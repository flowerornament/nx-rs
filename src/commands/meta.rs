use serde::Serialize;

use crate::cli::VersionArgs;

#[derive(Debug, Serialize)]
struct VersionOutput<'a> {
    name: &'a str,
    package: &'a str,
    version: &'a str,
}

pub fn cmd_version(args: &VersionArgs) -> i32 {
    if args.json {
        let output = VersionOutput {
            name: "nx",
            package: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        };
        match serde_json::to_string_pretty(&output) {
            Ok(text) => {
                println!("{text}");
                0
            }
            Err(err) => {
                eprintln!("version json rendering failed: {err}");
                1
            }
        }
    } else {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        0
    }
}
