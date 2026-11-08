use super::*;

const OLD_KNOWN: &str = include_str!("../tests/fixtures/indexes/known-issues-README.md");
const OLD_ADR: &str = include_str!("../tests/fixtures/indexes/adr-README.md");
const OLD_ESCAPES: &str = include_str!("../tests/fixtures/indexes/escapes-README.md");

/// A scratch repository: the three frozen old READMEs plus a stub file for every ADR and escape they link.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("transit-indexes-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [ADR_DIR, ESCAPES_DIR, "docs/known-issues"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join(KNOWN_README), OLD_KNOWN).unwrap();
        fs::write(root.join(ADR_DIR).join("README.md"), OLD_ADR).unwrap();
        fs::write(root.join(ESCAPES_DIR).join("README.md"), OLD_ESCAPES).unwrap();
        for (dir, old) in [(ADR_DIR, OLD_ADR), (ESCAPES_DIR, OLD_ESCAPES)] {
            for line in normalize(old).lines() {
                if let Some(start) = line.find("](") {
                    let target = line[start + 2..].split(')').next().unwrap();
                    if target.ends_with(".md") && !target.contains('/') {
                        fs::write(root.join(dir).join(target), format!("# ADR 0000: stub {target}\n\n- **Status:** stub\n")).unwrap();
                    }
                }
            }
        }
        Scratch(root)
    }

    fn split(name: &str) -> Scratch {
        let scratch = Scratch::new(name);
        split_legacy(&scratch.0).unwrap();
        scratch
    }

    fn entry(&self, name: &str, text: &str) {
        fs::write(self.0.join(KNOWN_ENTRIES).join(name), text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        normalize(&fs::read_to_string(self.0.join(rel)).unwrap())
    }

    /// The entry rules without the link-existence check (the scratch tree lacks the docs the legacy rows link).
    fn rules(&self) -> Result<(), String> {
        kinds(&self.0).map(|_| ())
    }

    fn drop_legacy_entries(&self) {
        for (name, _) in md_files(&self.0.join(KNOWN_ENTRIES)).unwrap() {
            if is_legacy(name.trim_end_matches(".md")) {
                fs::remove_file(self.0.join(KNOWN_ENTRIES).join(name)).unwrap();
            }
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn without_markers(text: &str) -> String {
    normalize(text).lines().filter(|l| *l != BEGIN && *l != END).map(|l| format!("{l}\n")).collect()
}

/// The frozen table as it must come back: identical, except that the one row that lacked its closing pipe (K59)
/// gets it, which is the single deliberate normalization of the conversion.
fn expected(old: &str) -> String {
    normalize(old).lines().map(|l| if l.starts_with("| K59 | ") && !l.ends_with(" |") { format!("{l} |\n") } else { format!("{l}\n") }).collect()
}

fn issue(status: &str, body: &str) -> String {
    render_entry(&[("status", status)], body)
}

/// The ADR table with its third column blanked.
fn mask_adr_status(table: &str) -> String {
    table
        .lines()
        .map(|l| match l.strip_prefix("| [") {
            Some(rest) => format!("| [{}\n", rest.rsplit_once(" | ").map_or(rest, |(head, _)| head)),
            None => format!("{l}\n"),
        })
        .collect()
}

const READMES: [&str; 3] = [KNOWN_README, "docs/adr/README.md", "docs/escapes/README.md"];

#[test]
fn split_then_generate_reproduces_the_old_tables_byte_for_byte() {
    let scratch = Scratch::split("roundtrip");
    for (readme, old) in [(KNOWN_README, OLD_KNOWN), ("docs/adr/README.md", OLD_ADR), ("docs/escapes/README.md", OLD_ESCAPES)] {
        let now = scratch.read(readme);
        assert!(now.contains(BEGIN) && now.contains(END), "{readme} lacks its markers");
        if readme.contains("adr") {
            // The status column is read from each record's own Status line now, not carried from the old table.
            assert_eq!(mask_adr_status(&without_markers(&now)), mask_adr_status(&expected(old)), "{readme} differs outside the status column");
        } else {
            assert_eq!(without_markers(&now), expected(old), "{readme} differs from the table it was split from");
        }
    }
    let entries = md_files(&scratch.0.join(KNOWN_ENTRIES)).unwrap();
    assert_eq!(entries.len(), normalize(OLD_KNOWN).lines().filter(|l| l.starts_with("| ")).count() - 1, "one entry file per row");
    run_generate(&scratch.0, true).unwrap();
}

#[test]
fn links_are_written_from_the_entry_folder_and_rebased_in_the_row() {
    let scratch = Scratch::split("links");
    // K10 links `../adr/0018-plugin-host-thread.md` from the README; from entries/ that is one level deeper.
    let k10 = scratch.read("docs/known-issues/entries/K10.md");
    assert!(k10.contains("](../../adr/0018-plugin-host-thread.md)") && !k10.contains("](../adr/0018"), "{k10}");
    assert_eq!(to_entry_target(&to_readme_target("../x/y.md")), "../x/y.md");
    assert_eq!(to_readme_target("../../adr/a.md#s"), "../adr/a.md#s");
    assert_eq!(to_readme_target("sibling.md"), "entries/sibling.md");
    assert_eq!(to_entry_target("entries/sibling.md"), "sibling.md");
    assert_eq!(map_links("[a](https://x.y/z) [b](#top) [c](../d.md)", &to_readme_target), "[a](https://x.y/z) [b](#top) [c](d.md)");
}

#[test]
fn a_link_that_does_not_resolve_from_the_entry_folder_is_refused() {
    let scratch = Scratch::split("badlink");
    scratch.drop_legacy_entries();
    fs::write(scratch.0.join("docs/known-issues/other.md"), "x\n").unwrap();
    scratch.entry("issue-1.md", &issue("Open", "See [other](../other.md)."));
    check_links(&scratch.0).unwrap();
    // Written from the README's folder, the same link would not resolve from entries/.
    scratch.entry("issue-2.md", &issue("Open", "See [other](other.md)."));
    let err = check_links(&scratch.0).unwrap_err();
    assert!(err.contains("issue-2.md") && err.contains("other.md") && !err.contains("issue-1.md"), "{err}");
    assert!(run_validate(&scratch.0).is_err());
}

#[test]
fn the_check_fails_on_a_drifted_table_and_passes_on_a_fresh_one() {
    let scratch = Scratch::split("drift");
    run_generate(&scratch.0, true).unwrap();
    let path = scratch.0.join(KNOWN_ENTRIES).join("K12.md");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replace("status: Open", "status: Closed")).unwrap();
    let err = run_generate(&scratch.0, true).unwrap_err();
    assert!(err.contains("docs/known-issues/README.md") && err.contains("cargo xtask indexes"), "{err}");
    run_generate(&scratch.0, false).unwrap();
    run_generate(&scratch.0, true).unwrap();
    let readme = scratch.read(KNOWN_README);
    assert!(readme.contains("| K12 | Freeze uses a fixed 2 s tail") && readme.contains("| Closed | Measure or ask"), "{readme}");
}

#[test]
fn regeneration_is_idempotent_and_validate_writes_nothing() {
    let scratch = Scratch::split("idempotent");
    scratch.drop_legacy_entries();
    fs::write(scratch.0.join(KNOWN_README), format!("# Known issues\n\n{BEGIN}\n{END}\n")).unwrap();
    scratch.entry("issue-7.md", &issue("Open", "Fresh."));
    let before = scratch.read(KNOWN_README);
    run_validate(&scratch.0).unwrap();
    assert_eq!(scratch.read(KNOWN_README), before, "--validate leaves the READMEs alone");
    assert!(run_generate(&scratch.0, true).is_err(), "a new entry without a regenerated block is drift");
    run_generate(&scratch.0, false).unwrap();
    let once: Vec<String> = READMES.iter().map(|p| scratch.read(p)).collect();
    run_generate(&scratch.0, false).unwrap();
    let twice: Vec<String> = READMES.iter().map(|p| scratch.read(p)).collect();
    assert_eq!(once, twice, "a second run changes nothing, so the main workflow commits nothing");
    assert!(once[0].contains("| issue-7 | Fresh. | Open |"));
    run_generate(&scratch.0, true).unwrap();
}

#[test]
fn a_new_entry_is_one_file_and_lands_after_the_legacy_rows() {
    let scratch = Scratch::split("new");
    scratch.entry("issue-380.md", &render_entry(&[("status", "Open"), ("follow-up", "Fix it")], "A new defect."));
    scratch.entry("issue-99.md", &issue("Open", "An older issue number."));
    scratch.entry("issue-380-b.md", &issue("Open", "A second one on the same issue."));
    scratch.entry("midi-capture-ring.md", &issue("Open", "A slug."));
    fs::write(scratch.0.join(ESCAPES_DIR).join("issue-401.md"), "# a new escape (#401)\n\nBody.\n").unwrap();
    fs::write(scratch.0.join(ADR_DIR).join("0050-new.md"), "# ADR 0050: New\n\n- **Status:** proposed 2026-11-08 (the maintainer); more.\n").unwrap();
    run_generate(&scratch.0, false).unwrap();
    let known = scratch.read(KNOWN_README);
    let rows: Vec<&str> = known.lines().filter(|l| l.starts_with("| ")).collect();
    let tail = &rows[rows.len() - 5..];
    assert!(tail[0].starts_with("| K119 |"), "legacy rows keep their place: {tail:?}");
    assert_eq!(tail[1], "| issue-99 | An older issue number. | Open |", "a row without a follow-up has three cells");
    assert_eq!(tail[2], "| issue-380 | A new defect. | Open | Fix it |", "digits compare as numbers");
    assert!(tail[3].starts_with("| issue-380-b |") && tail[4].starts_with("| midi-capture-ring |"), "{tail:?}");
    assert!(scratch.read("docs/escapes/README.md").contains(&format!("- [a new escape (#401)](issue-401.md)\n{END}")));
    assert!(scratch.read("docs/adr/README.md").contains(&format!("| [0050](0050-new.md) | New | proposed 2026-11-08 |\n{END}")));
}

#[test]
fn a_wrapped_body_becomes_one_cell_and_a_second_paragraph_or_a_pipe_is_refused() {
    let scratch = Scratch::split("body");
    scratch.entry("wrapped.md", &issue("Open", "A long defect\nwrapped over   three\nlines."));
    run_generate(&scratch.0, false).unwrap();
    assert!(scratch.read(KNOWN_README).contains("| wrapped | A long defect wrapped over three lines. | Open |"));
    scratch.entry("two.md", &issue("Open", "First paragraph.\n\nSecond paragraph."));
    let err = scratch.rules().unwrap_err();
    assert!(err.contains("two.md") && err.contains("one paragraph"), "{err}");
    fs::remove_file(scratch.0.join(KNOWN_ENTRIES).join("two.md")).unwrap();
    scratch.entry("piped.md", &issue("Open", "A cell with a | pipe."));
    let err = scratch.rules().unwrap_err();
    assert!(err.contains("piped.md") && err.contains("`|`"), "{err}");
}

#[test]
fn each_validator_refusal_names_the_file() {
    let scratch = Scratch::split("refusals");
    scratch.rules().unwrap();
    let refused = |name: &str, text: &str, needle: &str| {
        scratch.entry(name, text);
        let err = scratch.rules().unwrap_err();
        assert!(err.contains(name) && err.contains(needle), "{name}: {err}");
        fs::remove_file(scratch.0.join(KNOWN_ENTRIES).join(name)).unwrap();
        scratch.rules().unwrap();
    };
    refused("copied.md", &render_entry(&[("status", "Open"), ("order", "42")], "x"), "`order`");
    refused("Upper_Case.md", &issue("Open", "x"), "kebab-case");
    refused("issue-abc.md", &issue("Open", "x"), "issue number");
    refused("K200.md", &issue("Open", "x"), "kebab-case");
    refused("no-block.md", "Just text.\n", "transit-index");
    refused("no-status.md", &render_entry(&[("follow-up", "x")], "x"), "status");
    refused("unclosed.md", "<!-- transit-index\nstatus: Open\n", "not closed");
}

#[test]
fn ids_are_unique_across_known_issues_and_escapes() {
    let scratch = Scratch::split("unique");
    scratch.entry("issue-380.md", &issue("Open", "x"));
    fs::write(scratch.0.join(ESCAPES_DIR).join("issue-380.md"), "# an escape\n").unwrap();
    let err = scratch.rules().unwrap_err();
    assert!(err.contains("issue-380") && err.contains("issue-<n>-b"), "{err}");
    fs::rename(scratch.0.join(ESCAPES_DIR).join("issue-380.md"), scratch.0.join(ESCAPES_DIR).join("issue-380-b.md")).unwrap();
    scratch.rules().unwrap();
    assert_eq!(escape_id("E45b-midi-position-compatibility"), "E45b");
    assert_eq!(escape_id("issue-380-b"), "issue-380-b");
}

#[test]
fn an_adr_or_escape_takes_its_text_from_its_heading_unless_a_block_overrides_it() {
    let (title, status) =
        adr_index_text("0099-x.md", "# ADR 0099: A title: with a colon\n\n- **Status:** accepted 2026-11-08 (decided); amended later.\n").unwrap();
    assert_eq!((title.as_str(), status.as_str()), ("A title: with a colon", "accepted 2026-11-08; amended later"));
    let over = "<!-- transit-index\ntitle: Short\nstatus: ignored\n-->\n# ADR 0099: Long title\n\n- **Status:** accepted. Then more.\n";
    assert_eq!(adr_index_text("0099-x.md", over).unwrap(), ("Short".to_string(), "accepted".to_string()), "the status is never overridden");
    assert!(adr_index_text("0099-x.md", "# ADR 0099: No status\n").unwrap_err().contains("Status"));
    assert_eq!(escape_title("E1-x.md", "# E1: what escaped (#1)\n").unwrap(), "E1: what escaped (#1)");
    assert_eq!(
        short_status("implemented in `transit` and verified 2026-10-18; **filed upstream**"),
        "implemented in `transit` and verified 2026-10-18"
    );
}

/// The rule this replaced: the first clause only.
fn first_clause_only(line: &str) -> String {
    let end = [line.find(';'), line.find(" ("), line.find(". ")].into_iter().flatten().min().unwrap_or(line.len());
    line[..end].trim().trim_end_matches('.').to_string()
}

#[test]
fn an_amended_record_keeps_its_amendment_marks_in_the_index() {
    // The Status lines of ADRs 0010, 0028, 0041 and 0037 as the records carry them.
    let cases = [
        (
            "accepted 2026-10-18; decisions 1 to 4 (the fonts) amended by [ADR 0045](0045-design-language-in-code.md) on 2026-11-06: IBM Plex Sans and Plex Mono replace Inter",
            "accepted 2026-10-18; decisions 1 to 4 amended by [ADR 0045](0045-design-language-in-code.md) on 2026-11-06",
        ),
        (
            "accepted 2026-10-25 (decided by Derek for v0.3.0, #97); amended by [ADR 0047](0047-device-macro-strip.md): the `selected:` controller-bank targets",
            "accepted 2026-10-25; amended by [ADR 0047](0047-device-macro-strip.md)",
        ),
        (
            "accepted 2026-11-04 (proposed 2026-11-01; decided by the maintainer; slices filed); amended 2026-11-03; amended 2026-11-08, accepted",
            "accepted 2026-11-04; amended 2026-11-03; amended 2026-11-08, accepted",
        ),
        ("accepted; amended 2026-11-03 (two tiers of portability)", "accepted; amended 2026-11-03"),
        (
            "accepted 2026-10-21 (direction; review resolved); amendment 2026-11-04, Low Latency mode proposed",
            "accepted 2026-10-21; amendment 2026-11-04, Low Latency mode proposed",
        ),
    ];
    for (line, want) in cases {
        assert_eq!(short_status(line), want, "{line}");
    }
    // Control: the first-clause-only rule drops every amendment mark, so the same cases fail under it.
    for (line, want) in &cases[..4] {
        assert_ne!(first_clause_only(line), *want, "the old rule must fail this case: {line}");
        assert!(!first_clause_only(line).contains("amend"));
    }
    // An amendment written as a sentence is kept too (ADR 0029).
    assert_eq!(
        short_status("accepted 2026-10-29 (issue #124). Slice 1 is implemented. Amended 2026-10-29 for #174: scheduling."),
        "accepted 2026-10-29; Amended 2026-10-29 for #174"
    );
    // A clause that merely mentions work done is not an amendment mark.
    assert_eq!(short_status("accepted 2026-10-18; implemented: merged at 23ad6fd; CI green"), "accepted 2026-10-18");
}

#[test]
fn index_block_values_are_taken_verbatim_and_crlf_is_read() {
    let text = "<!-- transit-index\r\nstatus: Open: `a` (#1)\r\norder: 3\r\n-->\r\n\r\nBody `x`: y.\r\n";
    let entry = parse_entry("K1.md", text).unwrap();
    assert_eq!(entry.get("status"), Some("Open: `a` (#1)"));
    assert_eq!(entry.get("order"), Some("3"));
    assert_eq!(entry.body, "Body `x`: y.");
}

#[test]
fn a_readme_without_markers_is_refused_and_the_block_is_replaced_in_place() {
    assert!(splice("# Known issues\n", &[]).unwrap_err().contains("begin"));
    assert_eq!(splice(&format!("a\n{BEGIN}\nold\n{END}\nb\n"), &["new".to_string()]).unwrap(), format!("a\n{BEGIN}\nnew\n{END}\nb\n"));
}

#[test]
fn splitting_twice_is_refused() {
    let scratch = Scratch::split("twice");
    assert!(split_legacy(&scratch.0).unwrap_err().contains("already has the markers"));
}
