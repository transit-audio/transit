//! Help catalog gate (#178): `cargo xtask help [--check|--write]`.
//!
//! The catalog is `docs/help/*.toml` (one file per area); the registered ids are the
//! `Variant => "key",` lines of `crates/ui-studio/src/help/ids.rs`. The gate fails when
//!
//! - a registered id has no entry (the widget would show nothing),
//! - an entry has no registered id, or its id is never used by a widget (a dead entry),
//! - an entry has no name or no text, or an id or key appears twice,
//! - a catalog file does not parse,
//! - a docs page carries a generated section (`<!-- help:AREA -->` ... `<!-- /help:AREA -->`) that
//!   differs from what the catalog says. `--write` rewrites those sections; the default is `--check`.
//!
//! One source feeds the Info panel and the troubleshooting page: a figure is defined once, here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One catalog entry as the gate sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub file: String,
    pub area: String,
    /// The docs page that carries this area's generated section, if any.
    pub page: Option<String>,
    pub id: String,
    pub name: String,
    pub text: String,
}

#[derive(serde::Deserialize)]
struct File {
    area: String,
    page: Option<String>,
    #[serde(default)]
    entry: Vec<Raw>,
}

#[derive(serde::Deserialize)]
struct Raw {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    text: String,
}

/// Parse one catalog file (`file` is its name, for messages).
pub fn parse_catalog(file: &str, text: &str) -> Result<Vec<Entry>, String> {
    let parsed: File = toml::from_str(text).map_err(|e| format!("{file}: {e}"))?;
    Ok(parsed
        .entry
        .into_iter()
        .map(|e| Entry { file: file.to_owned(), area: parsed.area.clone(), page: parsed.page.clone(), id: e.id, name: e.name, text: e.text })
        .collect())
}

/// The `(variant, key)` pairs of `ids.rs`.
pub fn parse_ids(source: &str) -> Vec<(String, String)> {
    source
        .lines()
        .filter_map(|line| {
            let (variant, key) = line.trim().split_once(" => ")?;
            let key = key.strip_suffix(',')?.strip_prefix('"')?.strip_suffix('"')?;
            let ident =
                !variant.is_empty() && variant.chars().all(|c| c.is_ascii_alphanumeric()) && variant.starts_with(|c: char| c.is_ascii_uppercase());
            ident.then(|| (variant.to_owned(), key.to_owned()))
        })
        .collect()
}

/// Whether `source` names `Id::<variant>` as a whole word.
fn mentions(source: &str, variant: &str) -> bool {
    let needle = format!("Id::{variant}");
    source.match_indices(&needle).any(|(at, _)| !source[at + needle.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
}

/// The markdown of one area's generated section: a bullet per entry, in catalog order.
pub fn section_body(entries: &[&Entry]) -> String {
    entries.iter().map(|e| format!("- **{}.** {}\n", e.name, e.text)).collect()
}

fn markers(area: &str) -> (String, String) {
    (format!("<!-- help:{area} -->"), format!("<!-- /help:{area} -->"))
}

/// `page` with the generated section of `area` replaced by `body`; `None` when the markers are missing.
pub fn fill_section(page: &str, area: &str, body: &str) -> Option<String> {
    let (open, close) = markers(area);
    let start = page.find(&open)? + open.len();
    let end = start + page[start..].find(&close)?;
    Some(format!("{}\n{}{}", &page[..start], body, &page[end..]))
}

/// All problems with the catalog, the ids and the sources. `sources` are the Rust files of the UI
/// crate (path, text) other than `ids.rs`; `pages` are the docs pages (path under `docs/`, text).
pub fn problems(entries: &[Entry], ids: &[(String, String)], sources: &[(String, String)], pages: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for e in entries {
        if !seen.insert(e.id.as_str()) {
            out.push(format!("{}: entry {} appears twice", e.file, e.id));
        }
        if e.name.trim().is_empty() || e.text.trim().is_empty() {
            out.push(format!("{}: entry {} needs a name and a text", e.file, e.id));
        }
    }
    let mut keys = BTreeSet::new();
    for (variant, key) in ids {
        if !keys.insert(key.as_str()) {
            out.push(format!("ids.rs: key {key} appears twice"));
        }
        if !entries.iter().any(|e| &e.id == key) {
            out.push(format!("ids.rs: Id::{variant} (\"{key}\") has no entry in docs/help/: add it in the same commit as the widget"));
        } else if !sources.iter().any(|(_, text)| mentions(text, variant)) {
            out.push(format!(
                "docs/help: entry {key} is unused: nothing in crates/ui-studio registers Id::{variant}; use it or delete the entry and the id"
            ));
        }
    }
    for e in entries {
        if !ids.iter().any(|(_, key)| key == &e.id) {
            out.push(format!("{}: entry {} is unused: it has no Id in crates/ui-studio/src/help/ids.rs", e.file, e.id));
        }
    }
    // Generated sections: every area that names a page must find its markers there, filled as the catalog says.
    let mut areas: BTreeMap<(&str, &str), Vec<&Entry>> = BTreeMap::new();
    for e in entries {
        if let Some(page) = &e.page {
            areas.entry((page.as_str(), e.area.as_str())).or_default().push(e);
        }
    }
    for ((page, area), group) in areas {
        let Some(text) = pages.get(page) else {
            out.push(format!("docs/{page}: not found, but the {area} catalog generates a section there"));
            continue;
        };
        match fill_section(text, area, &section_body(&group)) {
            None => out.push(format!("docs/{page}: needs the markers <!-- help:{area} --> and <!-- /help:{area} --> around the {area} figures")),
            Some(filled) if &filled != text => {
                out.push(format!("docs/{page}: the {area} section differs from docs/help: run `cargo xtask help --write`"))
            }
            Some(_) => {}
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A test file does not count as a widget registering an id.
fn is_test_file(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("tests.rs") || n == "tests.rs")
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let write = args.iter().any(|a| a == "--write");
    let help_dir = root.join("docs/help");
    let mut names: Vec<_> = std::fs::read_dir(&help_dir)
        .map_err(|e| format!("{}: {e}", help_dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    names.sort();
    let mut entries = Vec::new();
    for path in &names {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        entries.extend(parse_catalog(name, &text)?);
    }
    let ids_path = root.join("crates/ui-studio/src/help/ids.rs");
    let ids = parse_ids(&std::fs::read_to_string(&ids_path).map_err(|e| format!("{}: {e}", ids_path.display()))?);
    let mut files = Vec::new();
    rust_files(&root.join("crates/ui-studio/src"), &mut files);
    let mut sources = Vec::new();
    for path in files.iter().filter(|p| !is_test_file(p) && p.file_name().is_some_and(|n| n != "ids.rs")) {
        sources.push((path.display().to_string(), std::fs::read_to_string(path).unwrap_or_default()));
    }
    // Pages are compared with LF line ends and written back with the ends they had.
    let mut pages = BTreeMap::new();
    let mut crlf = BTreeSet::new();
    for page in entries.iter().filter_map(|e| e.page.clone()).collect::<BTreeSet<_>>() {
        if let Ok(text) = std::fs::read_to_string(root.join("docs").join(&page)) {
            if text.contains("\r\n") {
                crlf.insert(page.clone());
            }
            pages.insert(page, text.replace("\r\n", "\n"));
        }
    }
    if write {
        let mut groups: BTreeMap<(String, String), Vec<&Entry>> = BTreeMap::new();
        for e in &entries {
            if let Some(page) = &e.page {
                groups.entry((page.clone(), e.area.clone())).or_default().push(e);
            }
        }
        for ((page, area), group) in groups {
            let Some(text) = pages.get(&page) else { continue };
            if let Some(filled) = fill_section(text, &area, &section_body(&group)) {
                if &filled != text {
                    let on_disk = if crlf.contains(&page) { filled.replace('\n', "\r\n") } else { filled.clone() };
                    std::fs::write(root.join("docs").join(&page), on_disk).map_err(|e| format!("docs/{page}: {e}"))?;
                    println!("help: wrote the {area} section of docs/{page}");
                }
                pages.insert(page, filled);
            }
        }
    }
    let found = problems(&entries, &ids, &sources, &pages);
    if found.is_empty() {
        println!("help: {} entries in {} areas, {} ids, all used; generated sections current", entries.len(), names.len(), ids.len());
        Ok(())
    } else {
        Err(format!("the help catalog has {} problem(s):\n  {}", found.len(), found.join("\n  ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> Entry {
        Entry { file: "x.toml".into(), area: "x".into(), page: None, id: id.into(), name: "Name".into(), text: "Text.".into() }
    }

    fn ids(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(v, k)| ((*v).into(), (*k).into())).collect()
    }

    fn sources(text: &str) -> Vec<(String, String)> {
        vec![("widget.rs".into(), text.into())]
    }

    #[test]
    fn a_complete_catalog_passes() {
        let found = problems(&[entry("a.one")], &ids(&[("AOne", "a.one")]), &sources("help::hover(ui, &r, Id::AOne);"), &BTreeMap::new());
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_registered_id_without_an_entry_fails() {
        let found = problems(&[entry("a.one")], &ids(&[("AOne", "a.one"), ("ATwo", "a.two")]), &sources("Id::AOne Id::ATwo"), &BTreeMap::new());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("Id::ATwo") && found[0].contains("no entry"), "{found:?}");
    }

    #[test]
    fn an_entry_without_an_id_fails_as_unused() {
        let found = problems(&[entry("a.one"), entry("a.dead")], &ids(&[("AOne", "a.one")]), &sources("Id::AOne"), &BTreeMap::new());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("a.dead") && found[0].contains("unused"), "{found:?}");
    }

    #[test]
    fn an_entry_whose_id_no_widget_registers_fails_as_unused() {
        // `Id::AOneMore` is a different id: whole words only.
        let found = problems(&[entry("a.one")], &ids(&[("AOne", "a.one")]), &sources("Id::AOneMore"), &BTreeMap::new());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("a.one") && found[0].contains("unused"), "{found:?}");
    }

    #[test]
    fn an_entry_without_text_or_a_repeated_one_fails() {
        let mut empty = entry("a.one");
        empty.text = " ".into();
        let found = problems(&[empty, entry("a.one")], &ids(&[("AOne", "a.one")]), &sources("Id::AOne"), &BTreeMap::new());
        assert!(found.iter().any(|p| p.contains("needs a name and a text")), "{found:?}");
        assert!(found.iter().any(|p| p.contains("appears twice")), "{found:?}");
    }

    #[test]
    fn ids_come_from_variant_arrow_key_lines_only() {
        let source = "macro_rules! ids {\n    ($($variant:ident => $key:literal,)*) => {};\n}\nids! {\n    // note\n    AOne => \"a.one\",\n    ATwo => \"a.two\",\n}\n";
        assert_eq!(parse_ids(source), ids(&[("AOne", "a.one"), ("ATwo", "a.two")]));
    }

    #[test]
    fn a_generated_section_is_filled_from_the_catalog_and_checked() {
        let mut e = entry("x.one");
        e.page = Some("p.md".into());
        let catalog = [e];
        let table = ids(&[("XOne", "x.one")]);
        let page = "Intro\n<!-- help:x -->\nstale\n<!-- /help:x -->\nOutro\n";
        let pages = BTreeMap::from([("p.md".to_owned(), page.to_owned())]);
        let found = problems(&catalog, &table, &sources("Id::XOne"), &pages);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("cargo xtask help --write"), "{found:?}");
        let filled = fill_section(page, "x", &section_body(&[&catalog[0]])).unwrap_or_default();
        assert_eq!(filled, "Intro\n<!-- help:x -->\n- **Name.** Text.\n<!-- /help:x -->\nOutro\n");
        let pages = BTreeMap::from([("p.md".to_owned(), filled.clone())]);
        assert!(problems(&catalog, &table, &sources("Id::XOne"), &pages).is_empty());
        assert_eq!(fill_section(&filled, "x", &section_body(&[&catalog[0]])).as_deref(), Some(filled.as_str()), "filling twice changes nothing");
    }

    #[test]
    fn a_page_without_the_markers_fails() {
        let mut e = entry("x.one");
        e.page = Some("p.md".into());
        let pages = BTreeMap::from([("p.md".to_owned(), "no markers\n".to_owned())]);
        let found = problems(&[e], &ids(&[("XOne", "x.one")]), &sources("Id::XOne"), &pages);
        assert!(found.iter().any(|p| p.contains("needs the markers")), "{found:?}");
    }

    #[test]
    fn the_repository_catalog_passes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_default();
        assert_eq!(run(&root, &[]), Ok(()));
    }
}
