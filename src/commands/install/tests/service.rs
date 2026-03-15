use super::*;

#[test]
fn service_setup_skips_when_flag_disabled() {
    let tmp = setup_services_root();
    let root = tmp.path();
    let ctx = test_context(root);
    let args = install_args_template();

    let calls = Arc::new(AtomicUsize::new(0));
    maybe_setup_service_with("ripgrep", &args, &ctx, |_prompt| {
        calls.fetch_add(1, Ordering::SeqCst);
        CommandOutcome {
            success: true,
            output: String::new(),
        }
    });

    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn service_setup_dry_run_reports_without_edit_call() {
    let tmp = setup_services_root();
    let root = tmp.path();
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.service = true;
    args.flow.dry_run = true;

    let calls = Arc::new(AtomicUsize::new(0));
    maybe_setup_service_with("ripgrep", &args, &ctx, |_prompt| {
        calls.fetch_add(1, Ordering::SeqCst);
        CommandOutcome {
            success: true,
            output: String::new(),
        }
    });

    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn service_setup_calls_editor_with_services_target() {
    let tmp = setup_services_root();
    let root = tmp.path();
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.service = true;

    let calls = Arc::new(AtomicUsize::new(0));
    let prompt = Arc::new(Mutex::new(String::new()));
    maybe_setup_service_with("ripgrep", &args, &ctx, |edit_prompt| {
        calls.fetch_add(1, Ordering::SeqCst);
        *prompt.lock().expect("prompt lock should succeed") = edit_prompt.to_string();
        CommandOutcome {
            success: true,
            output: String::new(),
        }
    });

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let captured = prompt.lock().expect("prompt lock should succeed");
    assert!(captured.contains("launchd agent for ripgrep"));
    assert!(captured.contains("home/services.nix"));
}
