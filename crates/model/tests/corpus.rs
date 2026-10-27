//! The session compatibility corpus (#161): sessions saved by released builds, kept as files in
//! `tests/corpus/` and loaded by every later build. See `tests/corpus/README.md`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use transit_model::Session;

fn corpus() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "transit")).collect();
    files.sort();
    assert!(files.len() >= 2, "the corpus needs at least the demo and features sessions of one release, found {}", files.len());
    files.into_iter().map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap())).collect()
}

/// Every key path of `v` (`/tracks/0/mixer/volume_db`); array elements by index, null values included.
fn key_paths(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                let p = format!("{path}/{}", k.replace('~', "~0").replace('/', "~1"));
                out.push(p.clone());
                key_paths(x, &p, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                key_paths(x, &format!("{path}/{i}"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn every_corpus_session_loads_and_round_trips_losslessly() {
    for (name, text) in corpus() {
        let first = Session::from_json(&text).unwrap_or_else(|e| panic!("{name} no longer loads: {e}"));
        assert!(first.load_notes.is_empty(), "{name} loads with notes: {:?}", first.load_notes);
        let saved = first.to_json().unwrap_or_else(|e| panic!("{name} no longer saves: {e}"));
        let second = Session::from_json(&saved).unwrap_or_else(|e| panic!("{name} saved by this build does not load: {e}"));
        assert!(second.load_notes.is_empty(), "{name} reloads with notes: {:?}", second.load_notes);
        assert_eq!(first, second, "{name}: save then load changed the session");
    }
}

#[test]
fn no_key_of_a_corpus_file_is_dropped_by_a_round_trip() {
    for (name, text) in corpus() {
        let original: Value = serde_json::from_str(&text).unwrap();
        let resaved: Value = serde_json::from_str(&Session::from_json(&text).unwrap().to_json().unwrap()).unwrap();
        let (mut before, mut after) = (Vec::new(), Vec::new());
        key_paths(&original, "", &mut before);
        key_paths(&resaved, "", &mut after);
        let after: std::collections::HashSet<_> = after.into_iter().collect();
        let missing: Vec<_> = before.iter().filter(|p| !after.contains(*p)).take(20).collect();
        assert!(
            missing.is_empty(),
            "{name}: a save dropped {} key path(s), first: {missing:?}",
            before.iter().filter(|p| !after.contains(*p)).count()
        );
    }
}

#[test]
fn no_corpus_file_is_from_a_newer_format_than_this_build() {
    for (name, text) in corpus() {
        let v: Value = serde_json::from_str(&text).unwrap();
        let ver = v["format_version"].as_u64().unwrap_or_else(|| panic!("{name} has no format_version"));
        assert!(ver <= u64::from(Session::FORMAT_VERSION), "{name} has format_version {ver}, newer than {}", Session::FORMAT_VERSION);
    }
}

#[test]
fn corpus_files_are_named_by_release_and_stay_small() {
    for (name, text) in corpus() {
        let ok = name.strip_prefix('v').and_then(|r| r.strip_suffix(".transit")).is_some_and(|r| {
            r.split_once('-').is_some_and(|(version, kind)| {
                version.split('.').count() == 3 && version.split('.').all(|n| n.parse::<u32>().is_ok()) && !kind.is_empty()
            })
        });
        assert!(ok, "{name}: corpus files are named v<major>.<minor>.<patch>-<name>.transit");
        assert!(text.len() < 400_000, "{name} is {} bytes: a corpus file carries no audio and stays small", text.len());
    }
}
