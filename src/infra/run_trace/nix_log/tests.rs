use super::*;
use crate::infra::{
    nix_output::NixOutputMode,
    shell::{command_path, run_nix_command_with_stdout},
};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

#[test]
fn native_capture_is_private_bounded_and_preserves_command_output() {
    let python = command_path("python3").expect("Python test dependency");
    for case in [
        "native",
        "verbose",
        "structured",
        "disabled",
        "unavailable",
        "held",
        "terminator",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let program = temp.path().join("nix");
        fs::write(&program, format!("#!{python}\n{FAKE_NIX}")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let trace = temp.path().join("trace.jsonl");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "infra::run_trace::nix_log::tests::child",
                "--ignored",
                "--nocapture",
            ])
            .env("NX_NIX_CAPTURE_TEST", case)
            .env("NX_NIX_CAPTURE_PROGRAM", &program)
            .env_remove("NX_TRACE_DIR")
            .env_remove("NX_TRACE_PATH");
        if case != "disabled" {
            command.env("NX_TRACE_PATH", &trace);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{case}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if case == "disabled" {
            assert!(!trace.exists());
            continue;
        }
        let text = fs::read_to_string(&trace).unwrap();
        assert!(!text.contains("private build output"));
        let rows = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let count = rows
            .iter()
            .filter(|row| row["event"] == "nix-activity")
            .count();
        assert_eq!(
            count,
            if case == "unavailable" { 0 } else { 2 },
            "{case}: {text}"
        );
        if case != "structured" {
            let capture = rows
                .iter()
                .find(|row| row["event"] == "command-end")
                .unwrap()["data"]["nix_capture"]
                .clone();
            assert_eq!(
                capture["complete"],
                !matches!(case, "held" | "unavailable"),
                "{case}: {text}"
            );
        }
        assert_eq!(rows.last().unwrap()["event"], "run-end");
        assert_eq!(rows.last().unwrap()["code"], 0);
    }
}

#[test]
fn routes_only_known_nix_invocations_and_bounds_records() {
    assert!(nix_argument_start("/bin/nix", &[]).is_some());
    assert!(nix_argument_start("sudo", &["-n", "/bin/darwin-rebuild", "switch"]).is_some());
    assert!(nix_argument_start("sudo", &["-n", "/bin/other"]).is_none());
    assert!(nix_argument_start("just", &["rebuild"]).is_none());
    assert_eq!(
        nix_argument_start(
            "sudo",
            &[
                "-H",
                "env",
                "HOME=/var/root",
                "NIX_REMOTE=daemon",
                "/bin/nix-env",
                "--set",
                "--"
            ]
        ),
        Some(5)
    );
    let mut record = Record::default();
    let mut summary = CaptureSummary {
        complete: true,
        ..CaptureSummary::default()
    };
    record.push(&vec![b'a'; RECORD_LIMIT + 1], &mut summary);
    assert!(!summary.complete);
    assert!(record.pending.is_empty());
    record.push(b"\n{}\n", &mut summary);
    assert!(!record.oversized && record.pending.is_empty());
}

#[test]
#[ignore = "subprocess entry point; never invokes the installed Nix"]
fn child() {
    let Ok(case) = std::env::var("NX_NIX_CAPTURE_TEST") else {
        return;
    };
    let trace = super::super::Run::from_env();
    let program = std::env::var("NX_NIX_CAPTURE_PROGRAM").unwrap();
    if case == "held" {
        let explicit = ["build", "--option", "json-log-path", "/user/log"];
        let mut preserved = Command::new(&program);
        assert!(
            Capture::configure(&mut preserved, &program, &explicit)
                .unwrap()
                .is_none()
        );
        assert_eq!(preserved.get_args().collect::<Vec<_>>(), explicit);
        let sudo_args = [
            "-H",
            "env",
            "HOME=/var/root",
            "NIX_REMOTE=daemon",
            "nix-env",
            "--set",
            "--",
        ];
        let mut wrapped = Command::new("sudo");
        let wrapper_capture = Capture::configure(&mut wrapped, "sudo", &sudo_args)
            .unwrap()
            .unwrap();
        let wrapped_args = wrapped.get_args().collect::<Vec<_>>();
        assert_eq!(&wrapped_args[..6], &sudo_args[..6]);
        assert_eq!(&wrapped_args[6..8], &["--option", "json-log-path"]);
        assert_eq!(*wrapped_args.last().unwrap(), "--");
        drop(wrapper_capture);
        let mut span = super::super::command("nix", &["build"]);
        let mut command = Command::new(&program);
        let capture = Capture::configure(&mut command, &program, &[])
            .unwrap()
            .unwrap();
        let path = command.get_args().last().unwrap();
        let mut producer = UnixStream::connect(path).unwrap();
        producer.write_all(b"{\"action\":\"start\",\"id\":7,\"name\":\"EvaluateFlakeDerivationOutput\",\"type\":10113}\n{\"action\":\"stop\",\"id\":7}\n").unwrap();
        let started = std::time::Instant::now();
        let summary = capture.finish();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!summary.complete);
        if let Some(span) = &mut span {
            span.nix_capture(summary);
            span.finish(0);
        }
        drop(span);
    } else {
        let mode = match case.as_str() {
            "structured" => NixOutputMode::Structured,
            "verbose" => NixOutputMode::NativeBarWithLogs,
            _ => NixOutputMode::NativeBar,
        };
        let mut args = vec!["--log-format", mode.log_format().as_arg()];
        if case == "terminator" {
            args.push("--");
        }
        let output = run_nix_command_with_stdout(&program, &args, None, None, mode).unwrap();
        assert_eq!(output.code, 7);
        assert_eq!(output.stdout.trim(), "result");
        if mode.is_native() {
            assert!(output.stderr.contains("native progress"));
        }
    }
    if let Some(trace) = trace {
        trace.finish(0);
    }
}

const FAKE_NIX: &str = r"import json, os, socket, sys
mode = os.environ['NX_NIX_CAPTURE_TEST']
rows = [dict(action='start',id=7,type=10113,name='EvaluateFlakeDerivationOutput',payload={'nix.installable':'repo#system'}),dict(action='result',id=7,type=101,fields=['private build output']),dict(action='stop',id=7)]
if mode == 'structured':
    assert not os.isatty(2)
    for row in rows: print('@nix '+json.dumps(row),file=sys.stderr)
else:
    assert os.isatty(2)
    os.write(2,b'\x1b[32mnative progress\x1b[0m\r\n')
    if mode == 'disabled': assert 'json-log-path' not in sys.argv
    elif mode != 'unavailable':
        index=sys.argv.index('json-log-path')
        assert sys.argv[index-1] == '--option'
        path=sys.argv[index+1]
        if mode == 'terminator': assert sys.argv[-1] == '--'
        assert os.stat(os.path.dirname(path)).st_mode & 0o077 == 0
        with socket.socket(socket.AF_UNIX) as s:
            s.connect(path)
            for row in rows:
                data=(json.dumps(row)+'\n').encode()
                s.sendall(data[:5]); s.sendall(data[5:])
print('result')
sys.exit(7)
";

#[test]
#[ignore = "explicit read-only live Nix smoke; NX_NIX_LIVE=1 and fresh NX_TRACE_PATH required"]
fn live_nix() {
    assert_eq!(std::env::var("NX_NIX_LIVE").as_deref(), Ok("1"));
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("probe"), "nx trace probe").unwrap();
    let expression = format!(
        "builtins.path {{ path = builtins.toPath {}; name = \"nx-trace-probe\"; }}",
        serde_json::to_string(source.path()).unwrap()
    );
    let trace = super::super::Run::from_env().unwrap();
    let started = std::time::Instant::now();
    let output = run_nix_command_with_stdout(
        "nix",
        &[
            "eval",
            "--impure",
            "--expr",
            &expression,
            "--raw",
            "--log-format",
            "bar",
        ],
        None,
        None,
        NixOutputMode::NativeBar,
    )
    .unwrap();
    assert_eq!(output.code, 0, "{}", output.stderr);
    trace.finish(0);
    let text = fs::read_to_string(std::env::var("NX_TRACE_PATH").unwrap()).unwrap();
    let rows = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let activities = rows
        .iter()
        .filter(|row| row["event"] == "nix-activity")
        .count();
    assert!(activities > 0, "{text}");
    assert!(
        rows.iter()
            .any(|row| row["data"]["nix_capture"]["complete"] == true),
        "{text}"
    );
    eprintln!(
        "Live native capture: {activities} activity events in {:.3}s",
        started.elapsed().as_secs_f64()
    );
}
