use super::*;

fn rules(src: &str) -> Vec<(usize, char)> {
    scan("t.rs", src, true).iter().map(|h| (h.line, h.rule)).collect()
}

fn wrap(body: &str) -> String {
    format!("#[test]\nfn t() {{\n{body}\n}}\n")
}

#[test]
fn rule_a_flags_short_millisecond_and_any_microsecond_bounds() {
    assert_eq!(rules(&wrap("    assert!(took < Duration::from_millis(50));")), vec![(3, 'a')]);
    assert_eq!(rules(&wrap("    assert!(took <= Duration::from_micros(900));")), vec![(3, 'a')]);
    assert_eq!(rules(&wrap("    assert!(took < Duration::from_millis(250));")), vec![]);
    assert_eq!(rules(&wrap("    assert!(took < Duration::from_millis(100));")), vec![]);
    // Lower bounds cannot flake on a slow runner.
    assert_eq!(rules(&wrap("    assert!(waited >= Duration::from_millis(60));")), vec![]);
    assert_eq!(
        rules(&wrap(
            "    // timing: value a computed repaint interval
    assert!(delay <= Duration::from_millis(34));"
        )),
        vec![]
    );
}

#[test]
fn rule_a_ignores_sleeps_and_timeouts_without_a_comparison() {
    assert_eq!(rules(&wrap("    until(Duration::from_millis(5), || ok());\n    recv_timeout(Duration::from_millis(20));")), vec![]);
}

#[test]
fn markers_on_the_line_or_the_line_above_need_a_reason() {
    let ok_above = wrap("    // timing: stall-bound a descheduled thread, not a latency\n    assert!(took < Duration::from_millis(50));");
    assert_eq!(rules(&ok_above), vec![]);
    let ok_same = wrap("    assert!(took < Duration::from_millis(50)); // timing: median of 25 against a 15 ms tick");
    assert_eq!(rules(&ok_same), vec![]);
    let no_reason = wrap("    // timing: stall-bound\n    assert!(took < Duration::from_millis(50));");
    assert_eq!(rules(&no_reason), vec![(4, 'a')]);
    let too_far = wrap("    // timing: stall-bound reason\n    let x = 1;\n    assert!(took < Duration::from_millis(50));");
    assert_eq!(rules(&too_far), vec![(5, 'a')]);
}

#[test]
fn rule_b_flags_sleep_paced_measurements_only() {
    let paced = wrap("    thread::sleep(Duration::from_millis(500));\n    assert!(start.elapsed() > limit);");
    assert_eq!(rules(&paced), vec![(3, 'b')]);
    let rate = wrap("    std::thread::sleep(d);\n    assert_eq!(measured_rate, 10);");
    assert_eq!(rules(&rate), vec![(3, 'b')]);
    let counted = wrap("    let mut n = 0;\n    loop { n += 1; thread::sleep(d); }\n    assert!(n >= 2);");
    assert_eq!(rules(&counted), vec![(4, 'b')]);
    let plain = wrap("    thread::sleep(d);\n    assert_eq!(sample_rate, 48000);");
    assert_eq!(rules(&plain), vec![]);
    let marked = wrap("    // timing: sleep-ok negative check, nothing may arrive\n    thread::sleep(d);\n    assert!(start.elapsed() > limit);");
    assert_eq!(rules(&marked), vec![]);
}

#[test]
fn rule_c_flags_throughput_floors() {
    assert_eq!(rules(&wrap("    assert!(frames >= 10);")), vec![(3, 'c')]);
    assert_eq!(rules(&wrap("    assert!(20 <= blocks.len(), \"blocks\");")), vec![(3, 'c')]);
    assert_eq!(rules(&wrap("    assert!(frames >= 2);")), vec![]);
    assert_eq!(rules(&wrap("    assert!(total >= 10);")), vec![]);
    assert_eq!(rules(&wrap("    assert!(m.spread_frames <= 1 && m.found >= 6);")), vec![]);
    let counter = wrap("    let mut seen = 0;\n    while Instant::now() < end { seen += 1; }\n    assert!(seen >= 5);");
    assert_eq!(rules(&counter), vec![(5, 'c')]);
    let marked = wrap("    // timing: floor-ok the loop is bounded by the count, not the clock\n    assert!(laps >= 3);");
    assert_eq!(rules(&marked), vec![]);
}

#[test]
fn ignored_functions_are_skipped() {
    let src = "#[test]\n#[ignore = \"hardware\"]\nfn hw() {\n    assert!(took < Duration::from_millis(2));\n}\n#[test]\nfn live() {\n    assert!(took < Duration::from_millis(2));\n}\n";
    assert_eq!(rules(src), vec![(8, 'a')]);
}

#[test]
fn only_test_code_is_scanned() {
    let src = "fn product() {\n    assert!(took < Duration::from_millis(2));\n}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        assert!(took < Duration::from_millis(2));\n    }\n}\nfn after() {\n    assert!(took < Duration::from_millis(2));\n}\n";
    assert_eq!(scan("lib.rs", src, false).iter().map(|h| h.line).collect::<Vec<_>>(), vec![8]);
}

#[test]
fn test_files_are_recognized_by_path() {
    assert!(is_test_file("crates/a/tests/x.rs"));
    assert!(is_test_file("crates/a/src/b/tests.rs"));
    assert!(is_test_file("crates/a/src/stream_tests.rs"));
    assert!(!is_test_file("crates/a/src/lib.rs"));
}

#[test]
fn comments_and_strings_do_not_trigger_rules() {
    assert_eq!(rules(&wrap("    // assert!(frames >= 10);\n    let s = \"assert!(frames >= 10)\";")), vec![]);
}
