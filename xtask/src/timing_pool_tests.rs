use super::*;

const MIX: &str = "crates/mix/tests/t.rs";

/// The refusals rule (e) reports for `src` at `path`, as (line, refusal).
fn refusals(path: &str, src: &str) -> Vec<(usize, u8)> {
    scan(path, src, true).iter().map(|h| (h.line, h.text.as_bytes()["refusal ".len()] - b'0')).collect()
}

fn test_fn(body: &str) -> String {
    format!("#[test]\nfn t() {{\n{body}\n}}\n")
}

/// A test of an engine with render workers that renders, with `setup` before its blocks.
fn pool(setup: &str) -> String {
    test_fn(&format!(
        "    let mut eng = engine();\n    eng.start_render_workers(16);\n{setup}\n    eng.render_active(0, 512, &mut out);\n    assert!(peak > 0.01);"
    ))
}

#[test]
fn refusal_1_flags_a_pool_test_on_the_inherited_default_deadline() {
    assert_eq!(refusals(MIX, &pool("")), vec![(2, 1)], "E49 recurrence 2026-10-29: no deadline of its own");
    assert_eq!(refusals(MIX, &pool("    eng.set_strip_deadline(None);")), vec![(2, 1)], "None is the default share again");
    for ok in [
        "    eng.set_strip_deadline(Some(Duration::from_secs(60)));",
        "    eng.set_strip_deadline(Some(Duration::ZERO));",
        "    eng.set_strip_deadline(Some(Duration::from_millis(500)));",
        "    eng.set_block_deadline(Some(Instant::now() + Duration::from_millis(2)));",
        "    MixEngine::force_serial(true);",
        "    shared.hold.store(true, Ordering::Release);\n    while !shared.holding.load(Ordering::Acquire) {}",
    ] {
        assert_eq!(refusals(MIX, &pool(ok)), vec![], "{ok}");
    }
    // A far deadline in a same-file constant.
    let far = format!("const FAR: Duration = Duration::from_secs(60);\n{}", pool("    eng.set_strip_deadline(Some(FAR));"));
    assert_eq!(refusals(MIX, &far), vec![]);
    // Serial only when switched on before the first render and never otherwise.
    let late = test_fn("    eng.start_render_workers(16);\n    eng.render_active(0, 512, &mut out);\n    MixEngine::force_serial(true);");
    assert_eq!(refusals(MIX, &late), vec![(2, 1)], "switched on after the render (a test restoring it)");
    assert_eq!(refusals(MIX, &pool("    MixEngine::force_serial(true);\n    MixEngine::force_serial(serial);")), vec![(2, 1)]);
}

#[test]
fn refusal_1_follows_one_level_of_same_file_helpers_and_takes_its_markers() {
    let helper = "fn rig() -> Engine {\n    let mut eng = engine();\n    eng.start_render_workers(16);\n    eng\n}\n";
    let src = format!("{helper}#[test]\nfn t() {{\n    let mut eng = rig();\n    eng.render_active(0, 512, &mut out);\n}}\n");
    assert_eq!(refusals(MIX, &src), vec![(7, 1)], "workers started in the helper");
    let with_deadline = src.replace("    eng\n}", "    eng.set_strip_deadline(Some(Duration::from_secs(30)));\n    eng\n}");
    assert_eq!(refusals(MIX, &with_deadline), vec![], "the helper sets a far deadline");
    for tag in ["far-deadline", "held-worker", "explicit-deadline", "serial", "host-timeout"] {
        let marked = src.replace("#[test]", &format!("// timing: {tag} the shared rig in another file sets it\n#[test]"));
        assert_eq!(refusals(MIX, &marked), vec![], "{tag}");
    }
    let no_reason = src.replace("#[test]", "// timing: far-deadline\n#[test]");
    assert_eq!(refusals(MIX, &no_reason), vec![(8, 1)], "a marker needs its reason");
    let wrong = src.replace("#[test]", "// timing: any-strip it means any strip\n#[test]");
    assert_eq!(refusals(MIX, &wrong), vec![(8, 1)], "any-strip allows refusal 3 only");
}

#[test]
fn refusal_1_skips_what_has_no_default_to_inherit() {
    // Workers started, nothing rendered.
    let unrendered = test_fn("    eng.start_render_workers(16);\n    assert_eq!(eng.render_worker_promotions().len(), 3);");
    assert_eq!(refusals(MIX, &unrendered), vec![]);
    // A bare worker set takes its deadline per block.
    let rt = test_fn("    let mut set = set_of(&fake, 2);\n    let r = set.finish(Instant::now() + STALL, |_| ());");
    assert_eq!(refusals("crates/rt/src/worker_set/tests.rs", &rt), vec![]);
    // Ignored tests, crates out of scope and non-test code.
    assert_eq!(refusals(MIX, &pool("").replace("#[test]", "#[test]\n#[ignore = \"hardware\"]")), vec![]);
    assert_eq!(refusals("crates/engine/tests/t.rs", &pool("")), vec![]);
    assert_eq!(scan("crates/mix/src/x.rs", &pool("").replace("#[test]\n", ""), false), vec![]);
}

#[test]
fn refusal_1_needs_a_long_give_up_in_a_host_thread_test() {
    let ph = "crates/plugin-host/tests/t.rs";
    let src = test_fn("    let host = host(Duration::from_secs(1));\n    let p = host.submit(|_| ()).wait(Duration::from_millis(500)).unwrap();");
    assert_eq!(refusals(ph, &src), vec![(2, 1)], "a 500 ms give-up");
    assert_eq!(refusals(ph, &src.replace("from_millis(500)", "from_secs(5)")), vec![]);
    let marked = src.replace("#[test]", "// timing: host-timeout the subject is the host's own timeout\n#[test]");
    assert_eq!(refusals(ph, &marked), vec![]);
    let inline = test_fn("    let host = Host::new(Config { inline: true, ..Config::default() });\n    host.submit(|_| ()).poll();");
    assert_eq!(refusals(ph, &inline), vec![], "an inline host has no thread");
    let asio = test_fn("    let rig = rig_watching(scarlett(), Duration::from_millis(250));\n    drive(&rig, 64, 0, 3, &[]);");
    let asio_path = "crates/audio-asio/src/stream_tests.rs";
    assert_eq!(refusals(asio_path, &asio), vec![(2, 1)]);
    assert_eq!(refusals(asio_path, &asio.replace("from_millis(250)", "from_secs(3600)")), vec![]);
}

#[test]
fn refusal_2_flags_a_short_deadline_with_no_held_worker_through_a_helper_parameter() {
    let helper = "fn rig(deadline: Duration) -> Rig {\n    let mut rig = Rig::new(6, 0, 128);\n    rig.mix.start_render_workers(64);\n    rig.mix.set_strip_deadline(Some(deadline));\n    rig\n}\n";
    let test = |deadline: &str, body: &str| {
        format!("use common::workers;\n{helper}#[test]\nfn t() {{\n    let mut rig = rig({deadline});\n{body}\n    rig.block();\n}}\n")
    };
    assert_eq!(refusals(MIX, &test("Duration::from_micros(500)", "")), vec![(9, 2)]);
    assert_eq!(refusals(MIX, &test("Duration::from_millis(2)", "")), vec![(9, 2)]);
    assert_eq!(refusals(MIX, &test("Duration::from_secs(30)", "")), vec![], "far");
    let held = "    rig.shared[2].hold.store(true, Ordering::Release);\n    while !rig.shared[2].holding.load(Ordering::Acquire) { rig.block(); }";
    assert_eq!(refusals(MIX, &test("Duration::from_micros(500)", held)), vec![], "a held worker");
    let set_only = "    rig.shared[2].hold.store(true, Ordering::Release);";
    assert_eq!(refusals(MIX, &test("Duration::from_micros(500)", set_only)), vec![(9, 2)], "a hold nobody waits on");
    let marked =
        test("Duration::from_micros(500)", "").replace("#[test]", "// timing: explicit-deadline injected stalls decide the overruns\n#[test]");
    assert_eq!(refusals(MIX, &marked), vec![]);
}

/// E49 recurrence 2026-10-30: `render_workers::a_plan_waits_while_a_worker_holds_a_strip` waited for
/// any away strip and asserted about the one it had stalled.
#[test]
fn refusal_3_flags_a_wait_for_any_strip_before_a_per_strip_assertion() {
    let src = test_fn(
        "    rig.mix.set_strip_deadline(Some(Duration::from_secs(60)));
    rig.mix.start_render_workers(64);
    rig.shared[1].stall_ns.store(100_000_000, Ordering::Relaxed);
    while rig.mix.render_workers_settled() {
        rig.block();
    }
    assert_eq!(rig.note_on(1, 0, 70), EventOutcome::Queued);",
    );
    assert_eq!(refusals(MIX, &src), vec![(6, 3)]);
    let held = src.replace("    assert_eq!(rig.note_on", "    assert!(rig.shared[1].holding.load(Ordering::Acquire));\n    assert_eq!(rig.note_on");
    assert_eq!(refusals(MIX, &held), vec![]);
    let any_strip = src.replace("#[test]", "// timing: any-strip the assertion holds for whichever strip is away\n#[test]");
    assert_eq!(refusals(MIX, &any_strip), vec![]);
    // Waiting for every strip to come home is the completion signal, not this refusal.
    assert_eq!(refusals(MIX, &src.replace("while rig.mix", "while !rig.mix")), vec![]);
}

/// The quiet period `crates/mix/tests/no_render_promotion.rs::settle` used before #226.
#[test]
fn refusal_4_flags_a_quiet_period_wait_wherever_it_sits() {
    let settle = "fn settle() -> usize {
    let mut last = promotion_calls();
    let mut since = Instant::now();
    while since.elapsed() < Duration::from_millis(300) {
        std::thread::sleep(Duration::from_millis(20));
        let now = promotion_calls();
        if now != last {
            last = now;
            since = Instant::now();
        }
    }
    last
}
";
    assert_eq!(refusals(MIX, settle), vec![(4, 4)], "in a helper");
    assert_eq!(refusals("crates/playback/src/x_tests.rs", settle), vec![(4, 4)]);
    let marked = settle.replace("fn settle", "// timing: stall-bound a marker does not admit it\nfn settle");
    assert_eq!(refusals(MIX, &marked), vec![(5, 4)], "no marker allows a quiet period");
    // A no-progress guard far above any stall fails the test when it fires.
    let guard = "const NO_PROGRESS: Duration = Duration::from_secs(30);
fn t() {
    let mut last_progress = Instant::now();
    loop {
        if moved() {
            last_progress = Instant::now();
        } else if last_progress.elapsed() >= NO_PROGRESS {
            break;
        }
    }
    assert_eq!(received, COUNT, \"a no-progress exit fails here\");
}
";
    assert_eq!(refusals(MIX, guard), vec![]);
    // The longest gap between blocks is a measurement, not an exit.
    let gap = "fn t() {\n    let mut last = Instant::now();\n    for _ in 0..8 {\n        worst = worst.max(last.elapsed());\n        last = Instant::now();\n    }\n}\n";
    assert_eq!(refusals(MIX, gap), vec![]);
}

/// The closing-block rule of gates.md (E49 recurrence 2026-10-30).
#[test]
fn refusal_5_flags_a_coverage_check_after_closing_blocks_at_the_short_deadline() {
    let src = test_fn(
        "    rig.mix.start_render_workers(64);
    rig.mix.set_strip_deadline(Some(Duration::from_micros(500)));
    rig.shared[3].hold.store(true, Ordering::Release);
    while !rig.shared[3].holding.load(Ordering::Acquire) { rig.block(); }
    rig.shared[3].stall_ns.store(0, Ordering::Relaxed);
    settle(&mut rig);
    assert_eq!(rig.shared[3].report().stuck_notes, 0);",
    );
    assert_eq!(refusals(MIX, &src), vec![(9, 5)]);
    let far = src.replace("    settle(&mut rig);", "    rig.mix.set_strip_deadline(Some(Duration::from_secs(60)));\n    settle(&mut rig);");
    assert_eq!(refusals(MIX, &far), vec![]);
    let serial = src.replace("    settle(&mut rig);", "    MixEngine::force_serial(true);\n    settle(&mut rig);");
    assert_eq!(refusals(MIX, &serial), vec![]);
    let hold = src.replace("stall_ns.store(0, Ordering::Relaxed)", "hold.store(false, Ordering::Release)");
    assert_eq!(refusals(MIX, &hold), vec![(9, 5)], "a hold released the same way");
    let marked = src.replace("#[test]", "// timing: far-deadline the shared settle sets it\n#[test]");
    assert_eq!(refusals(MIX, &marked), vec![]);
    let helper = src.replace("#[test]\nfn t()", "fn closing(rig: &mut Rig)");
    assert_eq!(refusals(MIX, &helper), vec![(8, 5)], "in a helper of the test");
}

/// The first-block baseline of a hole count (E49 recurrences 2026-10-30 and 2026-10-31).
#[test]
fn refusal_6_flags_a_hole_count_with_no_first_block_baseline() {
    let src = test_fn(
        "    rig.mix.start_render_workers(64);
    rig.mix.set_strip_deadline(Some(Duration::from_secs(60)));
    rig.block();
    let holes: u64 = rig.shared.iter().map(|s| s.report().hole_samples).sum();
    assert_eq!(holes, f.overruns_unstarted * 128);",
    );
    assert_eq!(refusals(MIX, &src), vec![(6, 6)], "render_graph.rs before the fix");
    let based = src.replace(
        "    rig.block();",
        "    expect_from_the_start(&rig);
    rig.block();",
    );
    assert_eq!(refusals(MIX, &based), vec![], "the shared helper's call names it");
    let direct = src.replace(
        "    rig.block();",
        "    s.inst.lock().unwrap().expect_from(0);
    rig.block();",
    );
    assert_eq!(refusals(MIX, &direct), vec![]);
    let covered = src.replace("let holes: u64 = rig.shared.iter().map(|s| s.report().hole_samples).sum();", "assert_time_covered(&rig);");
    assert_eq!(refusals(MIX, &covered), vec![(6, 6)], "the processed-plus-holes identity too");
    for tag in ["far-deadline", "held-worker", "explicit-deadline", "serial", "any-strip"] {
        let marked = src.replace(
            "#[test]",
            &format!(
                "// timing: {tag} no marker allows it
#[test]"
            ),
        );
        assert_eq!(refusals(MIX, &marked), vec![(7, 6)], "{tag}");
    }
    let helper = format!(
        "fn count(rig: &Rig) -> u64 {{
    rig.shared.iter().map(|s| s.report().hole_samples).sum()
}}
{}",
        src.replace("let holes: u64 = rig.shared.iter().map(|s| s.report().hole_samples).sum();", "let holes = count(&rig);")
    );
    assert_eq!(refusals(MIX, &helper), vec![(5, 6)], "a same-file helper's count, reported on the test");
    let not_pool = test_fn(
        "    let r = inst.report();
    assert_eq!(r.hole_samples, 0);",
    );
    assert_eq!(refusals("crates/rt/src/midi_defer/tests.rs", &not_pool), vec![], "the instrument's own tests");
}

#[test]
fn rule_e_reports_through_the_timing_gate() {
    let hits = super::super::scan(MIX, &pool(""), true);
    assert_eq!(hits.iter().map(|h| (h.line, h.rule)).collect::<Vec<_>>(), vec![(2, 'e')]);
    assert!(hits[0].text.starts_with("refusal 1: fn t()"), "{}", hits[0].text);
}

/// E62: a long allowance is for a no-progress guard whose exit fails the test, never for a quiet
/// period whose end is the pass path, and a reset with the `std::time::` path counts too.
#[test]
fn refusal_4_allows_only_a_long_guard_that_breaks_into_a_failing_assertion() {
    let long_while = "fn t() {
    let mut since = Instant::now();
    while since.elapsed() < Duration::from_secs(5) {
        if moved() {
            since = Instant::now();
        }
    }
    assert!(done());
}
";
    assert_eq!(refusals(MIX, long_while), vec![(3, 4)], "a 5 s quiet period is still a pass condition");
    let guard = "fn t() {
    let mut last = Instant::now();
    loop {
        if moved() {
            last = std::time::Instant::now();
        } else if last.elapsed() >= Duration::from_millis(5_000) {
            break;
        }
    }
    assert_eq!(count, N);
}
";
    assert_eq!(refusals(MIX, guard), vec![], "a 5000 ms guard with an assertion after it");
    let no_assert = guard.replace("    assert_eq!(count, N);\n", "");
    assert_eq!(refusals(MIX, &no_assert), vec![(6, 4)], "a guard whose exit nothing fails");
    let short = guard.replace("from_millis(5_000)", "from_millis(300)");
    assert_eq!(refusals(MIX, &short), vec![(6, 4)], "a short guard, std::time path");
    let panics = short.replace("            break;", "            panic!(\"no progress\");");
    assert_eq!(refusals(MIX, &panics), vec![], "a guard that fails at once");
}

/// The marker and evidence combinations the first tests leave out.
#[test]
fn the_remaining_markers_and_evidence_forms() {
    let helper = "fn rig(deadline: Duration) -> Rig {\n    let mut rig = Rig::new(6, 0, 128);\n    rig.mix.start_render_workers(64);\n    rig.mix.set_strip_deadline(Some(deadline));\n    rig\n}\n";
    let short = |body: &str| {
        format!(
            "use common::workers;\n{helper}#[test]\nfn t() {{\n    let mut rig = rig(Duration::from_micros(500));\n{body}\n    rig.block();\n}}\n"
        )
    };
    for tag in ["far-deadline", "held-worker"] {
        let marked = short("").replace("#[test]", &format!("// timing: {tag} the hold is in another file\n#[test]"));
        assert_eq!(refusals(MIX, &marked), vec![], "{tag} on refusal 2");
    }
    for held in [
        "    hold_until_away(&mut rig, 2);",
        "    LATE_HOLD.store(true, Ordering::Release);\n    while !LATE_ENTERED.load(Ordering::Acquire) {}",
        "    hold.store(true, Ordering::Release);\n    while reader.held_tracks().is_empty() { rig.block(); }",
    ] {
        assert_eq!(refusals(MIX, &short(held)), vec![], "{held}");
    }
    let closing = test_fn(
        "    rig.mix.start_render_workers(64);
    rig.mix.set_strip_deadline(Some(Duration::ZERO));
    rig.shared[3].stall_ns.store(0, Ordering::Relaxed);
    rig.block();
    assert_eq!(rig.shared[3].report().stuck_notes, 0);",
    );
    assert_eq!(refusals(MIX, &closing), vec![(7, 5)]);
    let serial = closing.replace("#[test]", "// timing: serial the closing blocks render serially in a helper\n#[test]");
    assert_eq!(refusals(MIX, &serial), vec![], "serial on refusal 5");
    let held = closing.replace("#[test]", "// timing: held-worker not a closing-block reason\n#[test]");
    assert_eq!(refusals(MIX, &held), vec![(8, 5)], "held-worker does not allow refusal 5");
    let ph = "crates/plugin-host/tests/t.rs";
    for make in ["Host::spawn_after(cfg, Duration::from_millis(50))", "slow_host(Duration::from_millis(200), Duration::from_secs(1))"] {
        let src = test_fn(&format!("    let host = {make};\n    host.submit(|_| ()).wait(Duration::from_millis(500));"));
        assert_eq!(refusals(ph, &src), vec![(2, 1)], "{make}");
    }
}
