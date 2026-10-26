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

/// E49 recurrence: `crates/mix/tests/render_pool.rs` held its 50 ms bound in a `const`.
#[test]
fn rule_a_follows_a_name_bound_to_a_short_duration() {
    let src = "const BOUND: Duration = Duration::from_millis(50);\n#[test]\nfn t() {\n    assert!(worst < BOUND);\n}\n";
    assert_eq!(rules(src), vec![(4, 'a')]);
    assert_eq!(rules(&wrap("    let limit = Duration::from_micros(800);\n    assert!(took <= limit);")), vec![(4, 'a')]);
    assert_eq!(rules(&wrap("    let mut cap = Duration::from_millis(20);\n    assert!(cap > took);")), vec![(4, 'a')]);
    // A long bound, a short name used as a sleep or a timeout, a duration passed to a call, or a
    // marked binding is not an unmarked measurement ceiling.
    let long = "const STALL: Duration = Duration::from_millis(250);\n#[test]\nfn t() {\n    assert!(worst < STALL);\n}\n";
    assert_eq!(rules(long), vec![]);
    assert_eq!(rules(&wrap("    let tick = Duration::from_millis(5);\n    std::thread::sleep(tick);\n    rx.recv_timeout(tick);")), vec![]);
    assert_eq!(rules(&wrap("    let (mut rig, port) = slow(Duration::from_millis(50));\n    assert!(rig.count > 0);")), vec![]);
    let marked = wrap(
        "    // timing: stall-bound a stuck thread, not a latency
    let bound = Duration::from_millis(2);
    assert!(took < bound);",
    );
    assert_eq!(rules(&marked), vec![]);
    // A median is a latency claim: no marker admits it in a gated test (E49, #110); an ignored test may hold it.
    let median = wrap(
        "    // timing: median of 25 against a 15 ms tick
    let bound = Duration::from_millis(2);
    assert!(median < bound);",
    );
    assert_eq!(rules(&median), vec![(5, 'a')]);
    let ignored = "#[test]
#[ignore = \"hardware\"]
fn t() {
    let bound = Duration::from_millis(2);
    assert!(median < bound);
}
";
    assert_eq!(rules(ignored), vec![]);
}

#[test]
fn rule_a_ignores_sleeps_and_timeouts_without_a_comparison() {
    assert_eq!(rules(&wrap("    until(Duration::from_millis(5), || ok());\n    recv_timeout(Duration::from_millis(20));")), vec![]);
}

#[test]
fn markers_on_the_line_or_the_line_above_need_a_reason() {
    let ok_above = wrap("    // timing: stall-bound a descheduled thread, not a latency\n    assert!(took < Duration::from_millis(50));");
    assert_eq!(rules(&ok_above), vec![]);
    let ok_same = wrap("    assert!(took < Duration::from_millis(50)); // timing: stall-bound a descheduled thread, not a latency");
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

fn loop_lines(src: &str) -> Vec<usize> {
    loops::scan("t.rs", src, true).iter().map(|h| h.line).collect()
}

#[test]
fn rule_d_flags_each_pacing_call_and_loop_form() {
    for pace in ["sleep(tick)", "std::thread::sleep(tick)", "thread::park_timeout(tick)", "thread::yield_now()"] {
        for body in [
            format!("while start.elapsed() < Duration::from_millis(100) {{ {pace}; }}"),
            format!("loop {{ if start.elapsed() >= Duration::from_millis(100) {{ break; }} {pace}; }}"),
            format!("for _ in 0..100 {{ assert!(start.elapsed() < Duration::from_millis(100)); {pace}; }}"),
        ] {
            assert_eq!(loop_lines(&wrap(&body)), vec![3], "{body}");
        }
    }
}

#[test]
fn rule_d_accepts_injected_clocks_but_not_an_unused_clock_parameter() {
    let src = "fn t(clock: &TestClock) {
        let deadline = clock.now() + Duration::from_millis(100);
        while clock.now() < deadline { thread::yield_now(); }
    }";
    assert!(loop_lines(src).is_empty());
    assert!(
        loop_lines(
            "fn t(now: impl Fn() -> Instant) {
        let origin = Instant::now();
        let deadline = origin + Duration::from_millis(100);
        while now() < deadline { thread::yield_now(); }
    }"
        )
        .is_empty()
    );
    assert!(
        loop_lines(
            "fn t(now: impl Fn() -> Instant) {
        let deadline = now() + Duration::from_millis(100);
        while now() < deadline { thread::sleep(tick); }
    }"
        )
        .is_empty()
    );
    assert_eq!(
        loop_lines(
            "fn t(clock: &TestClock) {
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline { thread::yield_now(); }
    }"
        ),
        vec![3]
    );
    assert_eq!(
        loop_lines(
            "fn t(clock: &TestClock) {
        let start = clock.now();
        while start.elapsed() < Duration::from_millis(100) { thread::yield_now(); }
    }"
        ),
        vec![3]
    );
}

#[test]
fn rule_d_requires_a_stall_marker_with_a_reason_on_the_loop() {
    let body = "while start.elapsed() < Duration::from_millis(100) { thread::yield_now(); }";
    assert!(loop_lines(&wrap(&format!("// timing: stall-bound deliberate negative observation window\n{body}"))).is_empty());
    assert!(loop_lines(&wrap(&format!("{body} // timing: stall-bound deliberate negative observation window"))).is_empty());
    for marker in ["// timing: stall-bound", "// timing: sleep-ok reason", "// timing: stall-boundish reason"] {
        assert_eq!(loop_lines(&wrap(&format!("{marker}\n{body}"))), vec![4]);
    }
    assert_eq!(loop_lines(&wrap(&format!("// timing: stall-bound reason\nlet unrelated = 1;\n{body}"))), vec![5]);
    assert_eq!(
        loop_lines(&wrap(
            "// timing: stall-bound reason
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline { thread::yield_now(); }"
        )),
        vec![5]
    );
}

#[test]
fn rule_d_distinguishes_poll_intervals_from_exit_budgets() {
    assert_eq!(
        loop_lines(&wrap(
            "let host = host(Duration::from_millis(100));
        let deadline = Instant::now() + Duration::from_secs(5);
        while host.wedged() > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }"
        )),
        vec![5]
    );
    for limit in ["Duration::from_secs(2)", "Duration::from_millis(250)", "Duration::from_millis(2_000)"] {
        assert!(
            loop_lines(&wrap(&format!(
                "let deadline = Instant::now() + {limit};
            while !ready() {{
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(5));
            }}"
            )))
            .is_empty()
        );
    }
    assert!(loop_lines(&wrap("for _ in 0..10 { thread::sleep(Duration::from_millis(5)); }")).is_empty());
    assert!(
        loop_lines(&wrap(
            "while !ready() {
        if start.elapsed() < Duration::from_millis(100) { log(); }
        thread::sleep(Duration::from_millis(5));
    }"
        ))
        .is_empty()
    );
}

#[test]
fn rule_d_catches_the_async_import_short_ttl_hidden_behind_a_call() {
    for ttl in ["Duration::from_millis(100)", "TTL"] {
        let setup = format!(
            "const TTL: Duration = Duration::from_millis(100);\nfn t() {{\n    imports.set_limits(32, {ttl});\n    let deadline = Instant::now() + Duration::from_millis(400);"
        );
        let polling = "while imports.contains(id) && Instant::now() < deadline {\n        imports.service();\n        thread::sleep(Duration::from_millis(5));\n    }";
        assert_eq!(loop_lines(&format!("{setup}\n{polling}\n}}")), vec![5]);
        let injected = polling.replace("imports.service()", "imports.service_at(clock.now())");
        assert!(loop_lines(&format!("{setup}\n{injected}\n}}")).is_empty());
        assert!(loop_lines(&format!("{setup}\n// timing: stall-bound expiry is injected by the test harness\n{polling}\n}}")).is_empty());
        assert!(loop_lines(&format!("{setup}\n}}")).is_empty());
    }
}

#[test]
fn rule_d_keeps_short_call_arguments_inside_their_function_or_closure() {
    let short = "set_limits(Duration::from_millis(100));";
    let polling = "while !ready() { thread::sleep(Duration::from_millis(5)); }";
    for source in [
        format!("fn a() {{ {short} }} fn b() {{ {polling} }}"),
        wrap(&format!("let callback = || {{ {short} }}; {polling}")),
        wrap(&format!("{short} let callback = || {{ {polling} }};")),
        wrap(&format!("fn nested() {{ {short} }} {polling}")),
        wrap(&format!("{short} fn nested() {{ {polling} }}")),
    ] {
        assert!(loop_lines(&source).is_empty(), "{source}");
    }
    assert_eq!(loop_lines(&wrap(&format!("let callback = || {{ {short} {polling} }};"))), vec![3]);
    assert_eq!(loop_lines(&wrap(&format!("{polling} {short}"))), vec![3]);
    assert_eq!(loop_lines(&format!("fn t(clock: &Clock) {{ {short} {polling} }}")), vec![1]);
    for long in ["Duration::from_millis(250)", "Duration::from_micros(500_000)", "Duration::from_millis(100) * 20"] {
        assert!(loop_lines(&wrap(&format!("configure({long}); {polling}"))).is_empty());
    }
}

#[test]
fn rule_d_follows_flag_variable_exits() {
    for guard in ["start.elapsed() > Duration::from_millis(100)", "start.elapsed().as_millis() > 100"] {
        let source = wrap(&format!(
            "let mut done = false;\nwhile !done {{\n    if {guard} {{ done = true; }}\n    thread::sleep(Duration::from_millis(5));\n}}"
        ));
        assert_eq!(loop_lines(&source), vec![4]);
    }
    let injected = "let start = clock.now();\nlet mut done = false;\nwhile !done {\n    if clock.now() - start > Duration::from_millis(100) { done = true; }\n    thread::sleep(Duration::from_millis(5));\n}";
    assert!(loop_lines(&wrap(injected)).is_empty());
    let unrelated =
        "while !done {\n    if start.elapsed() > Duration::from_millis(100) { diagnostic = true; }\n    thread::sleep(Duration::from_millis(5));\n}";
    assert!(loop_lines(&wrap(unrelated)).is_empty());
    assert_eq!(loop_lines(&wrap("while !done { done = start.elapsed() > Duration::from_millis(100); thread::yield_now(); }")), vec![3]);
    assert!(
        loop_lines(&wrap("for _ in 0..count { if start.elapsed() > Duration::from_millis(100) { count = 0; } thread::yield_now(); }")).is_empty()
    );
    assert_eq!(
        loop_lines(&wrap(
            "while keep_polling {\nmatch state {\n    Waiting if start.elapsed() > Duration::from_millis(100) => keep_polling = false,\n    _ => {},\n}\nthread::yield_now();\n}"
        )),
        vec![3]
    );
}

#[test]
fn rule_d_accepts_passed_timestamps_without_excusing_real_short_exits() {
    for parameter in ["now: Instant", "now: impl Fn() -> Instant"] {
        let argument = if parameter.contains("Fn") { "now()" } else { "now" };
        let source = format!(
            "fn t({parameter}) {{\n    set_limits(Duration::from_millis(100));\n    while !ready() {{\n        service_at({argument});\n        thread::sleep(Duration::from_millis(5));\n    }}\n}}"
        );
        assert!(loop_lines(&source).is_empty());
        let real_exit = source.replace("while !ready()", "while start.elapsed() < Duration::from_millis(100)");
        assert_eq!(loop_lines(&real_exit), vec![3]);
    }
    let timestamp = "let now = Instant::now();\nset_limits(Duration::from_millis(100));\nwhile !ready() { service_at(now); thread::yield_now(); }";
    assert!(loop_lines(&wrap(timestamp)).is_empty());
    let real_read = timestamp.replace("service_at(now)", "service_at(Instant::now())");
    assert_eq!(loop_lines(&wrap(&real_read)), vec![5]);
}

#[test]
fn rule_d_tracks_elapsed_numeric_units_and_thresholds() {
    for (unit, short, long) in [("as_millis", "100", "250"), ("as_micros", "100_000", "250_000"), ("as_secs_f64", "0.1", "0.25")] {
        for comparison in [format!("start.elapsed().{unit}() > {short}"), format!("{short} <= start.elapsed().{unit}()")] {
            assert_eq!(loop_lines(&wrap(&format!("loop {{ if {comparison} {{ break; }} thread::yield_now(); }}"))), vec![3]);
        }
        assert!(loop_lines(&wrap(&format!("while start.elapsed().{unit}() < {long} {{ thread::yield_now(); }}"))).is_empty());
    }
    assert!(loop_lines(&wrap("while progress.as_millis() < 100 { thread::yield_now(); }")).is_empty());
}

#[test]
fn rule_d_resolves_multiline_deadlines_and_same_file_constants() {
    let src = "const WINDOW: Duration = SHORT;
    const SHORT: Duration = Duration::from_millis(MS);
    const MS: u64 = 100;
    fn t() {
        let deadline = Instant::now()
            + WINDOW;
        while Instant::now()
            < deadline
        {
            thread::park_timeout(Duration::from_millis(5));
        }
    }";
    assert_eq!(loop_lines(src), vec![7]);
    for duration in ["Duration::from_millis(249u64)", "Duration::from_micros(500)", "Duration::from_nanos(500)"] {
        assert_eq!(
            loop_lines(&wrap(&format!(
                "let window = {duration};
            loop {{ if start.elapsed() > window {{ return; }} thread::yield_now(); }}"
            ))),
            vec![4]
        );
    }
}

#[test]
fn rule_d_limits_bindings_and_test_scope() {
    let src = "fn product() {
        while start.elapsed() < Duration::from_millis(100) { thread::yield_now(); }
    }
    #[cfg(test)]
    mod tests {
        fn t() {
            while start.elapsed() < Duration::from_millis(100) { thread::yield_now(); }
        }
    }
    fn after() {
        while start.elapsed() < Duration::from_millis(100) { thread::yield_now(); }
    }";
    assert_eq!(loops::scan("lib.rs", src, false).iter().map(|h| h.line).collect::<Vec<_>>(), vec![7]);
    assert!(
        loop_lines(
            "const WINDOW: Duration = Duration::from_millis(100);
        fn t() {
            let WINDOW = Duration::from_secs(2);
            while start.elapsed() < WINDOW { thread::yield_now(); }
        }"
        )
        .is_empty()
    );
    assert!(
        loop_lines(
            "fn a() { let window = Duration::from_millis(100); }
        fn b() { while start.elapsed() < window { thread::yield_now(); } }"
        )
        .is_empty()
    );
}

#[test]
fn rule_d_does_not_read_comments_strings_or_unexecuted_closures_as_loops() {
    assert!(
        loop_lines(&wrap(
            r##"
        /* while start.elapsed() < Duration::from_millis(100) { thread::yield_now(); } */
        let text = r#"while start.elapsed() < Duration::from_millis(100) { thread::yield_now(); }"#;
        while start.elapsed() < Duration::from_millis(100) {
            let callback = || { thread::sleep(Duration::from_millis(1)); };
        }
    "##
        ))
        .is_empty()
    );
}

#[test]
fn rule_d_reports_the_inner_deadline_without_tainting_the_outer_loop() {
    assert_eq!(
        loop_lines(&wrap(
            "for _ in 0..5 {
        while start.elapsed() < Duration::from_millis(100) { thread::yield_now(); }
    }"
        )),
        vec![4]
    );
}

#[test]
fn rule_d_follows_qualified_and_associated_constants() {
    let src = "const WINDOW: Duration = Duration::from_millis(100);
        mod limits { pub const WAIT: Duration = super::WINDOW; }
        #[cfg(test)] mod tests {
            const WINDOW: Duration = Duration::from_secs(2);
            struct Limits;
            impl Limits {
                const MS: u64 = 100;
                const WAIT: Duration = Duration::from_millis(Self::MS);
                fn wait() {
                    while start.elapsed() < Self::WAIT { thread::yield_now(); }
                }
            }
            fn t() {
                while start.elapsed() < super::WINDOW { thread::yield_now(); }
                while start.elapsed() < self::WINDOW { thread::yield_now(); }
                while start.elapsed() < super::limits::WAIT { thread::yield_now(); }
                while start.elapsed() < Limits::WAIT { thread::yield_now(); }
                while start.elapsed() < crate::WINDOW { thread::yield_now(); }
            }
        }";
    assert_eq!(loops::scan("lib.rs", src, false).iter().map(|h| h.line).collect::<Vec<_>>(), vec![10, 14, 16, 17, 18]);
}

#[test]
fn rule_d_checks_a_clock_sample_refreshed_in_the_loop() {
    assert_eq!(
        loop_lines(&wrap(
            "let deadline = Instant::now() + Duration::from_millis(100);
        let mut now = Instant::now();
        while now < deadline {
            thread::yield_now();
            now = Instant::now();
        }"
        )),
        vec![5]
    );
    assert_eq!(
        loop_lines(&wrap(
            "loop {
        let now = Instant::now();
        if now > start + Duration::from_millis(100) { break; }
        thread::yield_now();
    }"
        )),
        vec![3]
    );
}

#[test]
fn rule_d_uses_the_composed_duration_not_its_short_components() {
    assert!(
        loop_lines(&wrap(
            "let start = Instant::now();
        while Instant::now() - start < Duration::from_secs(5) { thread::yield_now(); }"
        ))
        .is_empty()
    );
    for budget in ["Duration::from_secs(5) + Duration::from_millis(100)", "Duration::from_millis(100) * 20"] {
        assert!(
            loop_lines(&wrap(&format!(
                "let budget = {budget};
            while start.elapsed() < budget {{ thread::yield_now(); }}"
            )))
            .is_empty()
        );
        assert!(
            loop_lines(&wrap(&format!(
                "let deadline = Instant::now() + {budget};
            while Instant::now() < deadline {{ thread::yield_now(); }}"
            )))
            .is_empty()
        );
    }
}

#[test]
fn rule_d_treats_fine_units_consistently_in_deadlines_and_elapsed_comparisons() {
    for duration in ["Duration::from_micros(500_000)", "Duration::from_nanos(500_000_000)"] {
        assert_eq!(loop_lines(&wrap(&format!("while start.elapsed() < {duration} {{ thread::yield_now(); }}"))), vec![3]);
        assert_eq!(
            loop_lines(&wrap(&format!(
                "let deadline = Instant::now() + {duration};
            while Instant::now() < deadline {{ thread::yield_now(); }}"
            ))),
            vec![4]
        );
    }
}

#[test]
fn rule_d_is_wired_into_the_public_scan_and_diagnostic_format() {
    let hits = scan("crates/example/tests/poll.rs", &wrap("while start.elapsed() < Duration::from_millis(100) { thread::yield_now(); }"), true);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].rule, 'd');
    assert!(hits[0].to_string().starts_with("crates/example/tests/poll.rs:3: rule (d): sleep/park/yield loop"));
}

#[test]
fn rule_d_checks_match_guards_and_panicking_timeout_branches() {
    assert!(
        loop_lines(&wrap(
            "loop {
        match state {
            Ready => break,
            Waiting if start.elapsed() < Duration::from_millis(100) => log(),
            Waiting => {},
        }
        thread::yield_now();
    }"
        ))
        .is_empty()
    );
    assert_eq!(
        loop_lines(&wrap(
            "loop {
        if start.elapsed() > Duration::from_millis(100) { panic!(\"timed out\"); }
        thread::yield_now();
    }"
        )),
        vec![3]
    );
    assert_eq!(
        loop_lines(&wrap(
            "let deadline = Instant::now() + Duration::from_millis(100);
        loop {
            match poll() {
                Ready => break,
                Waiting if Instant::now() < deadline => thread::sleep(tick),
                _ => panic!(\"timed out\"),
            }
        }"
        )),
        vec![4]
    );
}
