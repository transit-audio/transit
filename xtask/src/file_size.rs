//! File-size gate (#149): a Rust file may not exceed `MAX_LINES` lines. Files over the budget today sit in
//! `OVER_BUDGET` with their line count; a listed file may only shrink, and the table only ever shrinks.
//!
//! Data tables are the one exemption. A data table is a file that only holds a lookup table ported or generated from a
//! reference implementation (a sample table, a wavetable, a set of inverse-cdf curves): one file per table, named for what
//! it holds, because a number-suffixed split only exists to satisfy this gate. A table file is exempt from `MAX_LINES` when
//! its repo-relative path is a row in `DATA_TABLES` with a one-line reason, and the gate then proves the file is data:
//!
//! 1. Comments (`//`, `///`, `//!`, `/* */`) and the insides of string and character literals are ignored. Raw strings are
//!    refused.
//! 2. What remains is a sequence of items at the top level, each ended by `;` outside any brackets. An attribute
//!    (`#[...]`, `#![...]`) is allowed anywhere between items. Every other item must be a `use`, a `const` or a `static`,
//!    with an optional `pub` or `pub(...)`; anything else (a `fn`, `impl`, `struct`, `enum`, `trait`, `mod`, a block without
//!    a closing `;`, text after the last `;`) fails.
//! 3. Inside an item, none of the words `fn impl match if else loop while for let return break continue macro_rules unsafe
//!    struct enum trait mod extern async await mut` may appear, no macro invocation (`name!`) and no `|` (so no closure).
//!    A call to a `const fn` declared elsewhere (a narrowing helper in the parent module) and arithmetic in a length are
//!    data and pass.
//!
//! A listed file must exist and must be over `MAX_LINES` (a table that fits needs no exemption), and a path may not be in both
//! tables. An exemption is a line in `DATA_TABLES`, reviewed like any other change.

use std::path::Path;
use std::process::Command;

/// The line budget for any Rust file.
pub const MAX_LINES: usize = 1500;

/// Files over `MAX_LINES` today, as (repo-relative path, line budget). Remove a row when its file drops to `MAX_LINES` or fewer.
pub const OVER_BUDGET: &[(&str, usize)] = &[];

/// Data-table files exempt from `MAX_LINES`, as (repo-relative path, one-line reason). Each must pass `check_data_only`.
pub const DATA_TABLES: &[(&str, &str)] = &[];

/// Checks `files` (repo-relative path, line count) against `MAX_LINES`, `table` and `data_tables`. Reports every problem.
pub fn check_file_size(files: &[(String, usize)], table: &[(&str, usize)], data_tables: &[(&str, &str)]) -> Result<(), String> {
    let mut problems = Vec::new();
    for (path, lines) in files {
        let lines = *lines;
        let exempt = data_tables.iter().any(|(p, _)| p == path);
        match table.iter().find(|(p, _)| p == path) {
            Some(_) if exempt => problems.push(format!("{path}: listed in both OVER_BUDGET and DATA_TABLES: a data table is not an over-budget file")),
            Some((_, budget)) if lines > *budget => problems.push(format!(
                "{path}: {lines} lines, over its OVER_BUDGET budget of {budget}; a listed file may only shrink: split it by concern (see #149)"
            )),
            Some((_, budget)) if lines <= MAX_LINES => {
                problems.push(format!("{path}: {lines} lines, at or under {MAX_LINES} (budget {budget}): remove it from OVER_BUDGET"))
            }
            Some(_) => {}
            None if exempt && lines <= MAX_LINES => problems.push(format!(
                "{path}: {lines} lines, at or under {MAX_LINES}: a table that fits the budget needs no exemption, remove it from DATA_TABLES"
            )),
            None if exempt => {}
            None if lines > MAX_LINES => problems.push(format!(
                "{path}: {lines} lines, over the {MAX_LINES}-line budget (not listed in OVER_BUDGET or DATA_TABLES): split it by concern (see #149), or, if it only holds a lookup table, list it in DATA_TABLES"
            )),
            None => {}
        }
    }
    for (path, budget) in table {
        if !files.iter().any(|(p, _)| p == path) {
            problems.push(format!("{path}: listed in OVER_BUDGET (budget {budget}) but the file no longer exists: remove it from OVER_BUDGET"));
        }
    }
    for (path, _) in data_tables {
        if !files.iter().any(|(p, _)| p == path) {
            problems.push(format!("{path}: listed in DATA_TABLES but the file does not exist: fix the path or remove it from DATA_TABLES"));
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("\n")) }
}

/// Words that never appear in a data table's items (rule 3 in the module doc).
const CODE_WORDS: &[&str] = &[
    "fn",
    "impl",
    "match",
    "if",
    "else",
    "loop",
    "while",
    "for",
    "let",
    "return",
    "break",
    "continue",
    "macro_rules",
    "unsafe",
    "struct",
    "enum",
    "trait",
    "mod",
    "extern",
    "async",
    "await",
    "mut",
];

/// Removes comments and the insides of string and character literals (newlines are kept, so line numbers hold).
fn strip(text: &str) -> Result<String, String> {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut line = 1;
    while i < c.len() {
        match c[i] {
            '/' if c.get(i + 1) == Some(&'/') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                let mut depth = 0;
                while i < c.len() {
                    if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        if c[i] == '\n' {
                            out.push('\n');
                            line += 1;
                        }
                        i += 1;
                    }
                }
                if depth != 0 {
                    return Err("an unterminated block comment".into());
                }
            }
            '"' => {
                if matches!(out.chars().last(), Some('r' | '#')) {
                    return Err(format!("line {line}: a raw string"));
                }
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' {
                        i += 1;
                    }
                    if c.get(i) == Some(&'\n') {
                        out.push('\n');
                        line += 1;
                    }
                    i += 1;
                }
                if i >= c.len() {
                    return Err("an unterminated string".into());
                }
                i += 1;
                out.push_str("\"\"");
            }
            '\'' if c.get(i + 1) == Some(&'\\') => {
                i += 3;
                while i < c.len() && c[i] != '\'' {
                    i += 1;
                }
                i += 1;
                out.push_str("' '");
            }
            '\'' if c.get(i + 2) == Some(&'\'') => {
                i += 3;
                out.push_str("' '");
            }
            ch => {
                if ch == '\n' {
                    line += 1;
                }
                out.push(ch);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn is_ident(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Checks one top-level item (text up to its `;`, attributes removed) against rules 2 and 3.
fn check_item(item: &str, line: usize) -> Result<(), String> {
    let item = item.trim();
    let after_pub = item.strip_prefix("pub").map_or(item, |r| {
        let r = r.trim_start();
        match r.strip_prefix('(') {
            Some(inner) => inner.split_once(')').map_or(r, |(_, after)| after),
            None => r,
        }
    });
    let first: String = after_pub.trim_start().chars().take_while(|ch| is_ident(*ch)).collect();
    if !matches!(first.as_str(), "const" | "static" | "use") {
        let shown: String = item.chars().take(40).collect();
        return Err(format!("line {line}: `{shown}` is not a `use`, `const` or `static` item"));
    }
    for word in item.split(|ch: char| !is_ident(ch)).filter(|w| !w.is_empty()) {
        if CODE_WORDS.contains(&word) {
            return Err(format!("line {line}: the word `{word}` in a table item (a data table holds data only)"));
        }
    }
    if item.contains('|') {
        return Err(format!("line {line}: a `|` in a table item (no closures; write a combined value as a literal)"));
    }
    let chars: Vec<char> = item.chars().collect();
    for (at, ch) in chars.iter().enumerate() {
        if *ch == '!' && chars.get(at + 1) != Some(&'=') && chars[..at].iter().rev().find(|x| !x.is_whitespace()).is_some_and(|x| is_ident(*x)) {
            return Err(format!("line {line}: a macro invocation in a table item"));
        }
    }
    Ok(())
}

/// Checks that `text` is a data table (rules 1 to 3 in the module doc). Returns the first offending line.
pub fn check_data_only(text: &str) -> Result<(), String> {
    let text = strip(text)?;
    let c: Vec<char> = text.chars().collect();
    let (mut i, mut depth, mut line) = (0, 0_i32, 1);
    let (mut item, mut item_line) = (String::new(), 1);
    while i < c.len() {
        let ch = c[i];
        if depth == 0 && item.trim().is_empty() && ch == '#' {
            i += 1;
            if c.get(i) == Some(&'!') {
                i += 1;
            }
            while c.get(i).is_some_and(|x| x.is_whitespace()) {
                if c[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            if c.get(i) != Some(&'[') {
                return Err(format!("line {line}: a `#` that does not start an attribute"));
            }
            let mut inner = 0;
            while i < c.len() {
                match c[i] {
                    '[' => inner += 1,
                    ']' => inner -= 1,
                    '\n' => line += 1,
                    _ => {}
                }
                i += 1;
                if inner == 0 {
                    break;
                }
            }
            if inner != 0 {
                return Err("an unterminated attribute".into());
            }
            item.clear();
            continue;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return Err(format!("line {line}: an unbalanced closing bracket"));
        }
        if ch == ';' && depth == 0 {
            check_item(&item, item_line)?;
            item.clear();
            i += 1;
            continue;
        }
        if ch == '\n' {
            line += 1;
        }
        if item.trim().is_empty() && !ch.is_whitespace() {
            item_line = line;
        }
        item.push(ch);
        i += 1;
    }
    if depth != 0 {
        return Err("unbalanced brackets at the end of the file".into());
    }
    if !item.trim().is_empty() {
        return Err(format!("line {item_line}: text that is not a `;`-terminated `use`, `const` or `static` item"));
    }
    Ok(())
}

pub fn count_lines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| **b == b'\n').count()
}

pub fn run(root: &Path) -> Result<(), String> {
    let out = Command::new("git").current_dir(root).args(["ls-files", "-z", "--", "*.rs"]).output().map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let mut files = Vec::new();
    for path in String::from_utf8_lossy(&out.stdout).split('\0').filter(|p| !p.is_empty()) {
        if !["crates/", "apps/", "tools/", "xtask/"].iter().any(|dir| path.starts_with(dir)) {
            continue;
        }
        // A tracked file deleted in the working tree is reported by the "no longer exists" rule only if listed.
        if let Ok(bytes) = std::fs::read(root.join(path)) {
            files.push((path.to_string(), count_lines(&bytes)));
        }
    }
    let mut problems = Vec::new();
    if let Err(e) = check_file_size(&files, OVER_BUDGET, DATA_TABLES) {
        problems.push(e);
    }
    for (path, reason) in DATA_TABLES {
        let Ok(bytes) = std::fs::read(root.join(path)) else { continue };
        if let Err(e) = check_data_only(&String::from_utf8_lossy(&bytes)) {
            problems.push(format!("{path}: listed in DATA_TABLES ({reason}) but it is not data only: {e}; move the code out, or remove the row"));
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    println!("file-size: ok ({} files, {} over budget, {} data tables)", files.len(), OVER_BUDGET.len(), DATA_TABLES.len());
    Ok(())
}
