//! `cargo xtask timing`: flags E49-class timing assertions in test code (docs/contributing/evidence/gates.md).
//!
//! Three rules, line-based and deliberately simple:
//! (a) a comparison against `Duration::from_millis(N)` with N < 100, or any `Duration::from_micros(..)`, written
//!     inline or through a `const`, `static` or `let` name bound to one in the same file;
//! (b) `thread::sleep(` in a fn that also asserts on a rate, BPM, latency, `elapsed()` or a loop counter;
//! (c) a throughput floor `assert!(x >= N)` / `assert!(N <= x)`, N >= 3, x a frames/blocks/laps/iterations/polls
//!     count or a counter incremented in a fn that also reads a clock or sleeps.
//!
//! Allow marker, on the offending line or the line above: `// timing: <tag> <reason>` where the tag is
//! `stall-bound` (a ceiling far above jitter), `median` (a median over many samples), `floor-ok` (a count that is
//! not clock-bound), `sleep-ok` (a sleep that is not pacing a measurement) or `value` (a bound on a computed value,
//! not a measurement); the reason is mandatory. Functions marked `#[ignore]`
//! (hardware tests) are skipped.

use std::path::Path;

const TAGS: &[&str] = &["stall-bound", "median", "floor-ok", "sleep-ok", "value"];
const FLOOR_NAMES: &[&str] = &["frames", "blocks", "laps", "iterations", "polls"];
const MEASURE_WORDS: &[&str] = &["bpm", "latency", "elapsed"];

#[derive(Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    pub line: usize,
    pub rule: char,
    pub text: String,
}

impl std::fmt::Display for Hit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: rule ({}): {}", self.path, self.line, self.rule, self.text.trim())
    }
}

struct Func {
    start: usize,
    end: usize,
    ignored: bool,
}

struct Parsed {
    code: Vec<String>,
    comment: Vec<String>,
    in_test: Vec<bool>,
    funcs: Vec<Func>,
}

/// Split a line into code (string contents blanked) and its trailing `//` comment.
fn split_line(line: &str) -> (String, String) {
    let chars: Vec<char> = line.chars().collect();
    let mut code = String::new();
    let mut i = 0;
    let mut in_str = false;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
                code.push('"');
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            code.push('"');
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            return (code, chars[i..].iter().collect());
        } else if c == '\'' && chars.get(i + 2) == Some(&'\'') {
            code.push_str("' '");
            i += 3;
            continue;
        } else if c == '\'' && chars.get(i + 1) == Some(&'\\') && chars.get(i + 3) == Some(&'\'') {
            code.push_str("' '");
            i += 4;
            continue;
        } else {
            code.push(c);
        }
        i += 1;
    }
    (code, String::new())
}

fn is_fn_header(code: &str) -> bool {
    let mut rest = code;
    while let Some(p) = rest.find("fn ") {
        let before_ok = !rest[..p].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after = &rest[p + 3..];
        let ident: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if before_ok && !ident.is_empty() && after[ident.len()..].trim_start().starts_with(['(', '<']) {
            return true;
        }
        rest = after;
    }
    false
}

fn parse(src: &str, test_file: bool) -> Parsed {
    let mut p = Parsed { code: Vec::new(), comment: Vec::new(), in_test: Vec::new(), funcs: Vec::new() };
    let mut depth = 0usize;
    let mut attrs_ignore = false;
    let mut cfg_test_next = false;
    let mut test_mod_pending = false;
    let mut test_mod_depth: Option<usize> = None;
    // (start line, ignored) of a fn header whose body brace has not opened yet.
    let mut pending: Option<(usize, bool)> = None;
    // (start line, ignored, depth after the body brace opened)
    let mut stack: Vec<(usize, bool, usize)> = Vec::new();
    for (n, raw) in src.lines().enumerate() {
        let (code, comment) = split_line(raw);
        let trimmed = code.trim();
        p.in_test.push(test_file || test_mod_depth.is_some());
        if trimmed.starts_with("#[") {
            attrs_ignore |= trimmed.starts_with("#[ignore");
            cfg_test_next |= trimmed.starts_with("#[cfg(test)]");
        } else if !trimmed.is_empty() {
            if cfg_test_next && trimmed.contains("mod ") && !trimmed.ends_with(';') {
                test_mod_pending = true;
            }
            if pending.is_none() && is_fn_header(trimmed) {
                pending = Some((n, attrs_ignore));
            }
            attrs_ignore = false;
            cfg_test_next = false;
        }
        for c in code.chars() {
            match c {
                '{' => {
                    depth += 1;
                    if test_mod_pending {
                        test_mod_pending = false;
                        test_mod_depth = Some(depth);
                    }
                    if let Some((start, ignored)) = pending.take() {
                        stack.push((start, ignored, depth));
                    }
                }
                '}' => {
                    if let Some(&(start, ignored, open)) = stack.last()
                        && open == depth
                    {
                        stack.pop();
                        p.funcs.push(Func { start, end: n, ignored });
                    }
                    if test_mod_depth == Some(depth) {
                        test_mod_depth = None;
                    }
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
        }
        // A bodiless fn (trait declaration) never opens a brace.
        if pending.is_some() && trimmed.ends_with(';') {
            pending = None;
        }
        p.code.push(code);
        p.comment.push(comment);
    }
    p
}

fn has_marker(p: &Parsed, i: usize) -> bool {
    let ok = |s: &str| {
        s.find("// timing:").is_some_and(|at| {
            let rest = s[at + "// timing:".len()..].trim();
            TAGS.iter().any(|t| rest.strip_prefix(t).is_some_and(|r| !r.trim().is_empty()))
        })
    };
    ok(&p.comment[i]) || (i > 0 && p.code[i - 1].trim().is_empty() && ok(&p.comment[i - 1]))
}

/// A measurement ceiling: a short duration on the bound side of `x < D`, `x <= D`, `D > x` or `D >= x`.
/// Lower bounds (`x >= D`) cannot flake on a slow runner, and a sleep or timeout argument is not a bound.
/// `short` holds the names bound to a short duration in the file (`short_names`), so a bound kept in a
/// named constant is caught too.
fn short_ceiling(code: &str, short: &[String]) -> bool {
    if code.contains("sleep(") {
        return false;
    }
    let bound = |side: &str| short_duration(side) || tokens(side).any(|t| short.iter().any(|n| n == t));
    [(" <= ", false), (" < ", false), (" >= ", true), (" > ", true)]
        .iter()
        .any(|(op, left)| code.split_once(op).is_some_and(|(l, r)| bound(if *left { l } else { r })))
}

/// Names a `const`, `static` or `let` binds to a short duration: `const BOUND: Duration = Duration::from_millis(50);`
/// (or an `if` choosing between them). A duration passed to a call (`let rig = slow(Duration::from_millis(50))`) is
/// not a bound, and a marker on the binding covers every use of the name.
fn short_names(p: &Parsed) -> Vec<String> {
    let mut out = Vec::new();
    for (i, code) in p.code.iter().enumerate() {
        let Some((lhs, rhs)) = code.split_once(" = ") else { continue };
        let rhs = rhs.trim_start();
        let direct = ["Duration::", "std::time::Duration::", "core::time::Duration::", "if "].iter().any(|s| rhs.starts_with(s));
        if lhs.contains('(') || !direct || !short_duration(rhs) || has_marker(p, i) {
            continue;
        }
        let mut words = tokens(lhs).skip_while(|t| !matches!(*t, "const" | "static" | "let")).skip(1);
        if let Some(name) = words.find(|t| *t != "mut") {
            out.push(name.to_string());
        }
    }
    out
}

/// A `Duration::from_millis(N)` below 100, or any `Duration::from_micros(`.
fn short_duration(code: &str) -> bool {
    if code.contains("Duration::from_micros(") {
        return true;
    }
    let mut rest = code;
    while let Some(p) = rest.find("Duration::from_millis(") {
        let arg = &rest[p + "Duration::from_millis(".len()..];
        let consumed = arg.find(')').unwrap_or(0);
        let digits: String = arg[..consumed].chars().filter(|c| *c != '_').collect();
        if digits.parse::<u64>().is_ok_and(|n| n < 100) {
            return true;
        }
        rest = arg;
    }
    false
}

/// A sleep inside a wait loop bounded by a deadline or clock is polling, not pacing.
fn is_polling_sleep(p: &Parsed, i: usize) -> bool {
    let lo = i.saturating_sub(6);
    p.code[lo..=i].iter().any(|l| l.contains("deadline") || l.contains("Instant::now()") || l.contains(".elapsed() <") || l.contains("until("))
}

fn tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|t| !t.is_empty())
}

/// Names incremented in a fn (`x += ..`, `x.fetch_add(..)`).
fn counters(p: &Parsed, f: &Func) -> Vec<String> {
    let mut out = Vec::new();
    for line in &p.code[f.start..=f.end] {
        for pat in [" += ", ".fetch_add("] {
            if let Some(at) = line.find(pat)
                && let Some(name) = tokens(&line[..at]).last()
            {
                out.push(name.to_string());
            }
        }
    }
    out
}

fn is_measure_assert(code: &str, counters: &[String]) -> bool {
    if !code.contains("assert") {
        return false;
    }
    let word = tokens(code).any(|t| {
        let t = t.to_ascii_lowercase();
        MEASURE_WORDS.iter().any(|w| t.contains(w)) || t == "rate" || (t.ends_with("_rate") && t != "sample_rate")
    });
    let counted = (code.contains(" >= ") || code.contains(" <= ")) && tokens(code).any(|t| counters.iter().any(|c| c == t));
    word || counted
}

/// A literal integer that ends the operand (followed by `,`, `)` or nothing).
fn literal(s: &str) -> Option<u64> {
    let s = s.trim();
    let end = s.find(|c: char| !(c.is_ascii_digit() || c == '_')).unwrap_or(s.len());
    let tail = s[end..].trim_start();
    let digits: String = s[..end].chars().filter(|c| *c != '_').collect();
    if digits.is_empty() || !(tail.is_empty() || tail.starts_with([',', ')'])) {
        return None;
    }
    digits.parse().ok()
}

/// `(expression, literal)` of `assert!(x >= N ..)` or `assert!(N <= x ..)`.
fn floor_parts(code: &str) -> Option<(String, u64)> {
    let args = code.split_once("assert!(")?.1;
    if let Some((l, r)) = args.split_once(" >= ") {
        let l = l.rsplit("&&").next().unwrap_or(l).rsplit("||").next().unwrap_or(l);
        return literal(r).map(|n| (l.to_string(), n));
    }
    if let Some((l, r)) = args.split_once(" <= ") {
        let n = literal(l)?;
        let end = r.find([',', ')']).unwrap_or(r.len());
        return Some((r[..end].to_string(), n));
    }
    None
}

pub fn scan(path: &str, src: &str, test_file: bool) -> Vec<Hit> {
    let p = parse(src, test_file);
    let short = short_names(&p);
    let lines: Vec<&str> = src.lines().collect();
    let mut hits = Vec::new();
    for i in 0..p.code.len() {
        if !p.in_test[i] {
            continue;
        }
        let inside: Vec<&Func> = p.funcs.iter().filter(|f| f.start <= i && i <= f.end).collect();
        if inside.iter().any(|f| f.ignored) || has_marker(&p, i) {
            continue;
        }
        let code = &p.code[i];
        let rule = if short_ceiling(code, &short) {
            Some('a')
        } else if let Some(f) = inside.iter().min_by_key(|f| f.end - f.start) {
            let names = counters(&p, f);
            let body = &p.code[f.start..=f.end];
            if code.contains("thread::sleep(") && !is_polling_sleep(&p, i) && body.iter().any(|l| is_measure_assert(l, &names)) {
                Some('b')
            } else if let Some((x, n)) = floor_parts(code) {
                let named = FLOOR_NAMES.iter().any(|k| x.to_ascii_lowercase().contains(k));
                let timed = body.iter().any(|l| l.contains("Instant::now") || l.contains("sleep(") || l.contains("deadline"));
                let counted = timed && names.iter().any(|c| c == x.trim());
                (n >= 3 && (named || counted)).then_some('c')
            } else {
                None
            }
        } else {
            None
        };
        if let Some(rule) = rule {
            hits.push(Hit { path: path.into(), line: i + 1, rule, text: lines.get(i).copied().unwrap_or("").into() });
        }
    }
    hits
}

fn is_test_file(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let (file, dirs) = parts.split_last().map_or(("", &[][..]), |(f, d)| (*f, d));
    let stem = file.strip_suffix(".rs").unwrap_or(file);
    dirs.contains(&"tests") || stem == "tests" || stem.ends_with("_tests")
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for e in entries {
        let path = e.map_err(|e| e.to_string())?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                walk(&path, out)?;
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
    Ok(())
}

pub fn run(root: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    for top in ["crates", "apps", "tools"] {
        walk(&root.join(top), &mut files)?;
    }
    files.sort();
    let mut hits = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(root).unwrap_or(f).to_string_lossy().replace('\\', "/");
        let src = std::fs::read_to_string(f).map_err(|e| format!("{rel}: {e}"))?;
        hits.extend(scan(&rel, &src, is_test_file(&rel)));
    }
    if hits.is_empty() {
        println!("timing: ok");
        Ok(())
    } else {
        let list: Vec<String> = hits.iter().map(ToString::to_string).collect();
        Err(format!(
            "machine-dependent timing assertions in tests (gates.md, E49); assert a deterministic signal, or mark a deliberate stall bound, median or floor with `// timing: stall-bound|median|floor-ok|sleep-ok|value <reason>`:\n  {}",
            list.join("\n  ")
        ))
    }
}

#[cfg(test)]
#[path = "timing_tests.rs"]
mod tests;
