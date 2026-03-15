use super::*;

#[test]
fn parse_source_choice_empty_defaults_to_first() {
    assert_eq!(parse_source_choice("", 3), Some(0));
    assert_eq!(parse_source_choice("   ", 3), Some(0));
}

#[test]
fn parse_source_choice_accepts_valid_number() {
    assert_eq!(parse_source_choice("2", 3), Some(1));
}

#[test]
fn parse_source_choice_rejects_cancel_and_invalid() {
    assert_eq!(parse_source_choice("n", 3), None);
    assert_eq!(parse_source_choice("no", 3), None);
    assert_eq!(parse_source_choice("0", 3), None);
    assert_eq!(parse_source_choice("9", 3), None);
    assert_eq!(parse_source_choice("abc", 3), None);
}

#[test]
fn select_candidate_index_yes_bypasses_prompts() {
    let mut args = install_args_template();
    args.flow.yes = true;

    let mut confirm_calls = 0usize;
    let mut prompt_calls = 0usize;

    let selection = select_candidate_index(
        &args,
        3,
        || {
            confirm_calls += 1;
            true
        },
        |_| {
            prompt_calls += 1;
            Some(2)
        },
    );

    assert_eq!(selection, CandidateSelection::Selected(0));
    assert_eq!(confirm_calls, 0);
    assert_eq!(prompt_calls, 0);
}

#[test]
fn select_candidate_index_dry_run_bypasses_prompts() {
    let mut args = install_args_template();
    args.flow.dry_run = true;

    let mut confirm_calls = 0usize;
    let mut prompt_calls = 0usize;

    let selection = select_candidate_index(
        &args,
        2,
        || {
            confirm_calls += 1;
            true
        },
        |_| {
            prompt_calls += 1;
            Some(1)
        },
    );

    assert_eq!(selection, CandidateSelection::Selected(0));
    assert_eq!(confirm_calls, 0);
    assert_eq!(prompt_calls, 0);
}

#[test]
fn select_candidate_index_single_requires_confirmation() {
    let args = install_args_template();

    let mut confirm_calls = 0usize;
    let mut prompt_calls = 0usize;
    let declined = select_candidate_index(
        &args,
        1,
        || {
            confirm_calls += 1;
            false
        },
        |_| {
            prompt_calls += 1;
            Some(0)
        },
    );

    assert_eq!(declined, CandidateSelection::Skipped);
    assert_eq!(confirm_calls, 1);
    assert_eq!(prompt_calls, 0);
}

#[test]
fn select_candidate_index_multi_uses_numbered_prompt() {
    let args = install_args_template();

    let mut confirm_calls = 0usize;
    let mut prompt_calls = 0usize;
    let selected = select_candidate_index(
        &args,
        3,
        || {
            confirm_calls += 1;
            true
        },
        |_| {
            prompt_calls += 1;
            Some(2)
        },
    );

    let skipped = select_candidate_index(
        &args,
        3,
        || {
            confirm_calls += 1;
            true
        },
        |_| {
            prompt_calls += 1;
            None
        },
    );

    assert_eq!(selected, CandidateSelection::Selected(2));
    assert_eq!(skipped, CandidateSelection::Skipped);
    assert_eq!(confirm_calls, 0);
    assert_eq!(prompt_calls, 2);
}
