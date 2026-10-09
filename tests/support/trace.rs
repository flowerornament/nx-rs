use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Row {
    version: u8,
    event: String,
    id: u64,
    parent: Option<u64>,
    offset_ns: u128,
    label: String,
    code: Option<i32>,
}

pub fn assert_trace(path: &Path, expected_exit: i32) -> Result<(), Box<dyn Error>> {
    let events: Vec<Row> = fs::read_to_string(path)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert_eq!(
        events.first().map(|event| event.event.as_str()),
        Some("run-start")
    );
    assert_eq!(
        events.last().map(|event| event.event.as_str()),
        Some("run-end")
    );
    assert_eq!(
        events.last().and_then(|event| event.code),
        Some(expected_exit)
    );
    assert!(events.iter().all(|event| event.version == 1));
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].offset_ns <= pair[1].offset_ns)
    );
    let mut open = BTreeMap::new();
    for event in &events {
        if event.event == "nix-activity" {
            assert!(open.contains_key(&event.parent.expect("Nix activity needs command parent")));
            continue;
        }
        if event.event.ends_with("-start") {
            if let Some(parent) = event.parent {
                assert!(open.contains_key(&parent), "orphan {event:?}");
            }
            assert!(
                open.insert(event.id, event).is_none(),
                "duplicate {event:?}"
            );
        } else {
            let start = open.remove(&event.id).expect("end needs matching start");
            assert_eq!(
                start.label.split_whitespace().next(),
                event.label.split_whitespace().next()
            );
            assert_eq!(start.parent, event.parent);
            assert_eq!(
                start.event.trim_end_matches("start"),
                event.event.trim_end_matches("end")
            );
            assert!(event.code.is_some(), "unfinished {event:?}");
        }
    }
    assert!(open.is_empty());
    Ok(())
}

pub fn retain(
    case_id: &str,
    trace: &Path,
    timings: &Path,
    invocations: &Path,
    measurement: &serde_json::Value,
) -> Result<(), Box<dyn Error>> {
    if let Some(dir) = std::env::var_os("NX_PERF_DIR") {
        let dir = Path::new(&dir);
        fs::create_dir_all(dir)?;
        if measurement["traced"] == true {
            fs::copy(trace, dir.join(format!("{case_id}.trace.jsonl")))?;
        }
        fs::write(
            dir.join(format!("{case_id}.measurement.json")),
            serde_json::to_vec_pretty(measurement)?,
        )?;
        if timings.exists() {
            fs::copy(timings, dir.join(format!("{case_id}.timings.jsonl")))?;
        }
        let mut sanitized = String::new();
        for line in fs::read_to_string(invocations)?.lines() {
            let mut row: serde_json::Value = serde_json::from_str(line)?;
            if let Some(env) = row["env"].as_array_mut() {
                for pair in env {
                    pair[1] = "<redacted>".into();
                }
            }
            sanitized.push_str(&serde_json::to_string(&row)?);
            sanitized.push('\n');
        }
        fs::write(dir.join(format!("{case_id}.commands.jsonl")), sanitized)?;
    }
    Ok(())
}
