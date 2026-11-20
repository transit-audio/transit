//! Links other clients made to transit's ports survive transit's own reopens (ADR 0015, amendment
//! #431, rule 6). transit makes only its own links and binds no disconnect call, but closing its
//! client drops every link of its ports (jack.h, `jack_deactivate`: "disconnect all ports belonging
//! to it"), and every reopen closes the client. So one reopen operation reads the links of the old
//! client's ports before the first deactivation ([`take`]), holds the snapshot while the old
//! client closes, and restores it once on the replacement client after it started ([`restore`]).
//!
//! - **Ends.** A transit end is stored as direction plus local port index ([`Local`]) and mapped to
//!   the replacement client's actual port names, so a client the server renamed (`transit-02`)
//!   gets its links back. A remote end is stored by its exact full name and checked, before the
//!   connect, to exist as an audio port of the direction the link needs.
//! - **transit's own links** (the group's i-th capture port to `in_i`, `out_i` to its i-th playback
//!   port) are not saved, even if another client made them: the replacement makes its own links
//!   from its own configuration (none with `TRANSIT_JACK_AUTOCONNECT=0`, so then every link is
//!   saved as an outside link).
//! - **Meaning.** Each direction's endpoint and, per port, the device port that channel stands for
//!   ([`Meaning`]) are saved with the links. A link is restored only where the replacement port has
//!   the same endpoint and the same channel meaning: a changed input or output device drops that
//!   direction's links, a port beyond the new port count drops its links, each with a diagnostic.
//!   The comparison is per channel, so an endpoint whose channels change meaning one by one (the
//!   source list of slice C, where an edit changes some positions) drops only the links of the
//!   positions that changed: slice C fills `channels` with each entry's source name.
//! - **Restoring.** A link that already exists counts as restored: it is looked for among the
//!   port's links first (PipeWire's libjack makes a duplicate for a connect of an existing pair
//!   rather than answering `EEXIST`), and a connect that answers `EEXIST` (JACK 2, when another
//!   client made the link in between) is a success too. A remote port that
//!   is gone, or no longer an audio port of the right direction, is a failed restore, logged once
//!   and not retried; so is a connect the server refuses.
//! - **Limits.** Best effort: a graph edit racing with the snapshot or the deactivation cannot be
//!   made atomic by this scheme. After a server shutdown the client is gone before anything can be
//!   read, so nothing is saved and nothing restored.

use crate::api::JackClient;
use std::fmt::Write as _;

/// One of transit's ports: its direction and its index (`in_{index + 1}`, `out_{index + 1}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Local {
    pub(crate) input: bool,
    pub(crate) index: usize,
}

impl Local {
    fn label(self) -> String {
        format!("{}_{}", if self.input { "in" } else { "out" }, self.index + 1)
    }
}

/// The other end of a saved link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Far {
    /// A port of another client, by its exact full name.
    Remote(String),
    /// Another port of transit's own client (an outside patchbay linked `out_j` to `in_i`).
    Local(Local),
}

/// One link that was not transit's own: one of transit's ports and what it was linked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Saved {
    pub(crate) ours: Local,
    pub(crate) far: Far,
}

/// What one direction's ports stood for: the endpoint they were opened on and, per port index, the
/// device port that channel is (empty where the device has none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Meaning {
    pub(crate) endpoint: String,
    pub(crate) channels: Vec<String>,
}

/// One direction of a client as the snapshot and the restore see it.
#[derive(Debug, Clone)]
pub(crate) struct Ports {
    pub(crate) input: bool,
    /// The full names of this direction's ports, by index.
    pub(crate) names: Vec<String>,
    pub(crate) meaning: Meaning,
    /// transit's own (source, destination) links of this direction.
    pub(crate) own: Vec<(String, String)>,
}

/// The links of one reopen operation (the [`transit_audio_backend::LinkSnapshot`] the JACK
/// backend makes): taken from session `from`, consumed once by a later session's client.
#[derive(Debug)]
pub(crate) struct Snapshot {
    pub(crate) from: u64,
    pub(crate) links: Vec<Saved>,
    pub(crate) inputs: Option<Meaning>,
    pub(crate) outputs: Option<Meaning>,
}

impl Snapshot {
    fn meaning(&self, input: bool) -> Option<&Meaning> {
        if input { self.inputs.as_ref() } else { self.outputs.as_ref() }
    }
}

/// Reads every link of `sides`' ports on `client` and keeps those that are not transit's own.
/// A control-thread call, with the session's lock held, before the client is deactivated.
pub(crate) fn take(client: &dyn JackClient, sides: &[Ports], from: u64) -> Snapshot {
    let local = |name: &str| sides.iter().find_map(|side| side.names.iter().position(|n| n == name).map(|index| Local { input: side.input, index }));
    let mut links = Vec::new();
    for side in sides {
        for (index, name) in side.names.iter().enumerate() {
            let ours = Local { input: side.input, index };
            for theirs in client.links(side.input, index) {
                let pair = if side.input { (theirs.clone(), name.clone()) } else { (name.clone(), theirs.clone()) };
                if side.own.contains(&pair) {
                    continue;
                }
                let far = match local(&theirs) {
                    // A link between two of transit's ports is saved once, from its input end.
                    Some(_) if !side.input => continue,
                    Some(other) => Far::Local(other),
                    None => Far::Remote(theirs),
                };
                let saved = Saved { ours, far };
                if !links.contains(&saved) {
                    links.push(saved);
                }
            }
        }
    }
    let meaning = |input: bool| sides.iter().find(|s| s.input == input).map(|s| s.meaning.clone());
    Snapshot { from, links, inputs: meaning(true), outputs: meaning(false) }
}

/// A `jack_get_ports` name pattern (a POSIX extended regular expression, under JACK 2 and
/// PipeWire's libjack alike) that matches `name` and nothing else: anchored, every special
/// character escaped. The caller still compares the names returned exactly.
#[cfg_attr(not(all(target_os = "linux", feature = "libjack")), allow(dead_code))]
pub(crate) fn exact_pattern(name: &str) -> String {
    let mut pattern = String::with_capacity(name.len() + 2);
    pattern.push('^');
    for c in name.chars() {
        if matches!(c, '\\' | '.' | '[' | ']' | '(' | ')' | '*' | '+' | '?' | '{' | '}' | '|' | '^' | '$') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('$');
    pattern
}

/// What a restore did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Restored {
    /// Links made again, or found already made.
    pub(crate) restored: usize,
    /// "in_1 from Player:out_L (the input device changed)": links dropped because the port no
    /// longer means what it meant.
    pub(crate) dropped: Vec<String>,
    /// Links whose remote port is gone or that the server refused.
    pub(crate) failed: Vec<String>,
}

impl Restored {
    /// The diagnostic for the user when anything was not restored.
    pub(crate) fn note(&self) -> Option<String> {
        let lost: Vec<&String> = self.dropped.iter().chain(&self.failed).collect();
        if lost.is_empty() {
            return None;
        }
        let mut text = format!(
            "JACK: {} link{} made outside transit {} not restored after the reopen: ",
            lost.len(),
            if lost.len() == 1 { "" } else { "s" },
            if lost.len() == 1 { "was" } else { "were" }
        );
        const SHOWN: usize = 3;
        for (i, what) in lost.iter().take(SHOWN).enumerate() {
            if i > 0 {
                text.push_str("; ");
            }
            text.push_str(what);
        }
        if lost.len() > SHOWN {
            let _ = write!(text, "; and {} more (see the log)", lost.len() - SHOWN);
        }
        text.push('.');
        Some(text)
    }
}

/// Why the port `end` of a saved link does not keep its meaning on the replacement, if it does not.
fn changed(snapshot: &Snapshot, sides: &[Ports], end: Local) -> Option<String> {
    let direction = if end.input { "input" } else { "output" };
    let (Some(old), Some(new)) = (snapshot.meaning(end.input), sides.iter().find(|s| s.input == end.input)) else {
        return Some(format!("transit has no {direction} ports now"));
    };
    if old.endpoint != new.meaning.endpoint {
        return Some(format!("the {direction} device changed from {} to {}", old.endpoint, new.meaning.endpoint));
    }
    if end.index >= new.names.len() {
        let n = new.names.len();
        return Some(format!("transit now has {n} {direction} port{}", if n == 1 { "" } else { "s" }));
    }
    if old.channels.get(end.index) != new.meaning.channels.get(end.index) {
        return Some(format!("{} now stands for another channel", end.label()));
    }
    None
}

/// Restores `snapshot` on `client`, the replacement client whose directions are `sides`, after it
/// started. Every failure and drop is logged here, once; nothing is retried.
pub(crate) fn restore(client: &mut dyn JackClient, sides: &[Ports], snapshot: &Snapshot) -> Restored {
    let mut done = Restored::default();
    let name = |end: Local| sides.iter().find(|s| s.input == end.input).and_then(|s| s.names.get(end.index)).cloned();
    for link in &snapshot.links {
        let far_text = match &link.far {
            Far::Remote(name) => name.clone(),
            Far::Local(end) => end.label(),
        };
        let what = format!("{} {} {far_text}", link.ours.label(), if link.ours.input { "from" } else { "to" });
        let why = changed(snapshot, sides, link.ours).or_else(|| match &link.far {
            Far::Local(end) => changed(snapshot, sides, *end),
            Far::Remote(_) => None,
        });
        if let Some(why) = why {
            log::warn!("JACK: not restoring the link {what}: {why}");
            done.dropped.push(format!("{what} ({why})"));
            continue;
        }
        let (Some(ours), far) = (name(link.ours), &link.far) else { continue };
        let theirs = match far {
            Far::Remote(remote) => {
                // The remote port must still be an audio port that can feed (or be fed by) ours.
                if !client.has_port(remote, link.ours.input) {
                    log::warn!("JACK: not restoring the link {what}: the port is gone");
                    done.failed.push(format!("{what} (the port is gone)"));
                    continue;
                }
                remote.clone()
            }
            Far::Local(end) => match name(*end) {
                Some(theirs) => theirs,
                None => continue,
            },
        };
        // A link already there counts as restored without a connect: PipeWire's libjack makes a
        // second link for a connect of an existing pair instead of answering EEXIST.
        if client.links(link.ours.input, link.ours.index).contains(&theirs) {
            done.restored += 1;
            continue;
        }
        let (source, destination) = if link.ours.input { (&theirs, &ours) } else { (&ours, &theirs) };
        match client.connect(source, destination) {
            Ok(()) => done.restored += 1,
            Err(e) => {
                log::warn!("JACK: not restoring the link {what}: {e}");
                done.failed.push(format!("{what} ({e})"));
            }
        }
    }
    if !snapshot.links.is_empty() {
        log::info!("JACK: restored {} of {} links made outside transit", done.restored, snapshot.links.len());
    }
    done
}
