//! Rule (e) of `cargo xtask timing` (#226, E49 recurrences 2026-10-29 and 2026-10-30): a test that takes
//! the render-worker or host-thread path decides every outcome itself. Line-based over the parse in
//! `timing.rs`, with the one-level same-file helper resolution rule (d) uses: a test fn's evidence is its
//! own body plus the bodies of the same-file fns it calls (with a helper's parameters bound to the call's
//! arguments for the strip deadline it sets).
//!
//! Scanned: test code in `crates/{mix,rt,plugin-host,audio-asio,playback,ui-studio}`. A `#[test]` fn (not
//! `#[ignore]`) is a pool or host-thread test when its evidence names `start_render_workers`, `WorkerSet`,
//! `set_of(`, a `rig(`/`keyed_rig(`/`Rig::new(` call in a file that names `common::workers`, `Host::new`,
//! `Host::spawn_after`, `slow_host(` or `host(` in plug-in host tests, or `rig_watching(`/`hold_host(` in the
//! ASIO tests. The refusals:
//!
//! 1. A pool test with no deadline of its own (a far deadline, a held worker, a zero deadline or explicit
//!    instants, serial rendering), or a host-thread test with no give-up of 5 s or more: the inherited
//!    default (E49 recurrence 2026-10-29).
//! 2. A short strip deadline (`from_micros`, `from_millis` below 100) with no held worker.
//! 3. A `while ..render_workers_settled()` loop (waiting for some strip to go away) in a fn that injects a
//!    stall (`stall_ns`) and asserts a per-strip outcome without reading `holding` (E49 recurrence 2026-10-30).
//! 4. A quiet-period wait: a loop that exits on `v.elapsed()` for an `Instant` it resets when a value moves
//!    (a no-progress guard of 5 s or more that breaks into a failing assertion is allowed, E62).
//! 5. A coverage assertion (`assert_time_covered`, `hole_samples`, `stuck_notes`) after the last stall or
//!    hold release with no far deadline (or serial rendering) set between them.
//!
//! Markers, `// timing: <tag> <reason>` on the fn header or in the attribute and comment lines directly
//! above it, reason required: `far-deadline`, `held-worker`, `explicit-deadline`, `serial` and
//! `host-timeout` satisfy refusal 1 (the first three also 2, `far-deadline` and `serial` also 5);
//! `any-strip` allows refusal 3. Refusal 4 has no marker: the exit must name the state.

use super::{Func, Hit, Parsed, parse, tokens};

/// The crates whose tests rule (e) scans.
const CRATES: &[&str] = &["mix", "rt", "plugin-host", "audio-asio", "playback", "ui-studio"];
/// Marker tags and the refusals each one allows.
const MARKERS: &[(&str, &[u8])] = &[
    ("far-deadline", &[1, 2, 5]),
    ("held-worker", &[1, 2]),
    ("explicit-deadline", &[1, 2]),
    ("serial", &[1, 5]),
    ("host-timeout", &[1]),
    ("any-strip", &[3]),
];
/// Per-strip outcomes refusal 3 is about.
const PER_STRIP: &[&str] = &["Queued", "StaleBinding", "midi_dropped", "held_tracks"];
/// Coverage assertions refusal 5 is about.
const COVERAGE: &[&str] = &["assert_time_covered", "hole_samples", "stuck_notes"];

/// What a strip deadline argument resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Deadline {
    /// `from_secs(N)` with N >= 10.
    Far,
    /// `Duration::ZERO`.
    Zero,
    /// `from_micros(..)`, `from_millis(N)` with N < 100.
    Short,
    /// `None`: the default share again.
    Default,
    /// Anything else (a medium literal, a name from another file).
    Other,
}

/// The crate a repo-relative path belongs to, when rule (e) scans it.
fn in_scope(path: &str) -> Option<&'static str> {
    let rest = path.strip_prefix("crates/")?;
    let name = rest.split('/').next()?;
    CRATES.iter().copied().find(|c| *c == name)
}

/// The fn name of a header line.
fn fn_name(code: &str) -> Option<String> {
    let at = code.find("fn ")?;
    let ident: String = code[at + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!ident.is_empty()).then_some(ident)
}

/// The parameter names of a fn header (its first parenthesized list; `self` is skipped).
fn params(header: &str) -> Vec<String> {
    let Some(open) = header.find('(') else { return Vec::new() };
    let Some(args) = balanced(&header[open..]) else { return Vec::new() };
    split_top(&args).iter().filter_map(|a| a.split_once(':').map(|(n, _)| n.trim().trim_start_matches("mut ").to_string())).collect()
}

/// The text inside the parentheses that open `s` (which starts with `(`), when they close on this line.
fn balanced(s: &str) -> Option<String> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(s[1..i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits a list at its top-level commas.
fn split_top(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0usize);
    for c in s.chars() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Classifies a duration expression; `names` resolves a bare identifier (a same-file binding or a bound
/// parameter) one step.
fn classify(expr: &str, names: &dyn Fn(&str) -> Option<String>) -> Deadline {
    let e = expr.trim().trim_start_matches('&');
    if e == "None" {
        return Deadline::Default;
    }
    let e = e.strip_prefix("Some(").and_then(|r| r.strip_suffix(')')).unwrap_or(e).trim();
    if e.ends_with("Duration::ZERO") {
        return Deadline::Zero;
    }
    if e.contains("from_micros(") || e.contains("from_nanos(") {
        return Deadline::Short;
    }
    let arg = |f: &str| e.split_once(f).and_then(|(_, r)| r.split(')').next()).map(|d| d.replace('_', "")).and_then(|d| d.trim().parse::<u64>().ok());
    if let Some(n) = arg("from_millis(") {
        return if n < 100 { Deadline::Short } else { Deadline::Other };
    }
    if let Some(n) = arg("from_secs(") {
        return if n >= 10 { Deadline::Far } else { Deadline::Other };
    }
    if e.chars().all(|c| c.is_alphanumeric() || c == '_')
        && let Some(bound) = names(e)
    {
        return classify(&bound, &|_| None);
    }
    Deadline::Other
}

/// Same-file `const`/`static`/`let` bindings of a name to a `Duration` expression.
fn bindings(p: &Parsed) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for code in &p.code {
        let Some((lhs, rhs)) = code.split_once(" = ") else { continue };
        if !rhs.contains("Duration::") {
            continue;
        }
        let mut words = tokens(lhs).skip_while(|t| !matches!(*t, "const" | "static" | "let")).skip(1);
        if let Some(name) = words.find(|t| *t != "mut") {
            out.push((name.to_string(), rhs.trim().trim_end_matches(';').to_string()));
        }
    }
    out
}

/// A call of `name(` as a free function on `line` (not a method, not its definition): its argument text.
fn calls<'a>(line: &'a str, name: &str) -> Vec<&'a str> {
    let pat = format!("{name}(");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = line[from..].find(&pat).map(|i| i + from) {
        let before = line[..at].chars().next_back();
        let free = !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') && !line[..at].trim_end().ends_with("fn");
        if free && let Some(open) = line[at + name.len()..].find('(') {
            out.push(&line[at + name.len() + open..]);
        }
        from = at + pat.len();
    }
    out
}

/// A free call of `name(` on `line`, or a path call `..::name(`.
fn has_call(line: &str, name: &str) -> bool {
    !calls(line, name).is_empty() || line.contains(&format!("::{name}("))
}

/// One test fn's evidence: its own lines, then the lines of the same-file fns it calls, each with the
/// deadline its `set_strip_deadline` calls resolve to.
struct Evidence {
    lines: Vec<String>,
    deadlines: Vec<Deadline>,
}

fn evidence(p: &Parsed, f: &Func, fns: &[(String, &Func)], binds: &[(String, String)]) -> Evidence {
    let lookup = |n: &str| binds.iter().find(|(b, _)| b == n).map(|(_, v)| v.clone());
    let own: Vec<String> = p.code[f.start..=f.end].to_vec();
    let mut lines = own.clone();
    let mut deadlines = Vec::new();
    let mut scan_deadlines = |body: &[String], names: &dyn Fn(&str) -> Option<String>| {
        for l in body {
            for args in calls(l, "set_strip_deadline").into_iter().chain(l.split_once(".set_strip_deadline").map(|(_, r)| r)) {
                if let Some(inner) = balanced(args) {
                    deadlines.push(classify(&inner, names));
                }
            }
        }
    };
    scan_deadlines(&own, &lookup);
    for (name, g) in fns {
        if std::ptr::eq(*g, f) || g.start >= f.start && g.end <= f.end {
            continue;
        }
        let call_args: Vec<String> = own.iter().flat_map(|l| calls(l, name)).filter_map(balanced).collect();
        if call_args.is_empty() {
            continue;
        }
        let body = &p.code[g.start..=g.end];
        lines.extend(body.iter().cloned());
        let names = params(&p.code[g.start]);
        for args in call_args {
            let args = split_top(&args);
            let bound = |n: &str| names.iter().position(|x| x == n).and_then(|i| args.get(i).cloned()).or_else(|| lookup(n));
            scan_deadlines(body, &bound);
        }
    }
    Evidence { lines, deadlines }
}

/// Whether the fn starting at `start` carries `#[test]`, and the comments of the attribute and comment
/// lines directly above it and of its header.
fn header_block(p: &Parsed, start: usize) -> (bool, Vec<&str>) {
    let mut test = false;
    let mut comments = vec![p.comment[start].as_str()];
    let mut i = start;
    while i > 0 {
        i -= 1;
        let code = p.code[i].trim();
        if code.starts_with("#[") {
            test |= code.starts_with("#[test]");
        } else if !code.is_empty() || p.comment[i].is_empty() {
            break;
        }
        comments.push(p.comment[i].as_str());
    }
    (test, comments)
}

/// The refusals the markers in `comments` allow.
fn allowed(comments: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for c in comments {
        let Some(at) = c.find("// timing:") else { continue };
        let rest = c[at + "// timing:".len()..].trim();
        for (tag, refusals) in MARKERS {
            if rest.strip_prefix(tag).is_some_and(|r| r.starts_with(' ') && !r.trim().is_empty()) {
                out.extend_from_slice(refusals);
            }
        }
    }
    out
}

fn any(lines: &[String], pats: &[&str]) -> bool {
    lines.iter().any(|l| pats.iter().any(|p| l.contains(p)))
}

/// The pool trigger: an engine with render workers (or the shared worker rig) that renders a block.
/// A bare `transit-rt` worker set has no default deadline to inherit (`finish` takes the block's), so
/// it is not a trigger; nor is an engine that starts workers and never renders.
fn pool_test(ev: &Evidence, shared_rig: bool) -> bool {
    let workers = any(&ev.lines, &["start_render_workers"])
        || (shared_rig
            && ev
                .lines
                .iter()
                .any(|l| has_call(l, "rig") || has_call(l, "keyed_rig") || l.contains("Rig::new(") || l.contains("Rig::with_session(")));
    workers && any(&ev.lines, &["render_active(", ".block(", "render_range("])
}

/// The host-thread trigger of the plug-in host and ASIO tests.
fn host_test(ev: &Evidence, krate: &str) -> bool {
    match krate {
        // An inline host runs every job on the caller: no host thread.
        "plugin-host" => {
            !any(&ev.lines, &["inline: true"])
                && ev
                    .lines
                    .iter()
                    .any(|l| l.contains("Host::new") || l.contains("Host::spawn_after") || has_call(l, "slow_host") || has_call(l, "host"))
        }
        "audio-asio" => ev.lines.iter().any(|l| has_call(l, "rig_watching") || has_call(l, "hold_host")),
        _ => false,
    }
}

/// A worker held in a plug-in and a read of the flag it sets there (or of the held strip's own report).
fn holds(lines: &[String]) -> bool {
    let set = any(lines, &["hold.store(true", "HOLD.store(true", "hold_until_away("]);
    let read = any(lines, &["holding.load(", "entered.load(", "ENTERED.load(", "hold_until_away(", "held_tracks()"]);
    set && read
}

/// Explicit instants: a block deadline, a worker set's `finish` or a `run_at` fed an instant.
fn explicit(lines: &[String]) -> bool {
    any(lines, &["set_block_deadline(Some(", ".finish(", "run_at("])
}

/// Serial rendering: the switch turned on before the first render and never set to anything else.
fn serial(lines: &[String]) -> bool {
    let on = lines.iter().position(|l| l.contains("force_serial(true)"));
    let render = lines.iter().position(|l| ["render_active(", ".block(", "render_range("].iter().any(|r| l.contains(r)));
    let other = lines.iter().any(|l| l.split("force_serial(").skip(1).any(|r| !r.starts_with("true)")));
    on.is_some_and(|on| render.is_none_or(|r| on < r)) && !other
}

/// A give-up of 5 s or more (a wait, a poll's stall bound, a shutdown).
fn long_give_up(lines: &[String]) -> bool {
    lines.iter().any(|l| {
        let mut rest = l.as_str();
        while let Some(at) = rest.find("from_secs(") {
            let arg = &rest[at + "from_secs(".len()..];
            if arg.split(')').next().and_then(|d| d.replace('_', "").trim().parse::<u64>().ok()).is_some_and(|n| n >= 5) {
                return true;
            }
            rest = arg;
        }
        false
    })
}

/// Refusal 4 in `f`'s own lines: an `Instant` reassigned in the fn (`x = Instant::now()`, or with the
/// `std::time::` path) whose `elapsed()` decides a loop exit. A `while` condition on it is always a quiet
/// period: the loop's end is the pass path. An `if` on it that panics or asserts is a failing guard, not a
/// pass. An `if` that breaks is allowed only as a no-progress guard (E62): a bound of 5 s or more and an
/// assertion after the exit in the fn, which the guard's exit fails (review still checks that it does).
fn quiet_period(body: &[String], binds: &[(String, String)]) -> Option<usize> {
    let reset: Vec<String> = body
        .iter()
        .filter_map(|l| {
            let (lhs, rhs) = l.split_once(" = ")?;
            let lhs = lhs.trim();
            let now = rhs.trim_start().trim_start_matches("std::time::").starts_with("Instant::now()");
            (now && !lhs.contains("let ") && lhs.chars().all(|c| c.is_alphanumeric() || c == '_')).then(|| lhs.to_string())
        })
        .collect();
    body.iter().enumerate().find_map(|(i, l)| {
        let exit = |v: &String| {
            let read = format!("{v}.elapsed()");
            let Some((_, after)) = l.split_once(&read) else { return false };
            if l.trim_start().starts_with("while ") {
                return true;
            }
            let next = body.get(i + 1).map_or("", String::as_str);
            if !l.contains("if ") {
                return false;
            }
            if [l.as_str(), next].iter().any(|x| x.contains("panic!(") || x.contains("assert")) {
                return false;
            }
            if !(l.contains("break") || next.contains("break")) {
                return false;
            }
            let bound = after.trim_start().trim_start_matches(['<', '>', '=']).trim_start();
            let bound = bound.split([' ', '{']).next().unwrap_or("");
            let asserted_after = body[i + 1..].iter().any(|x| x.contains("assert"));
            !(long(bound, binds) && asserted_after)
        };
        reset.iter().any(exit).then_some(i)
    })
}

/// A duration of 5 s or more, written inline or bound to a same-file name.
fn long(expr: &str, binds: &[(String, String)]) -> bool {
    let e = binds.iter().find(|(n, _)| n == expr).map_or(expr, |(_, v)| v.as_str());
    let arg = |f: &str| e.split_once(f).and_then(|(_, r)| r.split(')').next()).and_then(|d| d.replace('_', "").trim().parse::<u64>().ok());
    arg("from_secs(").is_some_and(|n| n >= 5) || arg("from_millis(").is_some_and(|n| n >= 5_000)
}

pub fn scan(path: &str, src: &str, test_file: bool) -> Vec<Hit> {
    let Some(krate) = in_scope(path) else { return Vec::new() };
    let p = parse(src, test_file);
    let shared_rig = src.contains("common::workers");
    let binds = bindings(&p);
    let fns: Vec<(String, &Func)> = p.funcs.iter().filter_map(|f| fn_name(&p.code[f.start]).map(|n| (n, f))).collect();
    let lines: Vec<&str> = src.lines().collect();
    let mut hits = Vec::new();
    for f in &p.funcs {
        if !p.in_test[f.start] || f.ignored {
            continue;
        }
        let (test, comments) = header_block(&p, f.start);
        if !test {
            continue;
        }
        let ok = allowed(&comments);
        let ev = evidence(&p, f, &fns, &binds);
        let own = &p.code[f.start..=f.end];
        let mut refuse = |n: u8, line: usize| {
            if !ok.contains(&n) {
                let text = format!("refusal {n}: {}", lines.get(line).copied().unwrap_or("").trim());
                hits.push(Hit { path: path.into(), line: line + 1, rule: 'e', text });
            }
        };
        let held = holds(&ev.lines);
        let pool = pool_test(&ev, shared_rig);
        if pool {
            // Any deadline of the test's own counts here; a short one is refusal 2's.
            let set = ev.deadlines.iter().any(|d| *d != Deadline::Default);
            let zero = ev.deadlines.contains(&Deadline::Zero);
            let explicit = explicit(&ev.lines);
            if !(set || held || explicit || serial(&ev.lines)) {
                refuse(1, f.start);
            }
            if ev.deadlines.contains(&Deadline::Short) && !(held || zero || explicit) {
                refuse(2, f.start);
            }
        } else if host_test(&ev, krate) && !long_give_up(&ev.lines) {
            refuse(1, f.start);
        }
        // 3: a wait for some strip to be away, then a per-strip assertion, with no wait on the strip.
        let away = |l: &String| {
            let t = l.trim_start();
            t.starts_with("while ") && !t.starts_with("while !") && t.contains("render_workers_settled()")
        };
        if ev.lines.iter().any(away) && any(&ev.lines, &["stall_ns"]) && any(&ev.lines, PER_STRIP) && !any(&ev.lines, &["holding.load("]) {
            let line = own.iter().position(away).map_or(f.start, |i| f.start + i);
            refuse(3, line);
        }
    }
    let pool_file = shared_rig || src.contains("start_render_workers");
    for f in p.funcs.iter().filter(|f| p.in_test[f.start] && !f.ignored) {
        let own = &p.code[f.start..=f.end];
        // 4, in every fn of test code (the wait usually sits in a helper): no marker allows it.
        if let Some(i) = quiet_period(own, &binds) {
            let line = f.start + i;
            let text = format!("refusal 4: {}", lines.get(line).copied().unwrap_or("").trim());
            hits.push(Hit { path: path.into(), line: line + 1, rule: 'e', text });
        }
        // 5, in every fn of a pool test file (a test or the helper that holds its closing blocks): the
        // last stall or hold release, then a coverage assertion with no far deadline or serial switch
        // between them.
        if !pool_file {
            continue;
        }
        let release = |l: &String| l.contains("stall_ns.store(") || l.contains("hold.store(false") || l.contains("HOLD.store(false");
        let Some(last) = own.iter().rposition(release) else { continue };
        let covers = |l: &String| COVERAGE.iter().any(|c| l.contains(c)) && l.contains("assert");
        let Some(c) = own[last..].iter().position(covers) else { continue };
        let between = &own[last..last + c];
        let lookup = |n: &str| binds.iter().find(|(b, _)| b == n).map(|(_, v)| v.clone());
        let far = between
            .iter()
            .any(|l| l.split_once(".set_strip_deadline").and_then(|(_, r)| balanced(r)).is_some_and(|a| classify(&a, &lookup) == Deadline::Far))
            || any(between, &["force_serial(true)"]);
        if !far && !allowed(&header_block(&p, f.start).1).contains(&5) {
            let line = f.start + last + c;
            let text = format!("refusal 5: {}", lines.get(line).copied().unwrap_or("").trim());
            hits.push(Hit { path: path.into(), line: line + 1, rule: 'e', text });
        }
    }
    // A nested fn's lines are its parent's too.
    hits.sort_by_key(|h| h.line);
    hits.dedup();
    hits
}

#[cfg(test)]
#[path = "timing_pool_tests.rs"]
mod tests;
