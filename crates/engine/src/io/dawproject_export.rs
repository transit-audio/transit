//! DAWproject export (issue #261, slice 2): tracks and their mixer, note clips and MIDI expression,
//! tempo and meter, markers, and one full-length WAV stem per audible audio or instrument track.
//! Devices, routing, groups and parameter automation are represented by the stems, not as separate
//! DAWproject elements. External Instruments cannot render offline; their MIDI remains and the
//! report names them.

use super::*;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use transit_midi::{CtrlEvent, CtrlKind, PPQ, Sequence};
use transit_model::MarkerKind;

const CMD: &str = "file.export_dawproject";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DawprojectReport {
    pub path: PathBuf,
    pub tracks: usize,
    pub clips: usize,
    pub stems: usize,
    pub notes: Vec<String>,
    pub left_out: Vec<String>,
}

#[derive(Debug)]
struct Stem {
    track: TrackId,
    path: String,
    disk: PathBuf,
    channels: usize,
}

pub fn dawproject_file(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.extension().is_none() { path.with_extension("dawproject") } else { path }
}

struct Stage {
    path: PathBuf,
}

impl Stage {
    fn create(output: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let name = output.file_name().map_or_else(|| "export".into(), |n| n.to_string_lossy().into_owned());
        for _ in 0..100 {
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".{name}.stems-{}-{serial}", std::process::id()));
            crate::file_access::check_write(&path).map_err(EngineError::Io)?;
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(EngineError::Io(format!("{}: {e}", path.display()))),
            }
        }
        Err(EngineError::Io(format!("{}: could not create a temporary export folder", output.display())))
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn refuse_existing_part(part: &Path) -> Result<()> {
    match std::fs::symlink_metadata(part) {
        Ok(_) => Err(EngineError::Io(format!("{}: an archive part already exists; move or remove it before exporting", part.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(EngineError::Io(format!("{}: {e}", part.display()))),
    }
}

/// Export `e`'s session to `path`, adding `.dawproject` when it has no extension. The output is
/// renamed into place only after every stem and the ZIP have completed. The command layer controls
/// whether an existing output may be replaced.
pub fn export_dawproject(e: &Engine, path: &str) -> Result<DawprojectReport> {
    let path = dawproject_file(path);
    let name = path.file_name().map_or_else(|| "export.dawproject".into(), |n| n.to_string_lossy().into_owned());
    let part = path.with_file_name(format!("{name}.part"));
    crate::file_access::check_write(&path).map_err(EngineError::Io)?;
    crate::file_access::check_write(&part).map_err(EngineError::Io)?;
    refuse_existing_part(&part)?;
    let stage = Stage::create(&path)?;
    let s = e.session();
    let range = Range::new(0, s.content_end().max(1));
    check_bounce_range(s, range, CMD)?;
    let opts = EncodeOptions {
        format: FileFormat::Wav,
        bit_depth: transit_audio_io::BitDepth::Float32,
        dither: false,
        bwf: None,
        bitrate_kbps: None,
        title: None,
    };
    let left_out = crate::cmd::external_in_render(s);
    let mut stems = Vec::new();
    let mut solo = Arc::new(s.clone());
    for (index, track) in s.tracks.iter().enumerate() {
        let external = track.external_instrument().is_some() && track.freeze.is_none();
        if !matches!(track.kind, TrackKind::Audio | TrackKind::Instrument)
            || track.inactive
            || track.mixer.mute
            || track.clips().is_empty()
            || external
        {
            continue;
        }
        let downstream = downstream_tracks(s, track.id);
        for t in &mut Arc::make_mut(&mut solo).tracks {
            t.mixer.solo = t.id == track.id || downstream.contains(&t.id);
            t.mixer.solo_safe = false;
            if t.id == track.id || t.kind == TrackKind::Master {
                // The archive carries these static controls; printing them into the stem too would
                // apply them twice. Their lanes are omitted with the other mixer automation. Clip
                // processing, inserts, trim and sends still render into the stem.
                t.mixer.volume_db = 0.0;
                t.mixer.pan = transit_model::Mixer::new(t.format).pan;
                t.automation.retain(|lane| !matches!(lane.param, transit_model::AutoParam::Volume | transit_model::AutoParam::Pan(_)));
            }
        }
        let disk = stage.path.join(format!("track-{index}.wav"));
        Bounce::with_options(Arc::clone(&solo), &disk, range, &opts, BounceOptions::default())?.run()?;
        stems.push(Stem { track: track.id, path: format!("audio/track-{index}.wav"), disk, channels: bounce_layout(s, None, false).1.max(1) });
    }
    let xml = project_xml(s, &stems);
    write_archive(&part, &xml, &stems)?;
    replace_with(&part, &path, Ok(()))?;
    let tracks = s.tracks.iter().filter(|t| exported_track(t)).count();
    let clips =
        s.tracks.iter().filter(|t| exported_track(t)).flat_map(|t| t.clips()).filter(|c| matches!(&c.content, ClipContent::Midi { .. })).count();
    let notes = export_notes(s, stems.len(), &left_out);
    log::info!("DAWproject exported: path={:?} tracks={tracks} clips={clips} stems={} left_out=[{}]", path, stems.len(), left_out.join(", "));
    Ok(DawprojectReport { path, tracks, clips, stems: stems.len(), notes, left_out })
}

fn write_archive(part: &Path, xml: &str, stems: &[Stem]) -> Result<()> {
    let zerr = |err: zip::result::ZipError| EngineError::Io(format!("DAWproject: {err}"));
    let ioerr = |err: std::io::Error| EngineError::Io(format!("{}: {err}", part.display()));
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(part).map_err(ioerr)?;
    let written = (|| {
        let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
        let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("project.xml", deflated).map_err(zerr)?;
        zip.write_all(xml.as_bytes()).map_err(ioerr)?;
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
        for stem in stems {
            zip.start_file(&stem.path, stored).map_err(zerr)?;
            let mut input = std::fs::File::open(&stem.disk).map_err(ioerr)?;
            std::io::copy(&mut input, &mut zip).map_err(ioerr)?;
        }
        let mut out = zip.finish().map_err(zerr)?;
        out.flush().map_err(ioerr)?;
        out.get_ref().sync_all().map_err(ioerr)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(part);
    }
    written
}

fn exported_kind(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi | TrackKind::Master)
}

fn exported_track(track: &transit_model::Track) -> bool {
    !track.inactive && exported_kind(track.kind)
}

fn bus_outputs(t: &transit_model::Track, busses: &mut HashSet<BusId>) {
    for route in std::iter::once(&t.mixer.output).chain(t.mixer.sends.iter().flatten().map(|send| &send.target)) {
        if let transit_model::Route::Bus(bus) = route {
            busses.insert(*bus);
        }
    }
}

/// Auxes and folders downstream of `source`, through every output/send hop. The mix's implicit-solo
/// check is one hop; marking the whole chain keeps nested processing audible in an isolated stem.
fn downstream_tracks(s: &Session, source: TrackId) -> HashSet<TrackId> {
    let mut busses = HashSet::new();
    let mut tracks = HashSet::new();
    if let Some(t) = s.track(source) {
        bus_outputs(t, &mut busses);
    }
    loop {
        let before = tracks.len();
        for t in &s.tracks {
            if tracks.contains(&t.id) || !matches!(t.kind, TrackKind::Aux | TrackKind::Folder) {
                continue;
            }
            let transit_model::Route::Bus(input) = &t.mixer.input else { continue };
            if busses.contains(input) {
                tracks.insert(t.id);
                bus_outputs(t, &mut busses);
            }
        }
        if tracks.len() == before {
            return tracks;
        }
    }
}

fn export_notes(s: &Session, stems: usize, left_out: &[String]) -> Vec<String> {
    let mut notes = Vec::new();
    if stems > 0 {
        notes.push(format!(
            "{stems} full-length WAV stems carry the audio and device processing; MIDI clips remain editable. Devices, sends, groups and parameter automation are not exported separately."
        ));
    }
    if !left_out.is_empty() {
        notes.push("External Instrument audio is not rendered offline; those tracks' MIDI clips are included.".into());
    }
    let non_media = s.tracks.iter().filter(|t| !exported_kind(t.kind)).count();
    if non_media > 0 {
        notes.push(format!(
            "{non_media} aux, VCA and folder tracks are not exported separately; their audible contributions are included in the stems."
        ));
    }
    let inactive = s.tracks.iter().filter(|t| t.inactive && exported_kind(t.kind)).count();
    if inactive > 0 {
        notes.push(format!("{inactive} inactive tracks are omitted and are not reactivated by the archive."));
    }
    if s.tracks.iter().any(|t| {
        exported_track(t) && t.automation.iter().any(|lane| matches!(lane.param, transit_model::AutoParam::Volume | transit_model::AutoParam::Pan(_)))
    }) {
        notes.push("Track and master volume/pan automation is omitted; the static mixer values are kept.".into());
    }
    let locations = s.markers.iter().filter(|m| m.kind != MarkerKind::Marker || m.track.is_some()).count();
    if locations > 0 {
        notes.push(format!(
            "{locations} stored selections, view locations and track markers are not DAWproject arrangement markers and were not exported."
        ));
    }
    let panners = s.tracks.iter().filter(|t| exported_track(t) && !balance(&t.mixer.pan).1).count();
    if panners > 0 {
        notes.push(format!("{panners} stereo or surround panners were reduced to a single DAWproject balance value."));
    }
    notes
}

/// XML 1.0 attribute text. Invalid control code points become replacement characters rather than
/// making the whole document unreadable.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if c >= '\u{20}' && c != '\u{fffe}' && c != '\u{ffff}' => out.push(c),
            _ => out.push('\u{fffd}'),
        }
    }
    out
}

fn color(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

fn beats(s: &Session, at: Samples) -> f64 {
    let value = s.tempo.samples_to_ticks_f(at, s.sample_rate) / f64::from(PPQ);
    if value.is_finite() { value } else { 0.0 }
}

fn beat_len(s: &Session, start: Samples, length: Samples) -> f64 {
    beats(s, start.saturating_add(length)) - beats(s, start)
}

fn number(value: f64) -> String {
    format!("{value:.9}")
}

/// DAWproject has one balance value. Transit stores one panner per channel: the side moved toward
/// the center determines that balance. Independent dual panners and surround panners are lossy.
fn balance(pan: &[f32]) -> (f64, bool) {
    match pan {
        [one] => ((f64::from(*one) + 1.0) / 2.0, true),
        [left, right] => {
            let exact = (*left == -1.0 && *right >= -1.0) || (*right == 1.0 && *left <= 1.0);
            let value = if *left > -1.0 {
                (f64::from(*left) + 3.0) / 4.0
            } else if *right < 1.0 {
                (f64::from(*right) + 1.0) / 4.0
            } else {
                0.5
            };
            (value.clamp(0.0, 1.0), exact)
        }
        [] => (0.5, true),
        _ => (0.5, false),
    }
}

fn project_xml(s: &Session, stems: &[Stem]) -> String {
    let by_track: BTreeMap<TrackId, &Stem> = stems.iter().map(|s| (s.track, s)).collect();
    let mut out = String::new();
    macro_rules! line {
        ($($arg:tt)*) => {{
            let _ = write!(out, $($arg)*);
            out.push('\n');
        }};
    }
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Project version=\"1.0\">\n");
    line!("  <Application name=\"transit\" version=\"{}\"/>", env!("CARGO_PKG_VERSION"));
    let tempo = s.tempo.tempos().first().map_or(120.0, |t| t.bpm);
    let meter = s.tempo.meters().first().copied().unwrap_or(transit_time::MeterEvent { tick: 0, numerator: 4, denominator: 4 });
    line!(
        "  <Transport><Tempo unit=\"bpm\" value=\"{}\"/><TimeSignature numerator=\"{}\" denominator=\"{}\"/></Transport>",
        number(tempo),
        meter.numerator,
        meter.denominator
    );
    out.push_str("  <Structure>\n");
    let any_solo = s.tracks.iter().any(|t| !t.inactive && t.mixer.solo);
    for (index, t) in s.tracks.iter().filter(|t| exported_track(t)).enumerate() {
        let content = match t.kind {
            TrackKind::Instrument if by_track.contains_key(&t.id) => "audio notes",
            TrackKind::Instrument | TrackKind::Midi => "notes",
            _ => "audio",
        };
        let role = if t.kind == TrackKind::Master { "master" } else { "regular" };
        let (pan, _) = balance(&t.mixer.pan);
        line!(
            "    <Track contentType=\"{content}\" loaded=\"true\" id=\"track-{index}\" name=\"{}\" color=\"{}\"><Channel audioChannels=\"{}\" role=\"{role}\" solo=\"{}\" id=\"channel-{index}\">",
            escaped(&t.name),
            color(t.color),
            t.channels(),
            t.mixer.solo || (any_solo && t.mixer.solo_safe)
        );
        line!(
            "      <Mute value=\"{}\" id=\"mute-{index}\"/><Pan unit=\"normalized\" min=\"0\" max=\"1\" value=\"{}\" id=\"pan-{index}\"/><Volume unit=\"decibel\" value=\"{}\" id=\"volume-{index}\"/></Channel></Track>",
            t.mixer.mute,
            number(pan),
            number(f64::from(t.mixer.volume_db))
        );
    }
    out.push_str("  </Structure>\n  <Arrangement>\n    <Lanes timeUnit=\"beats\">\n");
    for (index, t) in s.tracks.iter().filter(|t| exported_track(t)).enumerate() {
        let stem = by_track.get(&t.id);
        if stem.is_none() && t.clips().iter().all(|c| !matches!(&c.content, ClipContent::Midi { .. })) {
            continue;
        }
        line!("      <Lanes track=\"track-{index}\"><Clips>");
        if let Some(stem) = stem {
            let end = s.content_end().max(1);
            line!(
                "        <Clip time=\"0.000000000\" duration=\"{}\" contentTimeUnit=\"seconds\" playStart=\"0.000000000\" name=\"{}\" color=\"{}\"><Audio channels=\"{}\" sampleRate=\"{}\" duration=\"{}\" timeUnit=\"seconds\"><File path=\"{}\"/></Audio></Clip>",
                number(beats(s, end)),
                escaped(&format!("{} (rendered)", t.name)),
                color(t.color),
                stem.channels,
                s.sample_rate.hz(),
                number(s.sample_rate.seconds(end)),
                escaped(&stem.path)
            );
        }
        for c in t.clips().iter().filter(|c| matches!(&c.content, ClipContent::Midi { .. })) {
            midi_clip(&mut out, s, c);
        }
        out.push_str("      </Clips></Lanes>\n");
    }
    out.push_str("    </Lanes>\n");
    let markers: Vec<_> = s.markers.iter().filter(|m| m.kind == MarkerKind::Marker && m.track.is_none()).collect();
    if !markers.is_empty() {
        out.push_str("    <Markers timeUnit=\"beats\">\n");
        for m in markers {
            line!("      <Marker time=\"{}\" name=\"{}\"/>", number(beats(s, m.start)), escaped(&m.name));
        }
        out.push_str("    </Markers>\n");
    }
    if s.tempo.tempos().len() > 1 {
        out.push_str("    <TempoAutomation timeUnit=\"beats\" unit=\"bpm\">\n      <Target/>\n");
        for t in s.tempo.tempos().iter().filter(|t| t.tick != 0) {
            line!("      <RealPoint time=\"{}\" value=\"{}\" interpolation=\"hold\"/>", number(t.tick as f64 / f64::from(PPQ)), number(t.bpm));
        }
        out.push_str("    </TempoAutomation>\n");
    }
    if s.tempo.meters().len() > 1 {
        out.push_str("    <TimeSignatureAutomation timeUnit=\"beats\">\n      <Target/>\n");
        for m in s.tempo.meters().iter().filter(|m| m.tick != 0) {
            line!(
                "      <TimeSignaturePoint time=\"{}\" numerator=\"{}\" denominator=\"{}\"/>",
                number(m.tick as f64 / f64::from(PPQ)),
                m.numerator,
                m.denominator
            );
        }
        out.push_str("    </TimeSignatureAutomation>\n");
    }
    out.push_str("  </Arrangement>\n</Project>\n");
    out
}

fn midi_clip(out: &mut String, s: &Session, c: &Clip) {
    let ClipContent::Midi { sequence } = &c.content else { return };
    let mut attrs = format!(
        "time=\"{}\" duration=\"{}\" contentTimeUnit=\"beats\" playStart=\"0.000000000\"",
        number(beats(s, c.start)),
        number(beat_len(s, c.start, c.length))
    );
    if let Some(looping) = c.looping.filter(|l| l.is_valid()) {
        let _ =
            write!(attrs, " loopStart=\"{}\" loopEnd=\"{}\"", number(beat_len(s, c.start, looping.start)), number(beat_len(s, c.start, looping.end)));
    }
    if c.muted {
        attrs.push_str(" enable=\"false\"");
    }
    if let Some(rgb) = c.color {
        let _ = write!(attrs, " color=\"{}\"", color(rgb));
    }
    out.push_str("        <Clip ");
    out.push_str(&attrs);
    out.push_str(" name=\"");
    out.push_str(&escaped(&c.name));
    out.push_str("\">\n          <Lanes>\n");
    notes(out, sequence);
    controllers(out, sequence);
    out.push_str("          </Lanes>\n        </Clip>\n");
}

fn notes(out: &mut String, sequence: &Sequence) {
    if sequence.notes.is_empty() {
        return;
    }
    out.push_str("          <Notes>\n");
    for n in &sequence.notes {
        out.push_str(&format!(
            "            <Note time=\"{}\" duration=\"{}\" channel=\"{}\" key=\"{}\" vel=\"{}\" rel=\"{}\"/>\n",
            number(n.start as f64 / f64::from(PPQ)),
            number(n.length.max(1) as f64 / f64::from(PPQ)),
            n.channel.min(15),
            n.pitch.min(127),
            number(f64::from(n.velocity.clamp(1, 127)) / 127.0),
            number(f64::from(n.release_velocity.min(127)) / 127.0)
        ));
    }
    out.push_str("          </Notes>\n");
}

fn kind_key(kind: CtrlKind) -> (u8, u8) {
    match kind {
        CtrlKind::Cc(n) => (0, n),
        CtrlKind::PitchBend => (1, 0),
        CtrlKind::ChannelPressure => (2, 0),
        CtrlKind::PolyPressure(p) => (3, p),
        CtrlKind::Program => (4, 0),
    }
}

fn controllers(out: &mut String, sequence: &Sequence) {
    let mut groups: BTreeMap<(u8, (u8, u8)), Vec<&CtrlEvent>> = BTreeMap::new();
    for event in &sequence.ctrls {
        groups.entry((event.channel.min(15), kind_key(event.kind))).or_default().push(event);
    }
    for ((channel, _key), events) in groups {
        let Some(first) = events.first() else { continue };
        let kind = first.kind;
        let (expression, controller, note) = match kind {
            CtrlKind::Cc(n) => ("channelController", Some(n.min(127)), None),
            CtrlKind::PitchBend => ("pitchBend", None, None),
            CtrlKind::ChannelPressure => ("channelPressure", None, None),
            CtrlKind::PolyPressure(p) => ("polyPressure", None, Some(p.min(127))),
            CtrlKind::Program => ("programChange", None, None),
        };
        out.push_str(&format!("          <Points unit=\"normalized\"><Target expression=\"{expression}\" channel=\"{channel}\""));
        if let Some(controller) = controller {
            out.push_str(&format!(" controller=\"{controller}\""));
        }
        if let Some(note) = note {
            out.push_str(&format!(" key=\"{note}\""));
        }
        out.push_str("/>\n");
        for event in events {
            let time = number(event.tick as f64 / f64::from(PPQ));
            if kind == CtrlKind::Program {
                out.push_str(&format!("            <IntegerPoint time=\"{time}\" value=\"{}\"/>\n", event.value.clamp(0, 127)));
            } else {
                let normalized = if kind == CtrlKind::PitchBend {
                    (f64::from(event.value.clamp(-8192, 8191)) + 8192.0) / 16384.0
                } else {
                    f64::from(event.value.clamp(0, 127)) / 127.0
                };
                out.push_str(&format!("            <RealPoint time=\"{time}\" value=\"{}\" interpolation=\"hold\"/>\n", number(normalized)));
            }
        }
        out.push_str("          </Points>\n");
    }
}

#[cfg(test)]
#[path = "dawproject_export_tests.rs"]
mod tests;
