use super::*;

#[test]
fn post_install_runs_rebuild_when_requested() {
    let tmp = temp_root();
    let ctx = test_context(tmp.path());
    let mut args = install_args_template();
    args.flow.rebuild = true;

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);
    run_post_install_actions(1, &args, &ctx, move || {
        calls_clone.fetch_add(1, Ordering::SeqCst);
        0
    });

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn post_install_skips_rebuild_without_flag() {
    let tmp = temp_root();
    let ctx = test_context(tmp.path());
    let args = install_args_template();

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);
    run_post_install_actions(1, &args, &ctx, move || {
        calls_clone.fetch_add(1, Ordering::SeqCst);
        0
    });

    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn post_install_skips_rebuild_in_dry_run() {
    let tmp = temp_root();
    let ctx = test_context(tmp.path());
    let mut args = install_args_template();
    args.flow.rebuild = true;
    args.flow.dry_run = true;

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);
    run_post_install_actions(1, &args, &ctx, move || {
        calls_clone.fetch_add(1, Ordering::SeqCst);
        0
    });

    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn post_install_skips_rebuild_when_nothing_installed() {
    let tmp = temp_root();
    let ctx = test_context(tmp.path());
    let mut args = install_args_template();
    args.flow.rebuild = true;

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);
    run_post_install_actions(0, &args, &ctx, move || {
        calls_clone.fetch_add(1, Ordering::SeqCst);
        0
    });

    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
