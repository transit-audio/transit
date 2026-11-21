//! The interface side of the gate: every string literal in the non-test code of the UI crate and
//! the application, found by walking the real module tree from the crate root.
//!
//! The interface runs a command by naming its id (`app.run("edit.trim", ..)`), whether from a menu
//! row, a shortcut handler, a context menu or a button, so a command id that appears as a literal
//! in shipped UI code is a command the interface can run. A test module (`#[cfg(test)]`, declared
//! with or without `#[path]`) is not walked: a test calling a command does not make it reachable.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Str(String),
    Ident(String),
    Punct(char),
}

/// A small Rust lexer: comments are dropped, literals keep their text, lifetimes vanish.
fn lex(src: &str) -> Vec<Tok> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && c.get(i + 1) == Some(&'*') {
            let mut depth = 1;
            i += 2;
            while i < c.len() && depth > 0 {
                if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if ch == '"' {
            i = string(&c, i, &mut out);
        } else if ch == '\'' {
            if c.get(i + 1) == Some(&'\\') {
                i += 2;
                while i < c.len() && c[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if c.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1; // a lifetime: the name that follows lexes as an identifier
            }
        } else if ch.is_alphanumeric() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            let word: String = c[start..i].iter().collect();
            let raw = matches!(word.as_str(), "r" | "br" | "cr") && matches!(c.get(i), Some('"' | '#'));
            let plain = matches!(word.as_str(), "b" | "c") && c.get(i) == Some(&'"');
            if raw {
                i = raw_string(&c, i, &mut out);
            } else if plain {
                i = string(&c, i, &mut out);
            } else {
                out.push(Tok::Ident(word));
            }
        } else {
            out.push(Tok::Punct(ch));
            i += 1;
        }
    }
    out
}

fn string(c: &[char], mut i: usize, out: &mut Vec<Tok>) -> usize {
    i += 1;
    let mut text = String::new();
    while i < c.len() && c[i] != '"' {
        if c[i] == '\\' {
            i += 1;
        }
        if let Some(&ch) = c.get(i) {
            text.push(ch);
        }
        i += 1;
    }
    out.push(Tok::Str(text));
    i + 1
}

fn raw_string(c: &[char], mut i: usize, out: &mut Vec<Tok>) -> usize {
    let mut hashes = 0;
    while c.get(i) == Some(&'#') {
        hashes += 1;
        i += 1;
    }
    i += 1; // the opening quote
    let start = i;
    while i < c.len() && !(c[i] == '"' && (0..hashes).all(|k| c.get(i + 1 + k) == Some(&'#'))) {
        i += 1;
    }
    out.push(Tok::Str(c[start..i.min(c.len())].iter().collect()));
    i + 1 + hashes
}

/// The index just past the item that starts at `i`: up to a `;` or a balanced `{ }` at depth 0.
fn skip_item(t: &[Tok], mut i: usize) -> usize {
    let mut depth = 0i32;
    while i < t.len() {
        match &t[i] {
            Tok::Punct('(' | '[') => depth += 1,
            Tok::Punct(')' | ']') => depth -= 1,
            Tok::Punct('{') => depth += 1,
            Tok::Punct('}') => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            Tok::Punct(';') if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    i
}

/// The index just past the `]` that closes the `[` at `i`.
fn attr_end(t: &[Tok], mut i: usize) -> usize {
    let mut depth = 0i32;
    while i < t.len() {
        match t[i] {
            Tok::Punct('[') => depth += 1,
            Tok::Punct(']') => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

/// The literals of one file's shipped code, and the modules it declares in other files.
struct Parsed {
    strings: Vec<String>,
    /// `(name, #[path] value)` of each `mod name;` outside `cfg(test)`.
    mods: Vec<(String, Option<String>)>,
}

fn parse(src: &str) -> Parsed {
    let t = lex(src);
    let mut p = Parsed { strings: Vec::new(), mods: Vec::new() };
    let (mut skip_next, mut path) = (false, None::<String>);
    let mut i = 0;
    while i < t.len() {
        if t[i] == Tok::Punct('#') && t.get(i + 1) == Some(&Tok::Punct('[')) {
            let end = attr_end(&t, i + 1);
            let attr = &t[i + 2..end - 1];
            let names = |w: &str| attr.iter().any(|a| *a == Tok::Ident(w.into()));
            if names("cfg") && names("test") && !names("not") {
                skip_next = true;
            }
            if names("path") {
                path = attr.iter().find_map(|a| if let Tok::Str(s) = a { Some(s.clone()) } else { None });
            }
            i = end;
            continue;
        }
        if skip_next {
            i = skip_item(&t, i);
            skip_next = false;
            path = None;
            continue;
        }
        match &t[i] {
            Tok::Str(s) => p.strings.push(s.clone()),
            Tok::Ident(m) if m == "mod" => {
                if let (Some(Tok::Ident(name)), Some(Tok::Punct(';'))) = (t.get(i + 1), t.get(i + 2)) {
                    p.mods.push((name.clone(), path.take()));
                }
            }
            Tok::Punct(';' | '{' | '}') => path = None,
            _ => {}
        }
        i += 1;
    }
    p
}

fn resolve(file: &Path, name: &str, path: Option<&str>) -> PathBuf {
    let dir = file.parent().unwrap();
    let stem = file.file_stem().unwrap().to_string_lossy();
    let root_like = matches!(stem.as_ref(), "lib" | "main" | "mod");
    let mut candidates = Vec::new();
    if let Some(p) = path {
        candidates.push(dir.join(p));
    } else {
        // A file loaded through `#[path]` keeps its children beside it, so try both directories.
        let mut dirs = vec![dir.to_path_buf()];
        if !root_like {
            dirs.insert(0, dir.join(stem.as_ref()));
        }
        for child in dirs {
            candidates.push(child.join(format!("{name}.rs")));
            candidates.push(child.join(name).join("mod.rs"));
        }
    }
    candidates.iter().find(|c| c.is_file()).cloned().unwrap_or_else(|| panic!("cannot find module `{name}` declared in {}", file.display()))
}

/// Every literal in the shipped code of the crate rooted at `root` (a `lib.rs` or `main.rs`),
/// plus the number of source files walked.
pub fn shipped_literals(root: &Path) -> (BTreeSet<String>, usize) {
    let (mut seen, mut literals) = (BTreeSet::new(), BTreeSet::new());
    let mut todo = vec![root.to_path_buf()];
    while let Some(file) = todo.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        let parsed = parse(&std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("read {}: {e}", file.display())));
        literals.extend(parsed.strings);
        for (name, path) in parsed.mods {
            todo.push(resolve(&file, &name, path.as_deref()));
        }
    }
    (literals, seen.len())
}
