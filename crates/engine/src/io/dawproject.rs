//! DAWproject import, slice 1 (issue #261): a `.dawproject` opens as a new, untitled session with
//! its tracks (volume, pan, mute, solo), note clips, tempo and meter, markers and MIDI expression.
//! Devices, audio, sends, group tracks and parameter automation are later slices; the open counts
//! every kind it leaves out and says so, so nothing disappears silently.
//!
//! The archive is hostile input, read the way `read_smf` reads a MIDI file: sizes, nesting and
//! counts are capped, every pass over the content is linear or log-linear, and a bad value skips its
//! element (counted) or refuses the file, never panics.
//! Times are in beats (quarter notes, the DAWproject default); scopes timed in seconds are skipped.
//! Controller ramps become steps at their points (the points are dense in practice).

use super::*;
use roxmltree::{Document, Node, ParsingOptions};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use transit_midi::{CtrlEvent, CtrlKind, Note, PPQ, Sequence};

const CMD: &str = "session.open";
/// Largest `project.xml` read, uncompressed (real exports: up to about 5 MB).
const MAX_XML_BYTES: u64 = 64 << 20;
/// Most `<` and `=` in the whole text. roxmltree reserves a node per `<` and an attribute per `=`
/// before it parses anything (`nodes_limit` is checked later), so both are counted first (real
/// exports: up to 53,000 and 184,000).
const MAX_NODES: u32 = 1_000_000;
/// Deepest element nesting and most attributes on one element (real projects: 12 and 10). The
/// parser recurses per level and checks attributes pairwise, so both are refused before it runs.
const MAX_DEPTH: usize = 64;
const MAX_ATTRS: usize = 64;
const MAX_EQUALS: usize = 3_000_000;
/// Most notes and controller events in the whole project, loops unrolled.
const MAX_EVENTS: usize = 4_000_000;
/// Most tempo and meter changes (each meter is placed by walking the ones before it, in time order).
const MAX_MAP_POINTS: usize = 10_000;
/// Latest time read, in beats (24 hours at 999 BPM is under 1.5 million).
const MAX_BEATS: f64 = 2_000_000.0;
/// Most tracks, clips and markers made (real projects have tens to hundreds; each add re-sorts).
const MAX_TRACKS: usize = 1_000;
const MAX_CLIPS: usize = 10_000;
const MAX_MARKERS: usize = 1_000;

/// Whether `path` names a DAWproject file.
pub fn is_dawproject(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|x| x.eq_ignore_ascii_case("dawproject"))
}

/// Open `path` (a `.dawproject`) as a new, untitled session. Returns what was not imported, one
/// line per kind, also posted as messages.
pub fn open_dawproject(e: &mut Engine, path: &str) -> Result<Vec<String>> {
    let file = super::open_file(path)?;
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
    check_shape(&xml)?;
    let doc = Document::parse_with_options(&xml, ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES })
        .map_err(|err| crate::cmd::bad(CMD, format!("DAWproject: project.xml: {err}")))?;
    let mut cx = Cx::default();
    let s = project(&mut cx, doc.root_element(), name)?;
    let report = cx.report();
    Ok((s, report))
}

/// Refuses XML nested deeper than [`MAX_DEPTH`] or with an element of more than [`MAX_ATTRS`]
/// attributes, in one linear pass that reads tags the way the parser does (quoted values, comments,
/// CDATA and processing instructions are skipped); the parser stops at the first malformed tag, so
/// it never sees deeper than this pass does.
fn check_shape(xml: &str) -> Result<()> {
    let b = xml.as_bytes();
    let count = |c: u8| b.iter().filter(|&&x| x == c).count();
    if count(b'<') > MAX_NODES as usize || count(b'=') > MAX_EQUALS {
        return Err(crate::cmd::bad(CMD, "DAWproject: project.xml has too many elements or attributes"));
    }
    let find = |from: usize, pat: &[u8]| b.get(from..).and_then(|r| r.windows(pat.len()).position(|w| w == pat)).map(|p| from + p + pat.len());
    let (mut i, mut depth) = (0, 0usize);
    while let Some(lt) = b.get(i..).and_then(|r| r.iter().position(|&c| c == b'<')) {
        i += lt + 1;
        let rest = b.get(i..).unwrap_or_default();
        let skip_to = |pat: &[u8]| find(i, pat).unwrap_or(b.len());
        if rest.starts_with(b"/") {
            depth = depth.saturating_sub(1);
        } else if rest.starts_with(b"?") {
            i = skip_to(b"?>");
        } else if rest.starts_with(b"!--") {
            i = skip_to(b"-->");
        } else if rest.starts_with(b"![CDATA[") {
            i = skip_to(b"]]>");
        } else if rest.starts_with(b"!") {
            i = skip_to(b">");
        } else {
            let (mut attrs, mut quote, mut closed) = (0, None, false);
            while let Some(&c) = b.get(i) {
                i += 1;
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(c),
                    (None, b'=') => attrs += 1,
                    (None, b'>') => {
                        closed = true;
                        break;
                    }
                    _ => {}
                }
            }
            if attrs > MAX_ATTRS {
                return Err(crate::cmd::bad(CMD, format!("DAWproject: project.xml has an element with more than {MAX_ATTRS} attributes")));
            }
            if closed && b.get(i.saturating_sub(2)) != Some(&b'/') {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(crate::cmd::bad(CMD, format!("DAWproject: project.xml is nested more than {MAX_DEPTH} levels deep")));
                }
            }
        }
    }
    Ok(())
}

/// What the import counts as it goes.
#[derive(Default)]
struct Cx {
    skipped: BTreeMap<&'static str, usize>,
    events: usize,
    clips: usize,
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

/// An `xs:boolean`: `true`, `false`, `1` or `0`.
fn boolean(v: Option<&str>) -> Option<bool> {
    match v.map(str::trim) {
        Some("true" | "1") => Some(true),
        Some("false" | "0") => Some(false),
        _ => None,
    }
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
        for c in kids(st, "Channel") {
            cx.skip("channels outside tracks", 1);
            channel_extras(cx, c);
        }
    }
    if let Some(sc) = child(root, "Scenes") {
        cx.skip("clip launcher clips", sc.descendants().filter(|d| d.has_tag_name("Clip")).count());
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
        distribute(cx, &mut cs, ctrls)?;
        let mut end = i64::MIN;
        let mut kept = Vec::with_capacity(cs.len());
        for c in cs {
            if c.start < end {
                cx.skip("overlapping clips (the later one covers the earlier)", 1);
            }
            end = end.max(c.start.saturating_add(c.len));
            kept.extend(from_zero(cx, &s, c)?);
        }
        for c in visible(kept) {
            place(&mut s, tid, c);
        }
        if let Some(pl) = s.track_mut(tid).and_then(|t| t.playlist_mut()) {
            pl.sort();
        }
    }
    if let Some(m) = arrangement.and_then(|a| child(a, "Markers")) {
        if seconds(m) {
            cx.skip("markers timed in seconds", kids(m, "Marker").count());
        } else {
            let all: Vec<_> = kids(m, "Marker").collect();
            cx.skip("markers past the limit", all.len().saturating_sub(MAX_MARKERS));
            for mk in all.into_iter().take(MAX_MARKERS) {
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
        for (tick, pt) in map_points(cx, p, "RealPoint", ["tempo changes without a valid time or value", "tempo changes past the limit"]) {
            match num(pt, "value") {
                Some(bpm) => tempo(cx, s, tick, bpm),
                None => cx.skip("tempo changes without a valid time or value", 1),
            }
        }
    }
    if let Some(p) = child(a, "TimeSignatureAutomation") {
        for (tick, pt) in map_points(cx, p, "TimeSignaturePoint", ["meter changes without a valid time", "meter changes past the limit"]) {
            meter_at(cx, s, tick, pt);
        }
    }
}

/// The points of a tempo or meter timeline in time order (each meter is placed by walking the
/// ones before it, so out-of-order points would cost the cube), capped.
fn map_points<'a, 'i: 'a>(cx: &mut Cx, p: Node<'a, 'i>, tag: &'static str, [bad, past]: [&'static str; 2]) -> Vec<(i64, Node<'a, 'i>)> {
    if seconds(p) {
        cx.skip("timelines timed in seconds", 1);
        return Vec::new();
    }
    let mut points: Vec<_> = kids(p, tag).filter_map(|pt| beats(pt, "time").map(|t| (ticks(t), pt))).collect();
    cx.skip(bad, kids(p, tag).count() - points.len());
    cx.skip(past, points.len().saturating_sub(MAX_MAP_POINTS));
    points.truncate(MAX_MAP_POINTS);
    points.sort_by_key(|(t, _)| *t);
    points
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
        channel_extras(cx, c);
    }
    let types = t.attribute("contentType").unwrap_or("");
    let has = |what: &str| types.split_whitespace().any(|w| w == what);
    // Bitwig writes a group as contentType="tracks" with role="master": nesting makes it a group.
    let group = has("tracks") || kids(t, "Track").next().is_some();
    let role = ch.and_then(|c| c.attribute("role")).unwrap_or("regular");
    let kind = match role {
        _ if group => {
            cx.skip("group, effect, VCA and other tracks", 1);
            None
        }
        "master" if ch.is_some_and(|c| !default_channel(c)) => Some(TrackKind::Master),
        "master" => None,
        "regular" if has("notes") => Some(TrackKind::Instrument),
        "regular" if has("audio") => Some(TrackKind::Audio),
        _ => {
            cx.skip("group, effect, VCA and other tracks", 1);
            None
        }
    };
    let kind = kind.filter(|_| {
        let room = s.tracks.len() < MAX_TRACKS;
        if !room {
            cx.skip("tracks past the limit", 1);
        }
        room
    });
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

/// What a channel holds that slice 1 does not import: its devices and sends, counted.
fn channel_extras(cx: &mut Cx, c: Node) {
    cx.skip("devices (plug-ins and built-in)", child(c, "Devices").map_or(0, |d| d.children().filter(Node::is_element).count()));
    cx.skip("sends", child(c, "Sends").map_or(0, |d| kids(d, "Send").count()));
}

/// A channel at unity volume, centered and not muted.
fn default_channel(c: Node) -> bool {
    let at = |tag, want: f64| child(c, tag).and_then(|n| num(n, "value")).is_none_or(|v| (v - want).abs() < 1e-6);
    let unity = match child(c, "Volume").and_then(|v| v.attribute("unit")) {
        Some("decibel") => at("Volume", 0.0),
        _ => at("Volume", 1.0),
    };
    unity && at("Pan", 0.5) && boolean(child(c, "Mute").and_then(|m| m.attribute("value"))) != Some(true)
}

/// A channel's volume (linear or dB), pan (normalized, as a balance on stereo tracks), mute and solo.
fn mixer(cx: &mut Cx, m: &mut transit_model::Mixer, c: Node) {
    if let Some(v) = child(c, "Volume") {
        // `inf` and `-inf` are how DAWproject writes a fader's ends: clamped, not refused.
        let value = v.attribute("value").and_then(|x| x.trim().parse::<f64>().ok()).filter(|x| !x.is_nan());
        match (v.attribute("unit"), value) {
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
    m.mute = boolean(child(c, "Mute").and_then(|n| n.attribute("value"))) == Some(true);
    m.solo = boolean(c.attribute("solo")) == Some(true);
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
                        Some(_) if cx.clips >= MAX_CLIPS => cx.skip("clips past the limit", 1),
                        Some(t) => {
                            cx.clips += 1;
                            out.entry(t).or_default().0.push(clip);
                        }
                        None => cx.skip("clips on tracks not imported", 1),
                    }
                }
            }
            "Points" if seconds(c) => cx.skip("timelines timed in seconds", 1),
            "Points" => match (midi_points(cx, c, 0, None, 0)?, here.flatten()) {
                (Some(evs), Some(t)) => out.entry(t).or_default().1.extend(evs),
                (Some(_), None) => cx.skip("controller lanes on tracks not imported", 1),
                (None, _) => cx.skip("parameter automation lanes", 1),
            },
            "Audio" | "Warps" => cx.skip("audio clips", 1),
            "Video" => cx.skip("video clips", 1),
            "ClipSlot" => cx.skip("clip launcher clips", c.descendants().filter(|d| d.has_tag_name("Clip")).count()),
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
        // Audio clips are timed in seconds: say what they are.
        let audio = k.descendants().any(|d| d.has_tag_name("Audio") || d.has_tag_name("Warps"));
        cx.skip(if audio { "audio clips" } else { "clips timed in seconds" }, 1);
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
    let stop = beats(k, "playStop").map(ticks);
    let seq = window(cx, content.resolved(), ticks(play), ticks(len), lp, stop)?;
    let mut name = k.attribute("name").unwrap_or("").to_string();
    transit_model::truncate_chars(&mut name, transit_model::MAX_NAME_CHARS);
    Ok(Some(ClipIn { start: ticks(time), len: ticks(len).max(1), name, muted: boolean(k.attribute("enable")) == Some(false), seq }))
}

/// A clip's content before its timbre is resolved: the timbre points and the spans of the notes
/// that carry them are kept until the whole clip is read, so notes in sibling timelines compete.
#[derive(Default)]
struct Content {
    seq: Sequence,
    timbre: Vec<Timbre>,
    spans: Vec<(i64, u8, i64, u8)>,
}

impl Content {
    /// The events with the timbre points their notes own as channel CC74.
    fn resolved(mut self) -> Sequence {
        own_timbre(&mut self.timbre, self.spans);
        self.seq.ctrls.extend(self.timbre.into_iter().map(|t| CtrlEvent {
            tick: t.tick,
            channel: t.channel,
            kind: CtrlKind::Cc(74),
            value: t.value,
        }));
        self.seq
    }
}

/// The note and controller events of a clip's content timeline, in content ticks. None when it
/// holds none of them.
fn content(cx: &mut Cx, n: Node) -> Result<Option<Content>> {
    let mut out = Content::default();
    let mut any = false;
    for c in n.children().filter(Node::is_element) {
        if seconds(c) {
            cx.skip("timelines timed in seconds", 1);
            continue;
        }
        match c.tag_name().name() {
            "Notes" => {
                notes(cx, c, &mut out)?;
                any = true;
            }
            "Points" => match midi_points(cx, c, 0, None, 0)? {
                Some(evs) => {
                    out.seq.ctrls.extend(evs);
                    any = true;
                }
                None => cx.skip("parameter automation lanes", 1),
            },
            "Lanes" => {
                if let Some(inner) = content(cx, c)? {
                    merge(&mut out.seq, inner.seq, 0);
                    out.timbre.extend(inner.timbre);
                    out.spans.extend(inner.spans);
                    any = true;
                }
            }
            "Clips" => {
                // A nested clip has its own window: its timbre is resolved inside it.
                for k in kids(c, "Clip") {
                    if let Some(inner) = clip(cx, k)? {
                        if !inner.muted {
                            merge(&mut out.seq, inner.seq, inner.start);
                        }
                        any = true;
                    }
                }
            }
            "Audio" | "Warps" => cx.skip("audio clips", 1),
            "Video" => cx.skip("video clips", 1),
            _ => {}
        }
    }
    Ok(any.then_some(out))
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
/// aftertouch; timbre points are kept for the clip to resolve ([`Content::resolved`]).
fn notes(cx: &mut Cx, ns: Node, out: &mut Content) -> Result<()> {
    for n in kids(ns, "Note") {
        let key = n.attribute("key").and_then(|k| k.trim().parse::<u8>().ok()).filter(|k| *k < 128);
        let channel = n.attribute("channel").map_or(Some(0), |c| c.trim().parse::<u8>().ok()).filter(|c| *c < 16);
        let (Some(t), Some(d), Some(pitch), Some(channel)) = (beats(n, "time"), beats(n, "duration"), key, channel) else {
            cx.skip("notes with bad values", 1);
            continue;
        };
        let (start, length) = (ticks(t), ticks(d).max(1));
        cx.count(1)?;
        out.seq.notes.push(Note {
            pitch,
            velocity: num(n, "vel").map_or(100, |v| midi7(v).max(1)) as u8,
            release_velocity: num(n, "rel").map_or(64, midi7) as u8,
            channel,
            start,
            length,
        });
        // One expression is a `Points` child; the schema wraps several in a `Lanes`.
        for p in kids(n, "Points").chain(kids(n, "Lanes").filter(|l| !seconds(*l)).flat_map(|l| kids(l, "Points"))) {
            if seconds(p) {
                cx.skip("timelines timed in seconds", 1);
                continue;
            }
            match child(p, "Target").and_then(|t| t.attribute("expression")) {
                Some("timbre") => {
                    let before = out.timbre.len();
                    for (tick, value) in normalized(cx, p, start, false)? {
                        out.timbre.push(Timbre { tick, note: (start, pitch), channel, value: midi7(value) });
                    }
                    if out.timbre.len() > before {
                        out.spans.push((start, pitch, start.saturating_add(length), channel));
                    }
                }
                Some("pressure") => match midi_points(cx, p, start, Some(pitch), channel)? {
                    Some(evs) => out.seq.ctrls.extend(evs),
                    None => cx.skip("per-note gain, pan, transpose and formant curves", 1),
                },
                _ => cx.skip("per-note gain, pan, transpose and formant curves", 1),
            }
        }
        cx.skip("timelines timed in seconds", kids(n, "Lanes").filter(|l| seconds(*l)).count());
    }
    Ok(())
}

/// Keeps the timbre points their notes own: a point is owned unless a note that started later (or
/// with it, higher) on its channel is sounding at its tick. One sweep in time order, keeping the
/// notes that sound in a set, so overlapping notes cost a logarithm each, not a scan.
fn own_timbre(timbre: &mut Vec<Timbre>, mut spans: Vec<(i64, u8, i64, u8)>) {
    timbre.sort_by_key(|t| t.tick);
    spans.sort_unstable();
    let mut sounding: BTreeMap<(u8, i64, u8), usize> = BTreeMap::new();
    let mut ends = BinaryHeap::new();
    let mut next = spans.iter().peekable();
    timbre.retain(|t| {
        while let Some(&&(start, pitch, end, ch)) = next.peek().filter(|s| s.0 <= t.tick) {
            *sounding.entry((ch, start, pitch)).or_default() += 1;
            ends.push(Reverse((end, ch, start, pitch)));
            next.next();
        }
        while let Some(&Reverse((_, ch, start, pitch))) = ends.peek().filter(|Reverse(e)| e.0 <= t.tick) {
            ends.pop();
            if let Some(n) = sounding.get_mut(&(ch, start, pitch)) {
                *n -= 1;
                if *n == 0 {
                    sounding.remove(&(ch, start, pitch));
                }
            }
        }
        let newest = sounding.range((t.channel, i64::MIN, 0)..=(t.channel, i64::MAX, u8::MAX)).next_back().map(|(k, _)| (k.1, k.2));
        newest.is_none_or(|n| n <= t.note)
    });
}

/// The `(tick, value)` points of a normalized `Points` timeline, `base` ticks added. Non-normalized
/// units give none (counted). With `ints`, an `IntegerPoint` (a program number, 0..=127) is read
/// too, as the normalized value that maps back to it.
fn normalized(cx: &mut Cx, p: Node, base: i64, ints: bool) -> Result<Vec<(i64, f64)>> {
    // Bitwig writes per-note pressure as "linear" 0..1, the same range as "normalized".
    if p.attribute("unit").is_some_and(|u| u != "normalized" && u != "linear") {
        cx.skip("controller lanes in other units", 1);
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for pt in p.children().filter(Node::is_element) {
        match pt.tag_name().name() {
            "RealPoint" => match (beats(pt, "time"), num(pt, "value")) {
                (Some(t), Some(v)) => out.push((base.saturating_add(ticks(t)), v)),
                _ => cx.skip("controller points with bad values", 1),
            },
            "IntegerPoint" if ints => match (beats(pt, "time"), pt.attribute("value").and_then(|v| v.trim().parse::<i64>().ok())) {
                (Some(t), Some(v)) => out.push((base.saturating_add(ticks(t)), v.clamp(0, 127) as f64 / 127.0)),
                _ => cx.skip("controller points with bad values", 1),
            },
            "Target" => {}
            _ => cx.skip("controller points of other kinds", 1),
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
    for (tick, v) in normalized(cx, p, base, kind == CtrlKind::Program)? {
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
/// `stop` (`playStop`) ends an unlooped window early.
fn window(cx: &mut Cx, mut content: Sequence, play: i64, len: i64, lp: Option<(i64, i64)>, stop: Option<i64>) -> Result<Sequence> {
    // Sorted once, so each pass finds its range by search instead of scanning the content.
    content.sort();
    let mut out = Sequence::default();
    let Some((ls, le)) = lp.filter(|(ls, le)| le > ls && play < *le) else {
        let to = play.saturating_add(len).min(stop.unwrap_or(i64::MAX));
        pass(cx, &content, &mut out, (play, to), 0, len)?;
        return Ok(out);
    };
    pass(cx, &content, &mut out, (play, le), 0, len)?;
    let mut at = le - play;
    while at < len {
        // Each pass counts, so a tiny loop over a long clip stops at the event cap.
        cx.count(1)?;
        pass(cx, &content, &mut out, (ls, le), at, len)?;
        at = at.saturating_add(le - ls);
    }
    Ok(out)
}

/// The events of sorted `content` in `from..to`, placed from clip tick `at` and cut at `len`.
fn pass(cx: &mut Cx, content: &Sequence, out: &mut Sequence, (from, to): (i64, i64), at: i64, len: i64) -> Result<()> {
    let first = content.notes.partition_point(|n| n.start < from);
    for n in content.notes.get(first..).unwrap_or_default().iter().take_while(|n| n.start < to) {
        let start = at + (n.start - from);
        if start < len {
            cx.count(1)?;
            out.notes.push(Note { start, length: n.length.min(to - n.start).min(len - start).max(1), ..*n });
        }
    }
    let first = content.ctrls.partition_point(|e| e.tick < from);
    for e in content.ctrls.get(first..).unwrap_or_default().iter().take_while(|e| e.tick < to) {
        let tick = at + (e.tick - from);
        if tick < len {
            cx.count(1)?;
            out.ctrls.push(CtrlEvent { tick, ..*e });
        }
    }
    Ok(())
}

/// An order for controller kinds (the `(channel, kind)` state below is keyed by it).
fn kind_key(k: CtrlKind) -> (u8, u8) {
    match k {
        CtrlKind::Cc(n) => (0, n),
        CtrlKind::PitchBend => (1, 0),
        CtrlKind::ChannelPressure => (2, 0),
        CtrlKind::PolyPressure(p) => (3, p),
        CtrlKind::Program => (4, 0),
    }
}

/// Track-level controller events into the clips (sorted by start) that cover them, each clip
/// starting from the values in force at its start. One sweep: the values in force are kept as the
/// clips go by, and every event a clip gets, carried or covered, counts toward the cap. Events no
/// clip covers or carries are counted.
fn distribute(cx: &mut Cx, clips: &mut [ClipIn], mut evs: Vec<CtrlEvent>) -> Result<()> {
    evs.sort_by_key(|e| e.tick);
    let mut covered = vec![false; evs.len()];
    let mut in_force: BTreeMap<(u8, (u8, u8)), usize> = BTreeMap::new();
    let mut next = 0;
    for c in clips.iter_mut() {
        while let Some(e) = evs.get(next).filter(|e| e.tick < c.start) {
            in_force.insert((e.channel, kind_key(e.kind)), next);
            next += 1;
        }
        cx.count(in_force.len())?;
        for &i in in_force.values() {
            if let Some(e) = evs.get(i) {
                c.seq.ctrls.push(CtrlEvent { tick: 0, ..*e });
                covered[i] = true;
            }
        }
        let end = c.start.saturating_add(c.len);
        for (i, e) in evs.iter().enumerate().skip(next).take_while(|(_, e)| e.tick < end) {
            cx.count(1)?;
            c.seq.ctrls.push(CtrlEvent { tick: e.tick - c.start, ..*e });
            covered[i] = true;
        }
    }
    cx.skip("controller points outside clips", covered.iter().filter(|c| !**c).count());
    Ok(())
}

/// A clip as it starts at zero: one that starts before loses the part before it (its events keep
/// their times), and one that ends there is left out. A clip ending after 24 hours is refused.
fn from_zero(cx: &mut Cx, s: &Session, mut c: ClipIn) -> Result<Option<ClipIn>> {
    if c.start < 0 {
        let cut = -c.start;
        let notes = c.seq.notes.len();
        c.seq.notes.retain(|n| n.start >= cut);
        cx.skip("notes before the start of the arrangement", notes - c.seq.notes.len());
        let ctrls = c.seq.ctrls.len();
        c.seq.ctrls.retain(|e| e.tick >= cut);
        cx.skip("controller points before the start of the arrangement", ctrls - c.seq.ctrls.len());
        for n in &mut c.seq.notes {
            n.start -= cut;
        }
        for e in &mut c.seq.ctrls {
            e.tick -= cut;
        }
        (c.start, c.len) = (0, c.len - cut);
        if c.len <= 0 {
            cx.skip("clips before the start of the arrangement", 1);
            return Ok(None);
        }
    }
    if s.tempo.tick_to_samples(c.start.saturating_add(c.len), s.sample_rate) > s.sample_rate.samples(86_400.0) {
        return Err(crate::cmd::bad(CMD, format!("DAWproject: clip {:?} ends after 24 hours", c.name)));
    }
    c.seq.sort();
    Ok(Some(c))
}

/// One track's clips, sorted by start, with their overlaps resolved in one sweep the way the
/// editor's overwrite (`edit::place_clip`) resolves them one clip at a time: a later clip covers
/// what it overlaps of the earlier ones (of two at one start, the later in the file), which keep
/// their parts before and after it. An event goes to the part its start falls in, as a split keeps
/// it on one side, and one under a later clip is dropped. Each event is copied at most once, so a
/// long dense clip under thousands of short ones costs its events, not their product (#377).
fn visible(cs: Vec<ClipIn>) -> Vec<ClipIn> {
    // The visible parts, `(clip, from, to)` in ticks: in order and disjoint, so their starts and
    // their ends both ascend.
    let mut parts: Vec<(usize, i64, i64)> = Vec::with_capacity(cs.len());
    for (i, c) in cs.iter().enumerate() {
        let (from, to) = (c.start, c.start.saturating_add(c.len));
        // The parts it overlaps: those ending after its start and starting before its end.
        let (lo, hi) = (parts.partition_point(|p| p.2 <= from), parts.partition_point(|p| p.1 < to));
        let mut put = Vec::with_capacity(3);
        if let Some(&(j, a, _)) = parts.get(lo).filter(|p| lo < hi && p.1 < from) {
            put.push((j, a, from));
        }
        put.push((i, from, to));
        if let Some(&(j, _, b)) = hi.checked_sub(1).and_then(|k| parts.get(k)).filter(|p| lo < hi && p.2 > to) {
            put.push((j, to, b));
        }
        parts.splice(lo..hi, put);
    }
    let mut spans: Vec<Vec<(i64, i64)>> = vec![Vec::new(); cs.len()];
    for &(i, a, b) in &parts {
        if let Some(v) = spans.get_mut(i) {
            v.push((a, b));
        }
    }
    let mut out = Vec::with_capacity(parts.len());
    for (c, spans) in cs.into_iter().zip(spans) {
        // Not overlapped at all: as it is, its events moved rather than copied.
        if spans == [(c.start, c.start.saturating_add(c.len))] {
            out.push(c);
            continue;
        }
        for (a, b) in spans {
            // In the clip's own ticks; its events are sorted by start (see `from_zero`).
            let (lo, hi) = (a - c.start, b - c.start);
            let notes = &c.seq.notes[c.seq.notes.partition_point(|n| n.start < lo)..c.seq.notes.partition_point(|n| n.start < hi)];
            let ctrls = &c.seq.ctrls[c.seq.ctrls.partition_point(|e| e.tick < lo)..c.seq.ctrls.partition_point(|e| e.tick < hi)];
            let seq = Sequence {
                notes: notes.iter().map(|n| Note { start: n.start - lo, ..*n }).collect(),
                ctrls: ctrls.iter().map(|e| CtrlEvent { tick: e.tick - lo, ..*e }).collect(),
            };
            out.push(ClipIn { start: a, len: b - a, name: c.name.clone(), muted: c.muted, seq });
        }
    }
    out
}

/// A clip onto its track, in samples through the tempo map (the caller sorts the playlist once).
fn place(s: &mut Session, track: TrackId, c: ClipIn) {
    let start = s.tempo.tick_to_samples(c.start, s.sample_rate);
    let end = s.tempo.tick_to_samples(c.start.saturating_add(c.len), s.sample_rate);
    let name = if c.name.is_empty() { s.track(track).map(|t| t.name.clone()).unwrap_or_default() } else { c.name };
    let id = s.new_clip_id();
    let mut clip = Clip::midi(id, name, start, (end - start).max(1), c.seq);
    clip.muted = c.muted;
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        pl.clips.push(clip);
    }
}

#[cfg(test)]
#[path = "dawproject_tests.rs"]
mod tests;
