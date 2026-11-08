//! DAWproject import (#261, slice 1): each mapping and each refusal, on hand-written projects.

use super::*;
use std::io::Write as _;

/// A deflated archive holding `xml` as `project.xml` (as Bitwig writes them).
fn archive(xml: &str) -> std::io::Cursor<Vec<u8>> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    w.start_file("project.xml", opts).unwrap();
    w.write_all(xml.as_bytes()).unwrap();
    let mut c = w.finish().unwrap();
    c.set_position(0);
    c
}

/// `structure` and `lanes` in a project at 80 BPM in 3/4.
fn project(structure: &str, lanes: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Project version="1.0">
  <Transport><Tempo unit="bpm" value="80"/><TimeSignature numerator="3" denominator="4"/></Transport>
  <Structure>{structure}</Structure>
  <Arrangement><Lanes timeUnit="beats">{lanes}</Lanes>
    <Markers><Marker time="3" name="B section"/></Markers>
  </Arrangement>
</Project>"#
    )
}

const PIANO: &str = r#"<Track contentType="notes" id="t1" name="Piano">
  <Channel role="regular" audioChannels="2" solo="true" id="c1">
    <Mute value="true" id="m1"/><Pan unit="normalized" value="0.25" id="p1"/><Volume unit="linear" value="0.5" id="v1"/>
  </Channel></Track>"#;

fn read(xml: &str) -> (Session, Vec<String>) {
    read_dawproject(archive(xml), "song").unwrap()
}

fn only_clip(s: &Session) -> &Clip {
    let clips: Vec<_> = s.tracks.iter().flat_map(|t| t.clips()).collect();
    assert_eq!(clips.len(), 1, "{clips:?}");
    clips[0]
}

fn seq(c: &Clip) -> &Sequence {
    match &c.content {
        ClipContent::Midi { sequence } => sequence,
        other => panic!("not MIDI: {other:?}"),
    }
}

#[test]
fn tracks_mixer_tempo_meter_markers_and_notes() {
    let lanes = r#"<Lanes track="t1"><Clips>
      <Clip time="3" duration="3" playStart="0" name="verse"><Notes>
        <Note time="0" duration="1" channel="0" key="60" vel="0.5" rel="0"/>
        <Note time="2" duration="0.5" channel="1" key="64" vel="1"/>
      </Notes></Clip></Clips></Lanes>"#;
    let (s, report) = read(&project(PIANO, lanes));
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(s.name, "song");
    assert_eq!(s.tempo.tempos()[0].bpm, 80.0);
    assert_eq!((s.tempo.meters()[0].numerator, s.tempo.meters()[0].denominator), (3, 4));
    let t = &s.tracks[0];
    assert_eq!((t.kind, t.name.as_str()), (TrackKind::Instrument, "Piano"));
    assert_eq!(t.instrument.as_ref().map(|i| i.plugin.as_str()), Some(transit_model::DEFAULT_INSTRUMENT));
    assert!((t.mixer.volume_db - -6.0206).abs() < 1e-3, "{}", t.mixer.volume_db);
    assert_eq!(t.mixer.pan, vec![-1.0, 0.0], "a quarter-left balance moves the right side to the centre");
    assert!(t.mixer.mute && t.mixer.solo);
    // Beat 3 at 80 BPM is 2.25 s.
    let c = only_clip(&s);
    assert_eq!((c.name.as_str(), c.start, c.length), ("verse", 108_000, 108_000));
    let n = &seq(c).notes;
    assert_eq!((n[0].start, n[0].length, n[0].pitch, n[0].velocity, n[0].release_velocity, n[0].channel), (0, 960, 60, 64, 0, 0));
    assert_eq!((n[1].start, n[1].length, n[1].pitch, n[1].velocity, n[1].release_velocity, n[1].channel), (1920, 480, 64, 127, 64, 1));
    assert_eq!(s.markers.len(), 1);
    assert_eq!((s.markers[0].name.as_str(), s.markers[0].start), ("B section", 108_000));
}

#[test]
fn per_note_pressure_and_timbre_with_overlaps() {
    // A sounds 0..2 beats; B starts inside it at 1 and ends at 1.5, so B owns timbre while it
    // sounds and A's point at 1.25 is left out; A's point at 1.5 is A's again.
    let lanes = r#"<Lanes track="t1"><Clips><Clip time="0" duration="4"><Notes>
      <Note time="0" duration="2" key="60" vel="1">
        <Points unit="normalized"><Target expression="timbre"/>
          <RealPoint time="0" value="0.2"/><RealPoint time="1.25" value="0.3"/><RealPoint time="1.5" value="0.4"/></Points>
        <Points unit="normalized"><Target expression="pressure"/>
          <RealPoint time="0" value="0.5"/><RealPoint time="0.5" value="0.5"/><RealPoint time="1" value="1"/></Points>
      </Note>
      <Note time="1" duration="0.5" key="67" vel="1">
        <Points unit="normalized"><Target expression="timbre"/><RealPoint time="0" value="0.8"/></Points>
        <Points unit="normalized"><Target expression="transpose"/><RealPoint time="0" value="0.5"/></Points>
      </Note></Notes></Clip></Clips></Lanes>"#;
    let (s, report) = read(&project(PIANO, lanes));
    assert_eq!(report, vec!["Not imported from the DAWproject: 1 per-note gain, pan, transpose and formant curves"]);
    let ctrls = &seq(only_clip(&s)).ctrls;
    let timbre: Vec<_> = ctrls.iter().filter(|e| e.kind == CtrlKind::Cc(74)).map(|e| (e.tick, e.value)).collect();
    assert_eq!(timbre, vec![(0, 25), (960, 102), (1440, 51)]);
    let pressure: Vec<_> = ctrls.iter().filter(|e| e.kind == CtrlKind::PolyPressure(60)).map(|e| (e.tick, e.value)).collect();
    assert_eq!(pressure, vec![(0, 64), (960, 127)], "repeated values are left out");
}

#[test]
fn track_controller_lanes_go_into_the_clips_that_cover_them() {
    // The pedal goes down before the clip (carried in at its start), up inside it; the mod wheel
    // point after every clip is counted.
    let lanes = r#"<Lanes track="t1">
      <Clips><Clip time="4" duration="4"><Notes><Note time="0" duration="1" key="60" vel="1"/></Notes></Clip></Clips>
      <Points unit="normalized"><Target expression="channelController" channel="0" controller="64"/>
        <RealPoint time="0" value="1"/><RealPoint time="6" value="0"/></Points>
      <Points unit="normalized"><Target expression="channelController" channel="0" controller="1"/>
        <RealPoint time="9" value="0.5"/></Points>
      <Points unit="normalized"><Target expression="pitchBend" channel="0"/><RealPoint time="5" value="1"/></Points>
      <Points unit="linear"><Target parameter="v1"/><RealPoint time="0" value="0.5"/></Points>
    </Lanes>"#;
    let (s, report) = read(&project(PIANO, lanes));
    assert_eq!(
        report,
        vec!["Not imported from the DAWproject: 1 controller points outside clips", "Not imported from the DAWproject: 1 parameter automation lanes"]
    );
    let got: Vec<_> = seq(only_clip(&s)).ctrls.iter().map(|e| (e.tick, e.kind, e.value)).collect();
    assert_eq!(got, vec![(0, CtrlKind::Cc(64), 127), (960, CtrlKind::PitchBend, 8191), (1920, CtrlKind::Cc(64), 0)]);
}

#[test]
fn loops_unroll_and_windows_cut() {
    // One beat of content looped over a 3.5-beat clip; the clip starts half a beat into it.
    let lanes = r#"<Lanes track="t1"><Clips>
      <Clip time="0" duration="3.5" playStart="0.5" loopStart="0" loopEnd="1"><Notes>
        <Note time="0" duration="0.25" key="60" vel="1"/><Note time="0.5" duration="1" key="62" vel="1"/>
      </Notes></Clip></Clips></Lanes>"#;
    let (s, _) = read(&project(PIANO, lanes));
    let got: Vec<_> = seq(only_clip(&s)).notes.iter().map(|n| (n.start, n.length, n.pitch)).collect();
    // The first pass plays 62 from the half beat, cut at the loop end; each later pass plays both.
    assert_eq!(got, vec![(0, 480, 62), (480, 240, 60), (960, 480, 62), (1440, 240, 60), (1920, 480, 62), (2400, 240, 60), (2880, 480, 62)]);
}

#[test]
fn nested_clips_flatten_and_muted_ones_are_left_out() {
    let lanes = r#"<Lanes track="t1"><Clips><Clip time="0" duration="8"><Clips>
      <Clip time="2" duration="2"><Notes><Note time="0" duration="1" key="60" vel="1"/></Notes></Clip>
      <Clip time="4" duration="2" enable="false"><Notes><Note time="0" duration="1" key="61" vel="1"/></Notes></Clip>
    </Clips></Clip></Clips></Lanes>"#;
    let (s, _) = read(&project(PIANO, lanes));
    let got: Vec<_> = seq(only_clip(&s)).notes.iter().map(|n| (n.start, n.pitch)).collect();
    assert_eq!(got, vec![(1920, 60)]);
}

#[test]
fn what_is_not_imported_is_counted() {
    let structure = r#"<Track contentType="notes" id="t1" name="Synth">
        <Channel role="regular" id="c1"><Devices><ClapPlugin deviceID="x" name="A"/><Vst3Plugin deviceID="y" name="B"/></Devices>
          <Sends><Send destination="c9"/></Sends></Channel></Track>
      <Track contentType="audio" id="t2" name="Vox"><Channel role="regular" audioChannels="1" id="c2"/></Track>
      <Track contentType="tracks" id="g1" name="Group"><Channel role="submix" id="c3"/>
        <Track contentType="notes" id="t3" name="Inner"><Channel role="regular" id="c4"/></Track></Track>
      <Track contentType="audio notes" id="m" name="Master"><Channel role="master" id="c5"><Volume unit="linear" value="1"/></Channel></Track>"#;
    let lanes = r#"<Lanes track="t2"><Clips><Clip time="0" duration="4"><Audio path="audio/vox.wav"/></Clip></Clips></Lanes>
      <Lanes track="g1"><Clips><Clip time="0" duration="1"><Notes><Note time="0" duration="1" key="60"/></Notes></Clip></Clips></Lanes>"#;
    let (s, report) = read(&project(structure, lanes));
    let tracks: Vec<_> = s.tracks.iter().map(|t| (t.name.as_str(), t.kind, t.format)).collect();
    assert_eq!(
        tracks,
        vec![
            ("Synth", TrackKind::Instrument, ChannelFormat::Stereo),
            ("Vox", TrackKind::Audio, ChannelFormat::Mono),
            ("Inner", TrackKind::Instrument, ChannelFormat::Stereo)
        ],
        "a master at unity adds no track"
    );
    assert_eq!(
        report,
        vec![
            "Not imported from the DAWproject: 1 audio clips",
            "Not imported from the DAWproject: 1 clips on tracks not imported",
            "Not imported from the DAWproject: 2 devices (plug-ins and built-in)",
            "Not imported from the DAWproject: 1 group, effect, VCA and other tracks",
            "Not imported from the DAWproject: 1 sends",
        ]
    );
}

#[test]
fn a_master_off_its_defaults_becomes_a_master_track() {
    let structure =
        r#"<Track contentType="audio" id="m" name="Master"><Channel role="master" id="c5"><Volume unit="linear" value="0.5"/></Channel></Track>"#;
    let (s, _) = read(&project(structure, ""));
    assert_eq!(s.tracks.len(), 1);
    assert_eq!(s.tracks[0].kind, TrackKind::Master);
    assert!((s.tracks[0].mixer.volume_db - -6.0206).abs() < 1e-3);
}

#[test]
fn hostile_and_unsupported_files_are_refused() {
    let err = |r: Result<(Session, Vec<String>)>| r.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err(read_dawproject(std::io::Cursor::new(b"not a zip".to_vec()), "x")).contains("DAWproject"));
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("metadata.xml", zip::write::SimpleFileOptions::default()).unwrap();
    let mut no_project = w.finish().unwrap();
    no_project.set_position(0);
    assert!(err(read_dawproject(no_project, "x")).contains("DAWproject"));
    assert!(err(read_dawproject(archive("<Song/>"), "x")).contains("no <Project>"));
    let laughs = r#"<?xml version="1.0"?><!DOCTYPE p [<!ENTITY a "aaaa"><!ENTITY b "&a;&a;&a;&a;">]><Project>&b;</Project>"#;
    assert!(err(read_dawproject(archive(laughs), "x")).contains("project.xml"), "no DTDs");
    let seconds = project(PIANO, "").replace(r#"<Lanes timeUnit="beats">"#, r#"<Lanes timeUnit="seconds">"#);
    assert!(err(read_dawproject(archive(&seconds), "x")).contains("seconds"));
    // A one-tick loop over a very long clip hits the event cap instead of running away.
    let runaway = r#"<Lanes track="t1"><Clips><Clip time="0" duration="1000000" loopStart="0" loopEnd="0.001"><Notes>
      <Note time="0" duration="0.001" key="60" vel="1"/></Notes></Clip></Clips></Lanes>"#;
    assert!(err(read_dawproject(archive(&project(PIANO, runaway)), "x")).contains("more than"));
    // Bad values skip their element; out-of-range times too.
    let bad = r#"<Lanes track="t1"><Clips><Clip time="0" duration="4"><Notes>
      <Note time="NaN" duration="1" key="60"/><Note time="0" duration="1" key="200"/><Note time="1e300" duration="1" key="60"/>
      <Note time="1" duration="1" key="60" channel="16"/><Note time="2" duration="1" key="61"/></Notes></Clip></Clips></Lanes>"#;
    let (s, report) = read(&project(PIANO, bad));
    assert_eq!(seq(only_clip(&s)).notes.len(), 1);
    assert_eq!(report, vec!["Not imported from the DAWproject: 4 notes with bad values"]);
}

#[test]
fn open_replaces_the_session_untitled_and_posts_the_report() {
    let dir = std::env::temp_dir().join(format!("transit-dawproject-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("piano.dawproject");
    let structure =
        format!(r#"{PIANO}<Track contentType="notes" id="t2" name="FX"><Channel role="regular" id="c2"><Sends><Send/></Sends></Channel></Track>"#);
    std::fs::write(&path, archive(&project(&structure, "")).into_inner()).unwrap();
    let mut e = Engine::new(Session::new("before", transit_time::SampleRate::default()));
    e.path = Some("/somewhere/before.transit".into());
    let v = e.execute("session.open", &serde_json::json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(v["name"], "piano");
    assert_eq!(v["tracks"], 2);
    assert_eq!(v["not_imported"], serde_json::json!(["Not imported from the DAWproject: 1 sends"]));
    assert_eq!(e.path, None, "Save asks where: the .dawproject is never overwritten");
    assert!(is_dawproject("A/B.DAWproject") && !is_dawproject("a.transit"));
    let _ = std::fs::remove_dir_all(&dir);
}
