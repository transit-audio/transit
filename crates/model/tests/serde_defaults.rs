//! The new-field rule of `docs/contributing/knowledge/session-format.md` (#161): a field of a
//! deserialized struct in `crates/model/src` must load when an older file lacks it, so it is
//! `Option<_>`, carries `#[serde(default ...)]` or a skip attribute, or its struct has a
//! container-level `#[serde(default)]`. Fields that are required by design are listed in
//! [`REQUIRED_BY_DESIGN`] (`Struct.field`); adding to that list is a reviewed decision, not a
//! way past the test.
//!
//! This is a line-based scan, not a parser: it reads `#[derive(...Deserialize...)]` structs with
//! named fields, and does not look into enum variants or tuple structs. A field that a custom
//! `Deserialize` impl supplies is out of its reach.

use std::path::Path;

/// Fields written by every build since the first release (or required for the file to mean
/// anything), as `Struct.field`. A file without one is damaged, not older. This is the baseline at
/// v0.3.2: a field added later must be defaulted instead of joining this list.
const REQUIRED_BY_DESIGN: &str = "\nAutomationLane.param
AutomationLane.points
AutomationPoint.at
AutomationPoint.value
Bus.format
Bus.id
Bus.name
Clip.content
Clip.id
Clip.length
Clip.name
Clip.start
ClipLoop.end
ClipLoop.start
EditState.automation_follows_edit
EditState.click
EditState.countoff
EditState.countoff_bars
EditState.delay_compensation
EditState.edit_mode
EditState.grid
EditState.insertion_follows_playback
EditState.keyboard_focus
EditState.layered_editing
EditState.link_timeline_edit
EditState.link_track_edit
EditState.loop_playback
EditState.main_counter
EditState.markers_follow_edit
EditState.midi_merge
EditState.mirrored_midi
EditState.nudge
EditState.playhead
EditState.post_roll
EditState.pre_fader_metering
EditState.pre_post_roll
EditState.pre_roll
EditState.quickpunch
EditState.record_mode
EditState.rulers
EditState.scrolling
EditState.selected_tracks
EditState.selection
EditState.solo_mode
EditState.tab_to_transient
EditState.timeline_selection
EditState.tool
EditState.zoom
ExternalLatency.buffer
ExternalLatency.frames
ExternalLatency.port_name
ExternalLatency.rate
ExternalLatency.ret
Fade.len
Fade.shape
Frozen.hash
Frozen.length
Frozen.source
Frozen.start
Group.active
Group.color
Group.edit
Group.id
Group.letter
Group.members
Group.mix
Group.name
HardwareInsert.ret
HardwareInsert.send
HardwareLatency.rate
HardwareLatency.ret
HardwareLatency.round_trip_frames
HardwareLatency.send
Insert.plugin
IoPath.channels
IoPath.direction
IoPath.format
IoPath.name
KeySource.from
Mapping.source
Mapping.target
MemoryLocation.end
MemoryLocation.id
MemoryLocation.kind
MemoryLocation.name
MemoryLocation.number
MemoryLocation.start
MidiOutPort.backend
MidiOutPort.name
MidiSource.kind
MidiSource.number
MidiSource.port
Mixer.automation_mode
Mixer.input
Mixer.inserts
Mixer.mute
Mixer.output
Mixer.pan
Mixer.sends
Mixer.solo
Mixer.volume_db
OutputPath.first_channel
OutputPath.format
OutputPath.name
Playlist.clips
Playlist.name
SendSlot.level_db
SendSlot.mute
SendSlot.pan
SendSlot.pre_fader
SendSlot.target
Session.bit_depth
Session.busses
Session.edit
Session.format_version
Session.frame_rate
Session.groups
Session.markers
Session.name
Session.next_id
Session.outputs
Session.sample_rate
Session.sources
Session.tempo
Session.timecode_start
Session.tracks
Source.channels
Source.format
Source.frames
Source.id
Source.name
Source.path
Source.sample_rate
Track.active_playlist
Track.automation
Track.color
Track.format
Track.height
Track.id
Track.kind
Track.mixer
Track.name
Track.playlists
Track.view
VideoSource.id
VideoSource.name
VideoSource.path
ZoomState.presets
ZoomState.samples_per_px
ZoomState.scroll
ZoomState.waveform_zoom
ZoomToggle.heights
ZoomToggle.samples_per_px
ZoomToggle.scroll
";

#[derive(Default)]
struct Found {
    violations: Vec<String>,
    structs: usize,
    fields: usize,
}

/// The attribute text accumulated from `#[` to its closing `]`.
fn attr_text(lines: &[&str], i: &mut usize) -> String {
    let mut text = String::new();
    let mut depth = 0i32;
    loop {
        let line = lines[*i];
        text.push_str(line.trim());
        depth += line.matches('[').count() as i32 - line.matches(']').count() as i32;
        *i += 1;
        if depth <= 0 || *i >= lines.len() {
            return text;
        }
    }
}

fn is_default_bearing(attrs: &str) -> bool {
    attrs.contains("serde(") && (attrs.contains("default") || attrs.contains("skip"))
}

fn scan(file: &str, text: &str, found: &mut Found) {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        // Attributes of an item: gather them until the item line.
        if !lines[i].trim_start().starts_with("#[") {
            i += 1;
            continue;
        }
        let mut attrs = String::new();
        while i < lines.len() && (lines[i].trim_start().starts_with("#[") || lines[i].trim_start().starts_with("///")) {
            if lines[i].trim_start().starts_with("#[") {
                attrs.push_str(&attr_text(&lines, &mut i));
            } else {
                i += 1;
            }
        }
        let Some(head) = lines.get(i).map(|l| l.trim_start()) else { break };
        let head = head.strip_prefix("pub ").or_else(|| head.strip_prefix("pub(crate) ")).unwrap_or(head);
        let Some(rest) = head.strip_prefix("struct ") else { continue };
        if !attrs.contains("Deserialize") || !head.trim_end().ends_with('{') {
            continue;
        }
        let name = rest.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("");
        let container_default = attrs.contains("serde(default") || attrs.contains("default)") && attrs.contains("serde(");
        let container_conversion = attrs.contains("from =") || attrs.contains("try_from") || attrs.contains("transparent");
        found.structs += 1;
        i += 1;
        let mut field_attrs = String::new();
        while i < lines.len() && !lines[i].starts_with("}") && !lines[i].trim_start().starts_with('}') {
            let t = lines[i].trim_start();
            if t.starts_with("#[") {
                field_attrs.push_str(&attr_text(&lines, &mut i));
                continue;
            }
            if t.starts_with("//") || t.is_empty() {
                i += 1;
                continue;
            }
            let t = t.strip_prefix("pub(crate) ").or_else(|| t.strip_prefix("pub ")).unwrap_or(t);
            if let Some((field, ty)) = t.split_once(':')
                && field.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                found.fields += 1;
                let ok = container_default
                    || container_conversion
                    || is_default_bearing(&field_attrs)
                    || ty.trim_start().starts_with("Option<")
                    || REQUIRED_BY_DESIGN.split_whitespace().any(|r| r == format!("{name}.{field}"));
                if !ok {
                    found.violations.push(format!("{file}: {name}.{field}"));
                }
            }
            field_attrs.clear();
            i += 1;
        }
    }
}

#[test]
fn every_deserialized_field_loads_when_an_older_file_lacks_it() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Found::default();
    let mut files: Vec<_> =
        std::fs::read_dir(&src).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "rs")).collect();
    files.sort();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        // Unit tests sit at the end of a file and declare throwaway structs.
        let body = text.split("#[cfg(test)]").next().unwrap_or(&text);
        scan(path.file_name().unwrap().to_str().unwrap(), body, &mut found);
    }
    assert!(
        found.structs > 15 && found.fields > 80,
        "the scan found {} structs and {} fields: has it stopped reading the source?",
        found.structs,
        found.fields
    );
    found.violations.sort();
    println!("{} structs, {} fields", found.structs, found.fields);
    assert!(
        found.violations.is_empty(),
        "these fields have no #[serde(default)], are not Option<_>, and are not in REQUIRED_BY_DESIGN; an older session without them would fail to load:\n{}",
        found.violations.join("\n")
    );
}

#[test]
fn the_required_list_names_only_fields_that_exist() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut text = String::new();
    for e in std::fs::read_dir(src).unwrap() {
        text.push_str(&std::fs::read_to_string(e.unwrap().path()).unwrap());
    }
    for entry in REQUIRED_BY_DESIGN.split_whitespace() {
        let (st, field) = entry.split_once('.').unwrap();
        assert!(text.contains(&format!("struct {st}")) && text.contains(&format!("{field}:")), "{entry} is listed but no longer exists: remove it");
    }
}
