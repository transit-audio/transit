//! DAWproject export from the bar holding the session start (ADR 0052 decision 10, slice 2b): with
//! a pre-count zone every arrangement time moves later by whole bars so nothing sits before time 0,
//! and the stems render from that bar; without a zone the document and the stem range are what the
//! base commit wrote, compared against its `project_xml` and `midi_clip`, copied below unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_model::{Clip, ClipLoop, MarkerKind};

/// One bar of 4/4 at 120 BPM and 48 kHz.
const BAR: Samples = 96_000;

// ---- the base commit's document writer (f24fa434), for the no-zone comparison --------------

fn base_project_xml(s: &Session, stems: &[Stem]) -> String {
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
            base_midi_clip(&mut out, s, c);
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
        for t in s.tempo.tempos().iter() {
            line!("      <RealPoint time=\"{}\" value=\"{}\" interpolation=\"hold\"/>", number(t.tick as f64 / f64::from(PPQ)), number(t.bpm));
        }
        out.push_str("    </TempoAutomation>\n");
    }
    if s.tempo.meters().len() > 1 {
        out.push_str("    <TimeSignatureAutomation timeUnit=\"beats\">\n      <Target/>\n");
        for m in s.tempo.meters().iter() {
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

fn base_midi_clip(out: &mut String, s: &Session, c: &Clip) {
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
    // What the clip's left edge hides is not exported: the file gets what sounds (ADR 0052
    // decision 17, slice 6a). A session file keeps it; the DAWproject clip starts at its edge.
    let mut shown = None;
    if c.holds_left_hidden() {
        let mut kept = sequence.clone();
        kept.notes.retain(|n| !transit_mix::left_hidden(c, n.start));
        kept.ctrls.retain(|e| !transit_mix::left_hidden(c, e.tick));
        shown = Some(kept);
    }
    let sequence = shown.as_ref().unwrap_or(sequence);
    notes(out, sequence);
    controllers(out, sequence);
    out.push_str("          </Lanes>\n        </Clip>\n");
}

// ---- fixtures ------------------------------------------------------------------------------

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

/// An instrument track with one looped MIDI clip from `start` for `length`, a tempo and meter
/// change at bar 3 and a marker at bar 2.
fn session(start: Samples, length: Samples) -> (Session, TrackId) {
    let mut s = Session::new("Song", transit_time::SampleRate::HZ_48000);
    s.tempo.set_tempo(7680, 100.0).unwrap();
    s.tempo.set_meter(7680, 3, 4).unwrap();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    s.track_mut(t).unwrap().instrument = Some(transit_model::Insert::new(transit_model::DEFAULT_INSTRUMENT));
    let seq = Sequence {
        notes: vec![note(60, 0, 960), note(64, 1920, 480)],
        ctrls: vec![CtrlEvent { tick: 0, channel: 0, kind: CtrlKind::Cc(64), value: 127 }],
    };
    let mut clip = Clip::midi(s.new_clip_id(), "m", start, length, seq);
    clip.set_loop(Some(ClipLoop { start: 0, end: length }));
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(clip);
    s.add_marker("Verse", MarkerKind::Marker, BAR, BAR);
    (s, t)
}

fn stem(s: &Session, track: TrackId) -> Stem {
    Stem { track, path: "audio/track-0.wav".into(), disk: PathBuf::new(), channels: bounce_layout(s, None, false).1.max(1) }
}

/// Every `time` attribute of the document's `tag` elements, in document order.
fn times(xml: &str, tag: &str) -> Vec<f64> {
    let doc = roxmltree::Document::parse(xml).unwrap();
    doc.descendants().filter(|n| n.has_tag_name(tag)).filter_map(|n| n.attribute("time")).map(|t| t.parse().unwrap()).collect()
}

fn zip_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Vec<u8> {
    use std::io::Read as _;
    let mut entry = archive.by_name(name).unwrap();
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    bytes
}

fn dawproject_archive(xml: &str) -> std::io::Cursor<Vec<u8>> {
    use std::io::Write as _;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("project.xml", zip::write::SimpleFileOptions::default()).unwrap();
    w.write_all(xml.as_bytes()).unwrap();
    let mut c = w.finish().unwrap();
    c.set_position(0);
    c
}

// ---- tests ---------------------------------------------------------------------------------

/// A zone from beat 3 of bar 0: the document's time 0 is bar 0's first beat, so nothing has a
/// negative time, the clip sits two beats in, bar 1 is one whole 4/4 bar (4 beats) in, and the
/// marker, the tempo and meter changes and the rendered stem clip all move by those 4 beats. The
/// first tempo and meter point (bar 1's, extended backward) is at time 0.
#[test]
fn a_dawproject_export_with_a_zone_starts_at_the_bar_holding_the_session_start() {
    let (s, t) = session(-BAR / 2, BAR + BAR / 2);
    assert_eq!(dawproject_range(&s), Range::new(-BAR, BAR));
    let xml = project_xml(&s, &[stem(&s, t)]);
    let doc = roxmltree::Document::parse(&xml).unwrap();
    let all: Vec<f64> = doc.descendants().filter_map(|n| n.attribute("time")).map(|t| t.parse().unwrap()).collect();
    assert!(all.iter().all(|t| *t >= 0.0), "no time before 0: {all:?}");
    let clip = |name: &str| doc.descendants().find(|n| n.has_tag_name("Clip") && n.attribute("name") == Some(name)).unwrap();
    let (midi, rendered) = (clip("m"), clip("Keys (rendered)"));
    assert_eq!(midi.attribute("time"), Some("2.000000000"), "the clip two beats after the file's first bar line");
    assert_eq!(midi.attribute("duration"), Some("6.000000000"));
    assert_eq!((midi.attribute("loopStart"), midi.attribute("loopEnd")), (Some("0.000000000"), Some("6.000000000")), "clip-relative, unchanged");
    assert_eq!(
        (rendered.attribute("time"), rendered.attribute("duration")),
        (Some("0.000000000"), Some("8.000000000")),
        "the stem covers bar 0 and bar 1"
    );
    let audio = rendered.descendants().find(|n| n.has_tag_name("Audio")).unwrap();
    assert_eq!(audio.attribute("duration"), Some("4.000000000"), "seconds: from bar 0 to the content end");
    assert_eq!(times(&xml, "Note"), [0.0, 2.0], "notes stay clip-relative");
    assert_eq!(times(&xml, "Marker"), [8.0], "the marker at bar 2 is one bar later");
    let tempo = doc.descendants().find(|n| n.has_tag_name("TempoAutomation")).unwrap();
    let tempo: Vec<f64> = tempo.children().filter_map(|n| n.attribute("time")).map(|t| t.parse().unwrap()).collect();
    assert_eq!(tempo, [0.0, 12.0], "bar 1's tempo at time 0, the bar-3 change one bar later");
    assert_eq!(times(&xml, "TimeSignaturePoint"), [0.0, 12.0]);
}

/// The importer reads the shifted document back with the zone's first bar as bar 1: the clip and
/// the marker arrive one whole bar later than they were, with the same spacing.
#[test]
fn a_zone_session_round_trips_through_the_importer_shifted_by_whole_bars() {
    let (s, _) = session(-BAR / 2, BAR + BAR / 2);
    let (back, _) = read_dawproject(dawproject_archive(&project_xml(&s, &[])), "round trip").unwrap();
    let clips: Vec<_> = back.tracks.iter().flat_map(|t| t.clips()).collect();
    assert_eq!(clips.len(), 1);
    assert_eq!((clips[0].start, clips[0].length), (BAR / 2, BAR + BAR / 2), "the zone clip arrives one bar later");
    assert_eq!(back.markers.iter().map(|m| m.start).collect::<Vec<_>>(), [2 * BAR]);
    assert_eq!(back.start(), 0, "the imported session has no zone");
}

/// Through the command, with a zone: the stem in the archive runs from bar 0 (two bars, 192,000
/// samples), silent until the zone's clip half a bar in, then sounding.
#[test]
fn the_command_renders_the_stems_from_the_bar_holding_the_session_start() {
    let dir = transit_test_support::TestDir::created("dawproject-zone");
    let (s, _) = session(-BAR / 2, BAR + BAR / 2);
    let mut e = Engine::new(s);
    let path = dir.join("zone.dawproject");
    e.execute("file.export_dawproject", &serde_json::json!({"path": path.to_string_lossy()})).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let (_, audio) = transit_audio_io::decode(&zip_entry(&mut archive, "audio/track-0.wav"), Some("wav")).unwrap();
    let ch = &audio.channels[0];
    assert_eq!(ch.len() as Samples, 2 * BAR);
    let half = (BAR / 2) as usize;
    assert_eq!(ch[..half].iter().position(|x| *x != 0.0), None, "silent before the zone's clip");
    assert!(ch[half..half + half / 2].iter().any(|x| *x != 0.0), "the zone's note half a bar into the stem");
    let xml = String::from_utf8(zip_entry(&mut archive, "project.xml")).unwrap();
    assert!(times(&xml, "Clip").iter().all(|t| *t >= 0.0), "{xml}");
}

/// Without a zone the command writes the base commit's document byte for byte (the base writer
/// copied above, fed the same stems), and the stems render over the base range `0..content_end`.
#[test]
fn a_dawproject_export_without_a_zone_is_unchanged() {
    let (s, t) = session(BAR, 2 * BAR);
    assert_eq!(dawproject_range(&s), Range::new(0, s.content_end().max(1)));
    assert!(project_xml(&s, &[stem(&s, t)]) == base_project_xml(&s, &[stem(&s, t)]));
    let demo = crate::demo::demo_engine().session().clone();
    assert_eq!(dawproject_range(&demo), Range::new(0, demo.content_end().max(1)));
    assert!(project_xml(&demo, &[]) == base_project_xml(&demo, &[]), "the demo session's document");

    let dir = transit_test_support::TestDir::created("dawproject-none");
    let mut e = Engine::new(s);
    let path = dir.join("none.dawproject");
    e.execute("file.export_dawproject", &serde_json::json!({"path": path.to_string_lossy()})).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let xml = String::from_utf8(zip_entry(&mut archive, "project.xml")).unwrap();
    let s = e.session();
    assert!(xml == base_project_xml(s, &[stem(s, t)]), "the archive's document differs from the base commit's");
    let (_, audio) = transit_audio_io::decode(&zip_entry(&mut archive, "audio/track-0.wav"), Some("wav")).unwrap();
    assert_eq!(audio.channels[0].len() as Samples, s.content_end().max(1), "the stem covers the base range");
}
