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
    assert_eq!(t.mixer.pan, vec![-1.0, 0.0], "a quarter-left balance moves the right side to the center");
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

// ─── The refutation review of the first commit (ADR 0009): each finding fails without its fix ───

fn err_of(xml: &str) -> String {
    read_dawproject(archive(xml), "x").err().map(|e| e.to_string()).unwrap_or_default()
}

fn notes_track(clips: &str) -> String {
    project(PIANO, &format!(r#"<Lanes track="t1"><Clips>{clips}</Clips></Lanes>"#))
}

#[test]
fn b1_open_respects_the_automation_read_root() {
    let base = std::env::temp_dir().join(format!("transit-dawproject-root-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("in")).unwrap();
    std::fs::create_dir_all(base.join("outside")).unwrap();
    let secret = base.join("outside/secret.dawproject");
    std::fs::write(&secret, archive(&project(PIANO, "")).into_inner()).unwrap();
    let roots = crate::file_access::AutomationRoots::new(Some(&base.join("in")), None).unwrap().unwrap();
    let mut e = Engine::new(Session::new("before", transit_time::SampleRate::default()));
    let r = crate::file_access::confine(Some(&roots), || e.execute("session.open", &serde_json::json!({"path": secret.to_string_lossy()})));
    let err = r.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("outside the read root"), "{err}");
    assert_eq!(e.session().name, "before");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn b2_deep_nesting_is_refused_before_it_can_overflow_the_stack() {
    // Run where the stack is small, as on Windows' 1 MiB main thread.
    let deep_clips = || {
        let n = 5_000;
        let inner = format!("{}<Notes/>{}", r#"<Clip time="0" duration="1"><Clips>"#.repeat(n), "</Clips></Clip>".repeat(n));
        notes_track(&inner)
    };
    let deep_lanes = || project(PIANO, &format!("{}{}", "<Lanes>".repeat(12_000), "</Lanes>".repeat(12_000)));
    for xml in [deep_clips(), deep_lanes()] {
        let err = std::thread::Builder::new().stack_size(1 << 20).spawn(move || err_of(&xml)).unwrap().join().unwrap();
        assert!(err.contains("nested more than"), "{err}");
    }
}

#[test]
fn b3_an_element_with_too_many_attributes_is_refused_quickly() {
    let attrs: String = (0..80_000).map(|i| format!(" a{i}=\"1\"")).collect();
    let t = std::time::Instant::now();
    let err = err_of(&project(&format!("<Track{attrs}/>"), ""));
    assert!(err.contains("attributes"), "{err}");
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn b3_meter_changes_in_any_order_apply_quickly_and_in_time_order() {
    let points: String =
        (0..4_000).rev().map(|i| format!(r#"<TimeSignaturePoint time="{}" numerator="{}" denominator="4"/>"#, i * 12, 3 + i % 2)).collect();
    let xml = project(PIANO, "").replace("<Markers>", &format!("<TimeSignatureAutomation>{points}</TimeSignatureAutomation><Markers>"));
    let t = std::time::Instant::now();
    let (s, report) = read(&xml);
    assert!(t.elapsed() < std::time::Duration::from_secs(5), "{:?}", t.elapsed());
    assert!(report.is_empty(), "{report:?}");
    let m = s.tempo.meters();
    assert_eq!(m.len(), 4_000);
    assert_eq!((m[1].tick, m[1].numerator), (12 * 960, 4));
}

#[test]
fn b3_tracks_clips_and_markers_are_capped() {
    let tracks: String =
        (0..MAX_TRACKS + 5).map(|i| format!(r#"<Track contentType="notes" id="t{i}" name="T"><Channel role="regular"/></Track>"#)).collect();
    let (s, report) = read(&project(&tracks, ""));
    assert_eq!(s.tracks.len(), MAX_TRACKS);
    assert!(report.iter().any(|r| r.ends_with("5 tracks past the limit")), "{report:?}");
    let clips: String = (0..MAX_CLIPS + 3).map(|i| format!(r#"<Clip time="{i}" duration="1"><Notes/></Clip>"#)).collect();
    let (s, report) = read(&notes_track(&clips));
    assert_eq!(s.tracks[0].clips().len(), MAX_CLIPS);
    assert!(report.iter().any(|r| r.ends_with("3 clips past the limit")), "{report:?}");
    let markers: String = (0..MAX_MARKERS + 2).map(|i| format!(r#"<Marker time="{i}" name="m"/>"#)).collect();
    let (s, report) = read(&project(PIANO, "").replace(r#"<Marker time="3" name="B section"/>"#, &markers));
    assert_eq!(s.markers.len(), MAX_MARKERS);
    assert!(report.iter().any(|r| r.ends_with("2 markers past the limit")), "{report:?}");
}

#[test]
fn b3_loop_passes_cost_what_they_play_not_the_whole_content() {
    // 20,000 notes after a one-tick loop: every pass used to scan all of them.
    let notes: String = (0..20_000).map(|i| format!(r#"<Note time="{}" duration="0.5" key="60" vel="1"/>"#, 10 + i)).collect();
    let clip = format!(
        r#"<Clip time="0" duration="520" loopStart="0" loopEnd="0.001"><Notes><Note time="0" duration="0.001" key="61" vel="1"/>{notes}</Notes></Clip>"#
    );
    let t = std::time::Instant::now();
    let (s, _) = read(&notes_track(&clip));
    assert!(t.elapsed() < std::time::Duration::from_secs(3), "{:?}", t.elapsed());
    assert_eq!(seq(only_clip(&s)).notes.len(), 499_200, "one note per one-tick pass");
}

#[test]
fn b3_timbre_ownership_of_80_000_overlapping_notes_is_one_sweep() {
    // One long note with `n` timbre points over `n` short notes that each have one. A ratio of two
    // runs' times measured the growth, but a wall-clock ratio fails on a busy machine (E49), so a
    // far ceiling stands guard instead.
    let run = |n: u32| {
        let long: String = (0..n).map(|i| format!(r#"<RealPoint time="{}" value="0.5"/>"#, f64::from(i) * 0.5 + 0.25)).collect();
        let shorts: String = (0..n)
            .map(|i| format!(r#"<Note time="{}" duration="0.1" key="72" vel="1"><Points><Target expression="timbre"/><RealPoint time="0" value="0.9"/></Points></Note>"#, f64::from(i) * 0.5))
            .collect();
        let len = n / 2;
        let clip = format!(
            r#"<Clip time="0" duration="{}"><Notes><Note time="0" duration="{len}" key="60" vel="1"><Points><Target expression="timbre"/>{long}</Points></Note>{shorts}</Notes></Clip>"#,
            len + 1
        );
        let xml = notes_track(&clip);
        let t = std::time::Instant::now();
        let (s, _) = read(&xml);
        let took = t.elapsed();
        // The long note's points fall between the short notes, so all of them are its own.
        assert_eq!(seq(only_clip(&s)).ctrls.iter().filter(|e| e.kind == CtrlKind::Cc(74)).count(), 2 * n as usize);
        took
    };
    let took = run(80_000);
    // timing: stall-bound a ceiling for a starved runner; this run took 0.13 s on an M-series Mac
    assert!(took < std::time::Duration::from_secs(5), "{took:?}");
}

#[test]
fn b4_carried_controller_values_count_toward_the_event_cap() {
    // 2,048 controller lanes (16 channels x 128 CCs) with one point each before 2,000 clips: every
    // clip carries all of them in, about 4.1 million events from a small file.
    let points: String = (0..16 * 128)
        .map(|i| {
            format!(
                r#"<Points><Target expression="channelController" channel="{}" controller="{}"/><RealPoint time="0" value="0.5"/></Points>"#,
                i / 128,
                i % 128
            )
        })
        .collect();
    let clips: String = (0..2_000).map(|i| format!(r#"<Clip time="{}" duration="1"><Notes/></Clip>"#, i + 1)).collect();
    let err = err_of(&project(PIANO, &format!(r#"<Lanes track="t1"><Clips>{clips}</Clips>{points}</Lanes>"#)));
    assert!(err.contains("more than"), "the carried values were not counted: {err:?}");
}

#[test]
fn b5_bitwig_groups_are_groups_not_master_tracks() {
    // Bitwig 6 writes a group as contentType="tracks" with role="master".
    let structure = r#"<Track contentType="tracks" id="g" name="Strings"><Channel role="master" id="gc"><Volume unit="linear" value="0.5"/></Channel>
        <Track contentType="notes" id="v" name="Vln"><Channel role="regular"/></Track></Track>
      <Track contentType="tracks" id="g2" name="Brass"><Channel role="master" id="gc2"/></Track>"#;
    let (s, report) = read(&project(structure, ""));
    let kinds: Vec<_> = s.tracks.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(kinds, vec![("Vln", TrackKind::Instrument)]);
    assert_eq!(report, vec!["Not imported from the DAWproject: 2 group, effect, VCA and other tracks"]);
}

#[test]
fn n1_bitwig_pressure_in_linear_units_imports() {
    let clip = r#"<Clip time="0" duration="4"><Notes><Note time="0" duration="2" key="60" vel="1">
        <Points unit="linear"><Target expression="pressure"/><RealPoint time="0" value="0"/><RealPoint time="1" value="0.85"/></Points>
      </Note></Notes></Clip>"#;
    let (s, report) = read(&notes_track(clip));
    assert!(report.is_empty(), "{report:?}");
    let got: Vec<_> = seq(only_clip(&s)).ctrls.iter().map(|e| (e.tick, e.kind, e.value)).collect();
    assert_eq!(got, vec![(0, CtrlKind::PolyPressure(60), 0), (960, CtrlKind::PolyPressure(60), 108)]);
}

#[test]
fn n2_nothing_is_dropped_without_a_count() {
    // Two expressions on one note, wrapped in Lanes as the schema requires.
    let wrapped = r#"<Clip time="0" duration="4"><Notes><Note time="0" duration="2" key="60" vel="1"><Lanes>
        <Points><Target expression="pressure"/><RealPoint time="0" value="1"/></Points>
        <Points><Target expression="timbre"/><RealPoint time="0" value="1"/></Points></Lanes></Note></Notes></Clip>"#;
    let (s, _) = read(&notes_track(wrapped));
    let kinds: Vec<_> = seq(only_clip(&s)).ctrls.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, vec![CtrlKind::PolyPressure(60), CtrlKind::Cc(74)]);
    // Scenes, a bare channel, a video clip, boolean points and overlapping clips are counted.
    let lanes = r#"<Lanes track="t1"><Clips>
        <Clip time="0" duration="4"><Notes><Note time="3" duration="1" key="60" vel="1"/></Notes></Clip>
        <Clip time="2" duration="4"><Notes><Note time="0" duration="1" key="72" vel="1"/></Notes></Clip>
        <Clip time="8" duration="4"><Video path="v.mp4"/></Clip></Clips>
        <Points><Target expression="channelController" channel="0" controller="64"/><BoolPoint time="0" value="true"/></Points></Lanes>"#;
    let xml = project(&format!(r#"{PIANO}<Channel role="effect" id="fx"><Devices><BuiltinDevice deviceName="Reverb"/></Devices></Channel>"#), lanes).replace(
        "</Arrangement>",
        r#"</Arrangement><Scenes><Scene name="A"><Lanes><ClipSlot track="t1"><Clip time="0" duration="4"><Notes/></Clip></ClipSlot></Lanes></Scene></Scenes>"#,
    );
    let (_, report) = read(&xml);
    for want in [
        "1 channels outside tracks",
        "1 devices (plug-ins and built-in)",
        "1 clip launcher clips",
        "1 video clips",
        "1 controller points of other kinds",
        "1 overlapping clips (the later one covers the earlier)",
    ] {
        assert!(report.iter().any(|r| r.ends_with(want)), "{want:?} missing from {report:?}");
    }
}

#[test]
fn n3_points_timed_in_seconds_are_counted_not_read_as_beats() {
    let lanes = r#"<Lanes track="t1"><Clips><Clip time="0" duration="8"><Notes>
        <Note time="0" duration="2" key="60" vel="1"><Points timeUnit="seconds"><Target expression="pressure"/><RealPoint time="1" value="1"/></Points></Note>
      </Notes></Clip></Clips>
      <Points timeUnit="seconds"><Target expression="channelController" channel="0" controller="1"/><RealPoint time="3" value="1"/></Points></Lanes>"#;
    let (s, report) = read(&project(PIANO, lanes));
    assert!(seq(only_clip(&s)).ctrls.is_empty(), "{:?}", seq(only_clip(&s)).ctrls);
    assert!(report.iter().any(|r| r.ends_with("2 timelines timed in seconds")), "{report:?}");
}

#[test]
fn n4_a_fader_at_minus_infinity_stays_down() {
    let structure = r#"<Track contentType="notes" id="t1" name="P"><Channel role="regular"><Volume unit="decibel" value="-inf"/></Channel></Track>"#;
    let (s, report) = read(&project(structure, ""));
    assert_eq!(s.tracks[0].mixer.volume_db, -144.0);
    assert!(report.is_empty(), "{report:?}");
}

#[test]
fn n5_play_stop_ends_the_window() {
    let clip = r#"<Clip time="0" duration="8" playStart="0" playStop="4"><Notes>
        <Note time="1" duration="1" key="60" vel="1"/><Note time="6" duration="1" key="64" vel="1"/></Notes></Clip>"#;
    let (s, _) = read(&notes_track(clip));
    let pitches: Vec<_> = seq(only_clip(&s)).notes.iter().map(|n| n.pitch).collect();
    assert_eq!(pitches, vec![60]);
}

#[test]
fn n6_a_clip_before_zero_keeps_its_notes_in_place() {
    let clip = r#"<Clip time="-4" duration="8"><Notes>
        <Note time="1" duration="1" key="60" vel="1"/><Note time="4" duration="1" key="64" vel="1"/></Notes></Clip>"#;
    let (s, report) = read(&notes_track(clip));
    let c = only_clip(&s);
    assert_eq!((c.start, c.length), (0, 144_000), "the part after zero: 4 beats at 80 BPM");
    let got: Vec<_> = seq(c).notes.iter().map(|n| (n.start, n.pitch)).collect();
    assert_eq!(got, vec![(0, 64)], "the note at absolute beat 0 stays at 0");
    assert!(report.iter().any(|r| r.ends_with("1 notes before the start of the arrangement")), "{report:?}");
}

// ─── The second refutation review (Kimi, on 9ca2a905): each finding fails without its fix ───

#[test]
fn k1_the_parser_cannot_be_made_to_reserve_more_than_the_caps() {
    // roxmltree reserves a node per `<` and an attribute per `=` in the whole text before parsing.
    let equals = format!("<Project>{}</Project>", "=".repeat(MAX_EQUALS + 1));
    assert!(err_of(&equals).contains("too many"), "{}", err_of(&equals));
    let nodes = format!("<Project>{}</Project>", "<a/>".repeat(MAX_NODES as usize));
    assert!(err_of(&nodes).contains("too many"), "{}", err_of(&nodes));
}

#[test]
fn k1_shape_variants_still_hit_the_caps() {
    let deep = format!("{}{}", "<x>".repeat(70), "</x>".repeat(70));
    for xml in [
        format!("\u{feff}<!-- <x> --><Project><![CDATA[<x>]]><?p <x>?><Lanes>{deep}</Lanes></Project>"),
        format!("<?xml version=\"1.0\"?><Project><Lanes>  {deep}  </Lanes></Project>"),
    ] {
        let err = std::thread::Builder::new().stack_size(1 << 20).spawn(move || err_of(&xml)).unwrap().join().unwrap();
        assert!(err.contains("nested more than"), "{err}");
    }
    let attrs: String = (0..65).map(|i| format!(" a{i} = '1 > 0'")).collect();
    assert!(err_of(&format!("<Project{attrs}/>")).contains("attributes"));
    for xml in ["<Project><!--", "<Project><![CDATA[x", "<Project><?p", "<Project a='x>", "<Project></Project></x>"] {
        assert!(!err_of(xml).is_empty(), "{xml}");
    }
}

#[test]
fn k2_xml_booleans_written_as_digits() {
    let structure = PIANO.replace(r#"solo="true""#, r#"solo="1""#).replace(r#"<Mute value="true""#, r#"<Mute value="1""#);
    let clip = r#"<Clip time="0" duration="1" enable="0"><Notes><Note time="0" duration="1" key="60"/></Notes></Clip>"#;
    let (s, _) = read(&project(&structure, &format!(r#"<Lanes track="t1"><Clips>{clip}</Clips></Lanes>"#)));
    assert_eq!((s.tracks[0].mixer.mute, s.tracks[0].mixer.solo, only_clip(&s).muted), (true, true, true));
}

#[test]
fn k3_program_changes_as_integer_points() {
    let clip =
        r#"<Clip time="0" duration="1"><Points><Target expression="programChange" channel="0"/><IntegerPoint time="0" value="5"/></Points></Clip>"#;
    let (s, report) = read(&notes_track(clip));
    let got: Vec<_> = seq(only_clip(&s)).ctrls.iter().map(|e| (e.tick, e.kind, e.value)).collect();
    assert_eq!(got, vec![(0, CtrlKind::Program, 5)]);
    assert!(report.is_empty(), "{report:?}");
}

#[test]
fn k4_timbre_ownership_crosses_sibling_notes_timelines() {
    let clip = r#"<Clip time="0" duration="4"><Lanes>
      <Notes><Note time="0" duration="2" key="60"><Points><Target expression="timbre"/><RealPoint time="1.25" value="0.2"/></Points></Note></Notes>
      <Notes><Note time="1" duration="0.5" key="67"><Points><Target expression="timbre"/><RealPoint time="0" value="0.8"/></Points></Note></Notes>
    </Lanes></Clip>"#;
    let (s, _) = read(&notes_track(clip));
    let got: Vec<_> = seq(only_clip(&s)).ctrls.iter().filter(|e| e.kind == CtrlKind::Cc(74)).map(|e| (e.tick, e.value)).collect();
    assert_eq!(got, vec![(960, 102)], "the older note's point at 1.25 must not fight the newer sounding note");
}

#[test]
fn k5_a_clip_slot_in_the_arrangement_is_counted() {
    let lanes =
        r#"<Lanes track="t1"><ClipSlot><Clip time="0" duration="1"><Notes><Note time="0" duration="1" key="60"/></Notes></Clip></ClipSlot></Lanes>"#;
    let (s, report) = read(&project(PIANO, lanes));
    assert!(s.tracks[0].clips().is_empty());
    assert_eq!(report, vec!["Not imported from the DAWproject: 1 clip launcher clips"]);
}

#[test]
fn k6_an_empty_clip_wholly_before_zero_is_counted() {
    let (s, report) = read(&notes_track(r#"<Clip time="-4" duration="2"><Notes/></Clip>"#));
    assert!(s.tracks[0].clips().is_empty());
    assert_eq!(report, vec!["Not imported from the DAWproject: 1 clips before the start of the arrangement"]);
}

/// What a track's clips hold, in order: start and length in samples, name, mute, notes and
/// controller ticks (ids aside).
fn layout(s: &Session) -> Vec<(i64, i64, String, bool, Vec<(i64, i64, u8)>, Vec<i64>)> {
    s.tracks
        .iter()
        .flat_map(|t| t.clips())
        .map(|c| {
            let q = seq(c);
            (
                c.start,
                c.length,
                c.name.clone(),
                c.muted,
                q.notes.iter().map(|n| (n.start, n.length, n.pitch)).collect(),
                q.ctrls.iter().map(|e| e.tick).collect(),
            )
        })
        .collect()
}

#[test]
fn overlapping_clips_resolve_as_place_clip_does() {
    // (start, length) in beats, in file order: nested, chained, sharing a start (the later covers),
    // covering several at once, inside one that was itself split, and one muted.
    let shapes = [(0, 12), (2, 2), (3, 4), (3, 2), (8, 6), (1, 13), (5, 1), (20, 2), (21, 3)];
    let notes = |len: i64, key: i64| -> String {
        (0..len * 2).map(|i| format!(r#"<Note time="{}" duration="1.25" key="{}" vel="1"/>"#, f64::from(i as u32) * 0.5, key + i % 12)).collect()
    };
    let clips: String = shapes
        .iter()
        .enumerate()
        .map(|(k, &(at, len))| {
            let muted = if k == 4 { r#" enable="false""# } else { "" };
            format!(r#"<Clip time="{at}" duration="{len}" name="c{k}"{muted}><Notes>{}</Notes></Clip>"#, notes(len, 40 + 5 * k as i64))
        })
        .collect();
    let (s, _) = read(&notes_track(&clips));
    // The oracle: each clip placed by the editor's own overwrite (`place_clip`), in start order
    // (stable: of two at one start, the later in the file covers).
    let (mut want, _) = read(&notes_track(""));
    let tid = want.tracks[0].id;
    let mut order: Vec<usize> = (0..shapes.len()).collect();
    order.sort_by_key(|&k| shapes[k].0);
    for k in order {
        let (at, len) = shapes[k];
        let one = format!(r#"<Clip time="0" duration="{len}"><Notes>{}</Notes></Clip>"#, notes(len, 40 + 5 * k as i64));
        let (alone, _) = read(&notes_track(&one));
        let mut clip = only_clip(&alone).clone();
        clip.start = want.tempo.tick_to_samples(ticks(at as f64), want.sample_rate);
        clip.id = want.new_clip_id();
        clip.name = format!("c{k}");
        clip.muted = k == 4;
        crate::edit::place_clip(&mut want, tid, clip);
    }
    let names: Vec<String> = layout(&s).into_iter().map(|c| c.2).collect();
    assert_eq!(names, ["c0", "c5", "c1", "c3", "c6", "c2", "c5", "c4", "c7", "c8"], "the visible parts, worked by hand");
    assert_eq!(layout(&s), layout(&want));
}

#[test]
fn a_long_dense_clip_under_thousands_of_short_clips_imports_in_one_sweep() {
    // Maintainer's probe on #377: every short clip inside a long one cloned the rest of the long
    // clip (twice per edge), outside the event cap. Here: a one-tick loop unrolled over 2,000 beats
    // (1,920,000 notes) and 8,000 one-tick clips on ticks 1 to 8,000, so the rest stays long.
    let tick = 1.0 / f64::from(PPQ);
    let shorts: String = (1..=8_000)
        .map(|k| {
            format!(
                r#"<Clip time="{}" duration="{tick}"><Notes><Note time="0" duration="{tick}" key="72" vel="1"/></Notes></Clip>"#,
                f64::from(k) * tick
            )
        })
        .collect();
    let long =
        r#"<Clip time="0" duration="2000" loopStart="0" loopEnd="0.001"><Notes><Note time="0" duration="0.001" key="60" vel="1"/></Notes></Clip>"#;
    let xml = notes_track(&format!("{long}{shorts}"));
    let t = std::time::Instant::now();
    let (s, report) = read(&xml);
    // timing: stall-bound a ceiling for a starved runner; the sweep takes well under a second; the per-clip overwrite took 17 s with half the short clips on an M-series Mac
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "{:?}", t.elapsed());
    let clips: Vec<_> = s.tracks.iter().flat_map(|t| t.clips()).collect();
    assert_eq!(clips.len(), 8_002, "the long clip's first tick, 8,000 short clips, the long clip's rest");
    let notes: usize = clips.iter().map(|c| seq(c).notes.len()).sum();
    assert_eq!(notes, 1 + (1_920_000 - 8_001) + 8_000, "a loop note on every visible tick, one note per short clip");
    assert!(report.iter().any(|r| r.ends_with("8000 overlapping clips (the later one covers the earlier)")), "{report:?}");
}
