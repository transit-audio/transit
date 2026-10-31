//! The help catalog: `docs/help/*.toml` compiled in with `include_str!`, parsed once on first use
//! (no file is read at run time). One file per area; each entry has an id, a name, one to three
//! plain sentences, an optional key combination and an optional docs link.

use super::Id;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Where the docs live on the web; an entry's `link` is a path under `docs/`.
pub const DOCS_URL: &str = "https://github.com/transit-audio/transit/blob/main/docs/";

/// The area files, in catalog order. `cargo xtask help` reads the same directory, so a new area
/// is one file there and one line here.
const FILES: &[&str] = &[
    include_str!("../../../../docs/help/transport.toml"),
    include_str!("../../../../docs/help/arrange-header.toml"),
    include_str!("../../../../docs/help/mixer-strip.toml"),
    include_str!("../../../../docs/help/device-chain.toml"),
    include_str!("../../../../docs/help/filter.toml"),
    include_str!("../../../../docs/help/macro-osc.toml"),
    include_str!("../../../../docs/help/telemetry.toml"),
    include_str!("../../../../docs/help/audio-devices.toml"),
    include_str!("../../../../docs/help/midi-monitor.toml"),
    include_str!("../../../../docs/help/midi-mappings.toml"),
];

#[derive(serde::Deserialize)]
struct File {
    area: String,
    link: Option<String>,
    #[serde(default)]
    entry: Vec<Raw>,
}

#[derive(serde::Deserialize)]
struct Raw {
    id: String,
    name: String,
    text: String,
    key: Option<String>,
    link: Option<String>,
}

/// One entry of the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub area: String,
    pub name: String,
    pub text: String,
    /// The key combination, as the shortcut table spells it ("Space", "R or F9").
    pub key: Option<String>,
    /// A path under `docs/`, with an optional anchor.
    pub link: Option<String>,
}

impl Entry {
    /// The page "More" opens.
    pub fn url(&self) -> Option<String> {
        self.link.as_ref().map(|l| format!("{DOCS_URL}{l}"))
    }
}

fn table() -> &'static HashMap<String, Entry> {
    static TABLE: OnceLock<HashMap<String, Entry>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::new();
        for text in FILES {
            match toml::from_str::<File>(text) {
                Ok(file) => {
                    for e in file.entry {
                        let entry =
                            Entry { area: file.area.clone(), name: e.name, text: e.text, key: e.key, link: e.link.or_else(|| file.link.clone()) };
                        map.insert(e.id, entry);
                    }
                }
                // A broken file is a build mistake the tests catch; the app keeps running without it.
                Err(e) => log::error!("help catalog: {e}"),
            }
        }
        map
    })
}

/// The entry for `id`, if the catalog has one (a test requires that every id does).
pub fn entry(id: Id) -> Option<&'static Entry> {
    table().get(id.key())
}

/// Every entry key in the catalog, for the tests.
#[cfg(test)]
pub fn keys() -> impl Iterator<Item = &'static str> {
    table().keys().map(String::as_str)
}

/// The entries of one area in file order are the xtask's job; the app only needs lookups.
#[cfg(test)]
pub fn area_of(id: Id) -> Option<&'static str> {
    entry(id).map(|e| e.area.as_str())
}
