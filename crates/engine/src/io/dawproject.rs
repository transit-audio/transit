//! DAWproject import, slice 1 (issue #261): a `.dawproject` opens as a new, untitled session with
//! its tracks (volume, pan, mute, solo), note clips, tempo and meter, markers and MIDI expression.
//! Devices, audio, sends, group tracks and parameter automation are later slices; the open counts
//! every kind it leaves out and says so, so nothing disappears silently.
//!
//! The archive is hostile input, read the way `read_smf` reads a MIDI file: sizes and counts are
//! capped, and a bad value skips its element (counted) or refuses the file, never panics.
//! Times are in beats (quarter notes, the DAWproject default); scopes timed in seconds are skipped.

use super::*;
use roxmltree::{Document, Node, ParsingOptions};
use std::collections::BTreeMap;
use transit_midi::{CtrlEvent, CtrlKind, Note, PPQ, Sequence};

const CMD: &str = "session.open";
/// Largest `project.xml` read, uncompressed (a 30-minute note-only project is about 3 MB).
const MAX_XML_BYTES: u64 = 256 << 20;
/// Most XML nodes parsed.
const MAX_NODES: u32 = 16_000_000;
/// Most notes and controller events in the whole project, loops unrolled.
const MAX_EVENTS: usize = 4_000_000;
/// Most tempo and meter changes (each meter is placed by walking the ones before it).
const MAX_MAP_POINTS: usize = 10_000;
/// Latest time read, in beats (24 hours at 999 BPM is under 1.5 million).
const MAX_BEATS: f64 = 2_000_000.0;

/// Whether `path` names a DAWproject file.
pub fn is_dawproject(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|x| x.eq_ignore_ascii_case("dawproject"))
}

/// Open `path` (a `.dawproject`) as a new, untitled session. Returns what was not imported, one
/// line per kind, also posted as messages.
pub fn open_dawproject(e: &mut Engine, path: &str) -> Result<Vec<String>> {
    let file = std::fs::File::open(path).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    let name = Path::new(path).file_stem().map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned());
    let (s, report) = read_dawproject(file, &name)?;
    log::info!(
        "DAWproject opened: path={path:?} tracks={} clips={} not_imported=[{}]",
        s.tracks.len(),
        s.tracks.iter().map(|t| t.clips().len()).sum::<usize>(),
        report.join(", ")
    );
    e.replace_session(s);
    e.path = None;
    for line in &report {
        e.message(line.clone());
    }
    Ok(report)
}

/// The session in a DAWproject archive, and what was not imported.
pub fn read_dawproject(archive: impl std::io::Read + std::io::Seek, name: &str) -> Result<(Session, Vec<String>)> {
    use std::io::Read as _;
    let zerr = |err: zip::result::ZipError| EngineError::Io(format!("DAWproject: {err}"));
    let mut z = zip::ZipArchive::new(archive).map_err(zerr)?;
    let mut f = z.by_name("project.xml").map_err(zerr)?;
    if f.size() > MAX_XML_BYTES {
        return Err(crate::cmd::bad(CMD, "DAWproject: project.xml is too large"));
    }
    let mut xml = String::new();
    // The declared size is hostile too: never read past the cap.
    f.by_ref().take(MAX_XML_BYTES).read_to_string(&mut xml).map_err(|err| EngineError::Io(format!("DAWproject: project.xml: {err}")))?;
    let doc = Document::parse_with_options(&xml, ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES })
        .map_err(|err| crate::cmd::bad(CMD, format!("DAWproject: project.xml: {err}")))?;
    let mut cx = Cx::default();
    let s = project(&mut cx, doc.root_element(), name)?;
    let report = cx.report();
    Ok((s, report))
}

/// What the import counts as it goes.
#[derive(Default)]
struct Cx {
    skipped: BTreeMap<&'static str, usize>,
    events: usize,
}

impl Cx {
    fn skip(&mut self, what: &'static str, n: usize) {
        if n > 0 {
            *self.skipped.entry(what).or_default() += n;
        }
    }

    fn count(&mut self, n: usize) -> Result<()> {
        self.events = self.events.saturating_add(n);
        if self.events > MAX_EVENTS {
            return Err(crate::cmd::bad(CMD, format!("DAWproject: more than {MAX_EVENTS} notes and controller events")));
        }
        Ok(())
    }

    fn report(&self) -> Vec<String> {
        self.skipped.iter().map(|(what, n)| format!("Not imported from the DAWproject: {n} {what}")).collect()
    }
}

/// A clip read from the arrangement: start and length in ticks, its events relative to its start.
struct ClipIn {
    start: i64,
    len: i64,
    name: String,
    muted: bool,
    seq: Sequence,
}

fn kids<'a, 'i: 'a>(n: Node<'a, 'i>, tag: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| c.has_tag_name(tag))
}

fn child<'a, 'i: 'a>(n: Node<'a, 'i>, tag: &'static str) -> Option<Node<'a, 'i>> {
    kids(n, tag).next()
}

fn num(n: Node, attr: &str) -> Option<f64> {
    n.attribute(attr)?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A time attribute in beats, within range.
fn beats(n: Node, attr: &str) -> Option<f64> {
    num(n, attr).filter(|v| v.abs() <= MAX_BEATS)
}

fn ticks(beats: f64) -> i64 {
    (beats * f64::from(PPQ)).round() as i64
}

fn seconds(n: Node) -> bool {
    n.attribute("timeUnit") == Some("seconds")
}

/// A normalized value as MIDI 0..=127.
fn midi7(v: f64) -> i32 {
    (v.clamp(0.0, 1.0) * 127.0).round() as i32
}

fn project(cx: &mut Cx, root: Node, name: &str) -> Result<Session> {
    if !root.has_tag_name("Project") {
        return Err(crate::cmd::bad(CMD, "DAWproject: project.xml has no <Project>"));
    }
    let mut s = Session::new(name, transit_time::SampleRate::default());
    tempo_map(cx, &mut s, root);
    let mut ids = BTreeMap::new();
    if let Some(st) = child(root, "Structure") {
        for t in kids(st, "Track") {
            track(cx, &mut s, t, &mut ids);
        }
    }
    let arrangement = child(root, "Arrangement");
    let mut clips: BTreeMap<TrackId, (Vec<ClipIn>, Vec<CtrlEvent>)> = BTreeMap::new();
    if let Some(l) = arrangement.and_then(|a| child(a, "Lanes")) {
        if seconds(l) {
            return Err(crate::cmd::bad(CMD, "DAWproject: the arrangement is timed in seconds; transit reads beats only"));
        }
        lanes(cx, l, None, &ids, &mut clips)?;
    }
    for (tid, (mut cs, ctrls)) in clips {
        cs.sort_by_key(|c| c.start);
        distribute(cx, &mut cs, ctrls);
        for c in cs {
            place(&mut s, tid, c)?;
        }
    }
    if let Some(m) = arrangement.and_then(|a| child(a, "Markers")) {
        if seconds(m) {
            cx.skip("markers timed in seconds", kids(m, "Marker").count());
        } else {
            for mk in kids(m, "Marker") {
                let Some(t) = beats(mk, "time") else {
                    cx.skip("markers without a valid time", 1);
                    continue;
                };
                let at = s.tempo.tick_to_samples(ticks(t).max(0), s.sample_rate);
                let mut label = mk.attribute("name").unwrap_or("Marker").to_string();
                transit_model::truncate_chars(&mut label, transit_model::MAX_NAME_CHARS);
                s.add_marker(&label, transit_model::MarkerKind::Marker, at, at);
            }
        }
    }
    Ok(s)
}

/// The transport's tempo and meter, then the arrangement's tempo and time-signature automation.
/// Tempo ramps become steps at their points: transit's tempo map has no ramps.
fn tempo_map(cx: &mut Cx, s: &mut Session, root: Node) {
    let transport = child(root, "Transport");
    if let Some(bpm) = transport.and_then(|t| child(t, "Tempo")).and_then(|t| num(t, "value")) {
        tempo(cx, s, 0, bpm);
    }
    if let Some(m) = transport.and_then(|t| child(t, "TimeSignature")) {
        meter_at(cx, s, 0, m);
    }
    let Some(a) = child(root, "Arrangement") else { return };
    if let Some(p) = child(a, "TempoAutomation") {
        let points: Vec<_> = kids(p, "RealPoint").collect();
        cx.skip("tempo changes past the limit", points.len().saturating_sub(MAX_MAP_POINTS));
        for pt in points.into_iter().take(MAX_MAP_POINTS) {
            match (beats(pt, "time"), num(pt, "value")) {
                (Some(t), Some(bpm)) if !seconds(p) => tempo(cx, s, ticks(t), bpm),
                _ => cx.skip("tempo changes without a valid time or value", 1),
            }
        }
    }
    if let Some(p) = child(a, "TimeSignatureAutomation") {
        let points: Vec<_> = kids(p, "TimeSignaturePoint").collect();
        cx.skip("meter changes past the limit", points.len().saturating_sub(MAX_MAP_POINTS));
        for pt in points.into_iter().take(MAX_MAP_POINTS) {
            match beats(pt, "time") {
                Some(t) if !seconds(p) => meter_at(cx, s, ticks(t), pt),
                _ => cx.skip("meter changes without a valid time", 1),
            }
        }
    }
}

fn tempo(cx: &mut Cx, s: &mut Session, tick: i64, bpm: f64) {
    if s.tempo.set_tempo(tick, bpm).is_err() {
        cx.skip("tempos out of range", 1);
    }
}

fn meter_at(cx: &mut Cx, s: &mut Session, tick: i64, m: Node) {
    let n = m.attribute("numerator").and_then(|v| v.trim().parse::<u32>().ok());
    let d = m.attribute("denominator").and_then(|v| v.trim().parse::<u32>().ok());
    let ok = match (n, d) {
        (Some(n), Some(d)) => s.tempo.set_meter(tick, n, d).is_ok(),
        _ => false,
    };
    if !ok {
        cx.skip("meters out of range", 1);
    }
}

/// One `Track` of the structure (and the tracks nested in it): note tracks become instrument
/// tracks, audio tracks empty audio tracks, a master channel off its defaults a Master track.
fn track<'a>(cx: &mut Cx, s: &mut Session, t: Node<'a, '_>, ids: &mut BTreeMap<&'a str, TrackId>) {
    let ch = child(t, "Channel");
    if let Some(c) = ch {
        cx.skip("devices (plug-ins and built-in)", child(c, "Devices").map_or(0, |d| d.children().filter(Node::is_element).count()));
        cx.skip("sends", child(c, "Sends").map_or(0, |d| kids(d, "Send").count()));
    }
    let types = t.attribute("contentType").unwrap_or("");
    let has = |what: &str| types.split_whitespace().any(|w| w == what);
    let role = ch.and_then(|c| c.attribute("role")).unwrap_or("regular");
    let kind = match role {
        "master" if ch.is_some_and(|c| !default_channel(c)) => Some(TrackKind::Master),
        "master" => None,
        "regular" if has("notes") => Some(TrackKind::Instrument),
        "regular" if has("audio") => Some(TrackKind::Audio),
        _ => {
            cx.skip("group, effect, VCA and other tracks", 1);
            None
        }
    };
    if let Some(kind) = kind {
        let format = if kind == TrackKind::Audio && ch.and_then(|c| c.attribute("audioChannels")) == Some("1") {
            ChannelFormat::Mono
        } else {
            ChannelFormat::Stereo
        };
        let mut name = t.attribute("name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(kind.default_name()).to_string();
        transit_model::truncate_chars(&mut name, transit_model::MAX_NAME_CHARS);
        let id = s.add_track(kind, format, Some(&name));
        if let Some(tr) = s.track_mut(id) {
            if kind == TrackKind::Instrument {
                tr.instrument = Some(transit_model::Insert::new(transit_model::DEFAULT_INSTRUMENT));
            }
            if let Some(c) = ch {
                mixer(cx, &mut tr.mixer, c);
            }
        }
        if let Some(x) = t.attribute("id") {
            ids.insert(x, id);
        }
    }
    for sub in kids(t, "Track") {
        track(cx, s, sub, ids);
    }
}

/// A channel at unity volume, centred and not muted.
fn default_channel(c: Node) -> bool {
    let at = |tag, want: f64| child(c, tag).and_then(|n| num(n, "value")).is_none_or(|v| (v - want).abs() < 1e-6);
    let unity = match child(c, "Volume").and_then(|v| v.attribute("unit")) {
        Some("decibel") => at("Volume", 0.0),
        _ => at("Volume", 1.0),
    };
    unity && at("Pan", 0.5) && child(c, "Mute").and_then(|m| m.attribute("value")) != Some("true")
}

/// A channel's volume (linear or dB), pan (normalized, as a balance on stereo tracks), mute and solo.
fn mixer(cx: &mut Cx, m: &mut transit_model::Mixer, c: Node) {
    if let Some(v) = child(c, "Volume") {
        match (v.attribute("unit"), num(v, "value")) {
            (Some("decibel"), Some(db)) => m.volume_db = db as f32,
            (Some("linear") | None, Some(lin)) => m.volume_db = if lin > 0.0 { (20.0 * lin.log10()) as f32 } else { -144.0 },
            _ => cx.skip("volumes in other units", 1),
        }
        m.volume_db = m.volume_db.clamp(-144.0, 12.0);
    }
    if let Some(p) = child(c, "Pan") {
        match (p.attribute("unit"), num(p, "value")) {
            (Some("normalized") | None, Some(v)) => {
                let b = (2.0 * v - 1.0).clamp(-1.0, 1.0) as f32;
                match m.pan.as_mut_slice() {
                    [one] => *one = b,
                    // Stereo tracks have a panner per side: a balance moves the far side in.
                    [l, r] if b < 0.0 => (*l, *r) = (-1.0, 1.0 + 2.0 * b),
                    [l, r] => (*l, *r) = (-1.0 + 2.0 * b, 1.0),
                    _ => {}
                }
            }
            _ => cx.skip("pans in other units", 1),
        }
    }
    m.mute = child(c, "Mute").and_then(|n| n.attribute("value")) == Some("true");
    m.solo = c.attribute("solo") == Some("true");
}

/// A `Lanes` timeline of the arrangement, scoped to `track` (a `track` attribute rescopes it).
fn lanes(
    cx: &mut Cx,
    l: Node,
    track: Option<Option<TrackId>>,
    ids: &BTreeMap<&str, TrackId>,
    out: &mut BTreeMap<TrackId, (Vec<ClipIn>, Vec<CtrlEvent>)>,
) -> Result<()> {
    if seconds(l) {
        cx.skip("lanes timed in seconds", 1);
        return Ok(());
    }
    // None: not inside any track yet; Some(None): inside a track that was not imported.
    let here = match l.attribute("track") {
        Some(x) => Some(ids.get(x).copied()),
        None => track,
    };
    for c in l.children().filter(Node::is_element) {
        match c.tag_name().name() {
            "Lanes" => lanes(cx, c, here, ids, out)?,
            "Clips" if seconds(c) => cx.skip("clips timed in seconds", kids(c, "Clip").count()),
            "Clips" => {
                for k in kids(c, "Clip") {
                    let Some(clip) = clip(cx, k)? else { continue };
                    match here.flatten() {
                        Some(t) => out.entry(t).or_default().0.push(clip),
                        None => cx.skip("clips on tracks not imported", 1),
                    }
                }
            }
            "Points" => match (midi_points(cx, c, 0, None, 0)?, here.flatten()) {
                (Some(evs), Some(t)) => out.entry(t).or_default().1.extend(evs),
                (Some(_), None) => cx.skip("controller lanes on tracks not imported", 1),
                (None, _) => cx.skip("parameter automation lanes", 1),
            },
            "Audio" | "Warps" => cx.skip("audio clips", 1),
            "Notes" => cx.skip("note timelines outside clips", 1),
            _ => {}
        }
    }
    Ok(())
}

/// An arrangement clip of notes: its window (`playStart`, `duration`) and loop over its content,
/// unrolled. None when it has no note content (audio clips are counted), is linked or is muted
/// inside another clip.
fn clip(cx: &mut Cx, k: Node) -> Result<Option<ClipIn>> {
    if k.attribute("contentTimeUnit") == Some("seconds") {
        cx.skip("clips timed in seconds", 1);
        return Ok(None);
    }
    if k.attribute("reference").is_some() {
        cx.skip("linked clips", 1);
        return Ok(None);
    }
    let play = beats(k, "playStart").unwrap_or(0.0);
    let len = beats(k, "duration").or_else(|| beats(k, "playStop").map(|stop| stop - play));
    let (Some(time), Some(len)) = (beats(k, "time"), len.filter(|l| *l > 0.0)) else {
        cx.skip("clips without a valid time or length", 1);
        return Ok(None);
    };
    let Some(content) = content(cx, k)? else { return Ok(None) };
    let lp = beats(k, "loopStart").zip(beats(k, "loopEnd")).map(|(a, b)| (ticks(a), ticks(b)));
    let seq = window(cx, &content, ticks(play), ticks(len), lp)?;
    let mut name = k.attribute("name").unwrap_or("").to_string();
    transit_model::truncate_chars(&mut name, transit_model::MAX_NAME_CHARS);
    Ok(Some(ClipIn { start: ticks(time), len: ticks(len).max(1), name, muted: k.attribute("enable") == Some("false"), seq }))
}

/// The note and controller events of a clip's content timeline, in content ticks. None when it
/// holds none of them.
fn content(cx: &mut Cx, n: Node) -> Result<Option<Sequence>> {
    let mut seq = Sequence::default();
    let mut any = false;
    for c in n.children().filter(Node::is_element) {
        if seconds(c) {
            cx.skip("timelines timed in seconds", 1);
            continue;
        }
        match c.tag_name().name() {
            "Notes" => {
                notes(cx, c, &mut seq)?;
                any = true;
            }
            "Points" => match midi_points(cx, c, 0, None, 0)? {
                Some(evs) => {
                    seq.ctrls.extend(evs);
                    any = true;
                }
                None => cx.skip("parameter automation lanes", 1),
            },
            "Lanes" => {
                if let Some(inner) = content(cx, c)? {
                    merge(&mut seq, inner, 0);
                    any = true;
                }
            }
            "Clips" => {
                for k in kids(c, "Clip") {
                    if let Some(inner) = clip(cx, k)? {
                        if !inner.muted {
                            merge(&mut seq, inner.seq, inner.start);
                        }
                        any = true;
                    }
                }
            }
            "Audio" | "Warps" => cx.skip("audio clips", 1),
            _ => {}
        }
    }
    Ok(any.then_some(seq))
}

fn merge(into: &mut Sequence, from: Sequence, at: i64) {
    into.notes.extend(from.notes.into_iter().map(|mut n| {
        n.start = n.start.saturating_add(at);
        n
    }));
    into.ctrls.extend(from.ctrls.into_iter().map(|mut e| {
        e.tick = e.tick.saturating_add(at);
        e
    }));
}

/// A timbre point of one note, before the notes that overlap are resolved.
struct Timbre {
    tick: i64,
    note: (i64, u8),
    channel: u8,
    value: i32,
}

/// The notes of a `Notes` timeline with their per-note expression: pressure becomes polyphonic
/// aftertouch; timbre becomes channel CC74, owned by the most recently started note still sounding
/// (the highest pitch among notes that start together), so overlapping notes do not fight over it.
fn notes(cx: &mut Cx, ns: Node, seq: &mut Sequence) -> Result<()> {
    let mut timbre = Vec::new();
    let mut spans = Vec::new();
    for n in kids(ns, "Note") {
        let key = n.attribute("key").and_then(|k| k.trim().parse::<u8>().ok()).filter(|k| *k < 128);
        let channel = n.attribute("channel").map_or(Some(0), |c| c.trim().parse::<u8>().ok()).filter(|c| *c < 16);
        let (Some(t), Some(d), Some(pitch), Some(channel)) = (beats(n, "time"), beats(n, "duration"), key, channel) else {
            cx.skip("notes with bad values", 1);
            continue;
        };
        let (start, length) = (ticks(t), ticks(d).max(1));
        cx.count(1)?;
        seq.notes.push(Note {
            pitch,
            velocity: num(n, "vel").map_or(100, |v| midi7(v).max(1)) as u8,
            release_velocity: num(n, "rel").map_or(64, midi7) as u8,
            channel,
            start,
            length,
        });
        for p in kids(n, "Points") {
            match child(p, "Target").and_then(|t| t.attribute("expression")) {
                Some("timbre") => {
                    let before = timbre.len();
                    for (tick, value) in normalized(cx, p, start)? {
                        timbre.push(Timbre { tick, note: (start, pitch), channel, value: midi7(value) });
                    }
                    if timbre.len() > before {
                        spans.push((start, pitch, start.saturating_add(length), channel));
                    }
                }
                Some("pressure") => match midi_points(cx, p, start, Some(pitch), channel)? {
                    Some(evs) => seq.ctrls.extend(evs),
                    None => cx.skip("per-note gain, pan, transpose and formant curves", 1),
                },
                _ => cx.skip("per-note gain, pan, transpose and formant curves", 1),
            }
        }
    }
    spans.sort_unstable();
    for t in timbre {
        // Owned unless a note that started later (or with it, higher) on its channel still sounds.
        let later = spans.partition_point(|s| (s.0, s.1) <= t.note);
        let taken = spans.get(later..).unwrap_or_default().iter().take_while(|s| s.0 <= t.tick).any(|s| s.3 == t.channel && t.tick < s.2);
        if !taken {
            seq.ctrls.push(CtrlEvent { tick: t.tick, channel: t.channel, kind: CtrlKind::Cc(74), value: t.value });
        }
    }
    Ok(())
}

/// The `(tick, value)` points of a normalized `Points` timeline, `base` ticks added. Non-normalized
/// units give none (counted).
fn normalized(cx: &mut Cx, p: Node, base: i64) -> Result<Vec<(i64, f64)>> {
    if p.attribute("unit").is_some_and(|u| u != "normalized") {
        cx.skip("controller lanes in other units", 1);
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for pt in kids(p, "RealPoint") {
        match (beats(pt, "time"), num(pt, "value")) {
            (Some(t), Some(v)) => out.push((base.saturating_add(ticks(t)), v)),
            _ => cx.skip("controller points with bad values", 1),
        }
    }
    cx.count(out.len())?;
    Ok(out)
}

/// The MIDI events of a `Points` timeline that targets a MIDI expression (`base` ticks added;
/// `key` and `channel` are the note's, for per-note pressure). None when it targets anything
/// else, such as a parameter. Repeated values are left out.
fn midi_points(cx: &mut Cx, p: Node, base: i64, key: Option<u8>, channel: u8) -> Result<Option<Vec<CtrlEvent>>> {
    let Some(target) = child(p, "Target") else { return Ok(None) };
    let attr7 = |name| target.attribute(name).and_then(|v| v.trim().parse::<u8>().ok()).filter(|v| *v < 128);
    let channel = target.attribute("channel").and_then(|c| c.trim().parse::<u8>().ok()).filter(|c| *c < 16).unwrap_or(channel);
    let kind = match (target.attribute("expression"), key) {
        (Some("channelController"), _) => attr7("controller").map(CtrlKind::Cc),
        (Some("channelPressure"), _) => Some(CtrlKind::ChannelPressure),
        (Some("polyPressure"), _) => attr7("key").map(CtrlKind::PolyPressure),
        (Some("pressure"), Some(k)) => Some(CtrlKind::PolyPressure(k)),
        (Some("pitchBend"), _) => Some(CtrlKind::PitchBend),
        (Some("programChange"), _) => Some(CtrlKind::Program),
        _ => None,
    };
    let Some(kind) = kind else { return Ok(None) };
    let mut out: Vec<CtrlEvent> = Vec::new();
    for (tick, v) in normalized(cx, p, base)? {
        let value =
            if kind == CtrlKind::PitchBend { ((2.0 * v.clamp(0.0, 1.0) - 1.0) * 8192.0).round().clamp(-8192.0, 8191.0) as i32 } else { midi7(v) };
        if out.last().is_none_or(|e| e.value != value) {
            out.push(CtrlEvent { tick, channel, kind, value });
        }
    }
    Ok(Some(out))
}

/// The events of `content` seen through a clip: from `play` for `len` ticks, repeating
/// `loop_start..loop_end` when the clip loops. Notes are cut at the window's and the loop's end.
fn window(cx: &mut Cx, content: &Sequence, play: i64, len: i64, lp: Option<(i64, i64)>) -> Result<Sequence> {
    let mut out = Sequence::default();
    let Some((ls, le)) = lp.filter(|(ls, le)| le > ls && play < *le) else {
        pass(cx, content, &mut out, (play, play.saturating_add(len)), 0, len)?;
        return Ok(out);
    };
    pass(cx, content, &mut out, (play, le), 0, len)?;
    let mut at = le - play;
    while at < len {
        // Each pass counts, so a tiny loop over a long clip stops at the event cap.
        cx.count(1)?;
        pass(cx, content, &mut out, (ls, le), at, len)?;
        at = at.saturating_add(le - ls);
    }
    Ok(out)
}

/// The events of `content` in `from..to`, placed from clip tick `at` and cut at `len`.
fn pass(cx: &mut Cx, content: &Sequence, out: &mut Sequence, (from, to): (i64, i64), at: i64, len: i64) -> Result<()> {
    let span = |t: i64| t >= from && t < to;
    for n in content.notes.iter().filter(|n| span(n.start)) {
        let start = at + (n.start - from);
        if start < len {
            cx.count(1)?;
            out.notes.push(Note { start, length: n.length.min(to - n.start).min(len - start).max(1), ..*n });
        }
    }
    for e in content.ctrls.iter().filter(|e| span(e.tick)) {
        let tick = at + (e.tick - from);
        if tick < len {
            cx.count(1)?;
            out.ctrls.push(CtrlEvent { tick, ..*e });
        }
    }
    Ok(())
}

/// Track-level controller events into the clips that cover them, each clip starting from the
/// value in force at its start. Events no clip covers are counted.
fn distribute(cx: &mut Cx, clips: &mut [ClipIn], mut evs: Vec<CtrlEvent>) {
    evs.sort_by_key(|e| e.tick);
    let mut covered = vec![false; evs.len()];
    for c in clips.iter_mut() {
        let end = c.start.saturating_add(c.len);
        let first = evs.partition_point(|e| e.tick < c.start);
        let mut seen: Vec<(u8, CtrlKind)> = Vec::new();
        for i in (0..first).rev() {
            let Some(e) = evs.get(i) else { continue };
            if !seen.contains(&(e.channel, e.kind)) {
                seen.push((e.channel, e.kind));
                c.seq.ctrls.push(CtrlEvent { tick: 0, ..*e });
                covered[i] = true;
            }
        }
        for (i, e) in evs.iter().enumerate().skip(first).take_while(|(_, e)| e.tick < end) {
            c.seq.ctrls.push(CtrlEvent { tick: e.tick - c.start, ..*e });
            covered[i] = true;
        }
    }
    cx.skip("controller points outside clips", covered.iter().filter(|c| !**c).count());
}

/// A clip onto its track, in samples through the tempo map.
fn place(s: &mut Session, track: TrackId, c: ClipIn) -> Result<()> {
    let start = s.tempo.tick_to_samples(c.start.max(0), s.sample_rate);
    let end = s.tempo.tick_to_samples(c.start.max(0).saturating_add(c.len), s.sample_rate);
    if end > s.sample_rate.samples(86_400.0) {
        return Err(crate::cmd::bad(CMD, format!("DAWproject: clip {:?} ends after 24 hours", c.name)));
    }
    let mut seq = c.seq;
    seq.sort();
    let name = if c.name.is_empty() { s.track(track).map(|t| t.name.clone()).unwrap_or_default() } else { c.name };
    let id = s.new_clip_id();
    let mut clip = Clip::midi(id, name, start, (end - start).max(1), seq);
    clip.muted = c.muted;
    crate::edit::place_clip(s, track, clip);
    Ok(())
}

#[cfg(test)]
#[path = "dawproject_tests.rs"]
mod tests;
