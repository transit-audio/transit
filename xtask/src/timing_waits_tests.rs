use super::*;

const UI: &str = "crates/ui-studio/src/x_tests.rs";

fn lines(path: &str, src: &str) -> Vec<usize> {
    scan(path, src, true).iter().map(|h| h.line).collect()
}

const HELPER: &str = "fn until(mut ready: impl FnMut() -> bool) {\n    let deadline = Instant::now() + Duration::from_secs(10);\n    while !ready() {\n        assert!(Instant::now() < deadline);\n    }\n}\n";

#[test]
fn a_wall_clock_wait_helper_needs_a_reason() {
    assert_eq!(lines(UI, HELPER), vec![1]);
    let elapsed = "fn settle(&mut self) {\n    let t0 = Instant::now();\n    loop {\n        if t0.elapsed().as_secs() > 5 { break; }\n    }\n}\n";
    assert_eq!(lines(UI, elapsed), vec![1]);
    let marked = HELPER.replace("    while", "    // timing: stall-bound the device thread is the subject\n    while");
    assert_eq!(lines(UI, &marked), Vec::<usize>::new(), "a reason in the body");
    let above = format!("// timing: stall-bound the device thread is the subject\n{HELPER}");
    assert_eq!(lines(UI, &above), Vec::<usize>::new(), "a reason above the header");
    let bare = HELPER.replace("    while", "    // timing: stall-bound\n    while");
    assert_eq!(lines(UI, &bare), vec![1], "a marker with no reason");
}

#[test]
fn tests_ignored_fns_other_crates_and_non_test_code_are_not_wait_helpers() {
    assert_eq!(lines(UI, &format!("#[test]\n{HELPER}")), Vec::<usize>::new(), "a test's own loop is rules (b) and (d)");
    assert_eq!(lines(UI, &format!("#[ignore = \"hardware\"]\n{HELPER}")), Vec::<usize>::new());
    assert_eq!(lines("crates/mix/tests/t.rs", HELPER), Vec::<usize>::new());
    assert_eq!(scan(UI, HELPER, false), vec![], "not test code");
    let no_loop = "fn clock() -> Instant {\n    Instant::now()\n}\n";
    assert_eq!(lines(UI, no_loop), Vec::<usize>::new());
    let no_clock = "fn frames(n: usize) {\n    for _ in 0..n {\n        step();\n    }\n}\n";
    assert_eq!(lines(UI, no_clock), Vec::<usize>::new());
}

#[test]
fn a_listed_helper_passes_and_only_at_its_path() {
    let (path, name, reason) = EXCEPTIONS.iter().find(|(_, n, _)| *n == "until").copied().unwrap();
    assert!(!reason.is_empty());
    let helper = HELPER.replace("fn until(", &format!("fn {name}("));
    assert_eq!(lines(path, &helper), Vec::<usize>::new());
    assert_eq!(lines(UI, &helper), vec![1], "the same name elsewhere is not listed");
    for (path, name, reason) in EXCEPTIONS {
        assert!(path.starts_with("crates/") && !name.is_empty() && reason.len() > 10, "{path} {name}: a reason");
    }
}
