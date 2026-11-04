//! Bounce destinations, source choices, takes, and undo; no device or real-time assertions.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use transit_audio_io::{AudioBuffer, EncodeOptions};
use transit_engine::{Engine, io};
use transit_model::{ChannelFormat, Clip, Session, TrackId, TrackKind};

fn fixture() -> (Engine, TrackId) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!("transit-bounce-{}-{}.wav", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let buffer = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 9600]] };
    std::fs::write(&path, transit_audio_io::encode(&buffer, &EncodeOptions::default()).unwrap()).unwrap();
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"kind": "audio", "name": "Vox"})).unwrap()["tracks"][0].as_u64().unwrap());
    let plan = io::Recording { start: 1200, pass: Some(4800), punch: None, rate: 48_000, takes: vec![], midi_journal: None, midi_placed: false };
    let clips = io::place_recording_take(&mut e, &plan, vec![(t, path.clone())], true);
    std::fs::remove_file(path).unwrap();
    assert_eq!(clips.len(), 2);
    assert_eq!(e.session().track(t).unwrap().take_stacks()[0].takes.len(), 2);
    e.execute("mix.insert", &json!({"track": t.0, "slot": 0, "plugin": "gain"})).unwrap();
    e.execute("mix.volume", &json!({"tracks": [t.0], "db": -6.0})).unwrap();
    (e, t)
}

struct Count {
    inner: Box<dyn transit_dsp::Plugin>,
    calls: Arc<AtomicUsize>,
}

impl transit_dsp::Plugin for Count {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, block: usize, ch: usize) {
        self.inner.prepare(sr, block, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.process(io, frames);
    }
}

/// Count actual processing, not instance creation (live compilation retains idle instances).
fn process_calls(s: &Session) -> usize {
    use transit_mix::{DeviceGeneration, InstanceSpec, MixEngine, NewInstance, PlanCompiler, PlanLimits, RenderTarget, SlotGen};
    MixEngine::force_serial(true);
    let calls = Arc::new(AtomicUsize::new(0));
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: 256, out_channels: 0, in_channels: 0 };
    let mut comp = PlanCompiler::new(target, PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: SlotGen| {
        let mut inner = transit_dsp::create("gain").unwrap();
        inner.prepare(48_000.0, 256, spec.ch);
        Some(NewInstance { plugin: Box::new(Count { inner, calls: calls.clone() }), refused_state: false })
    };
    let mut env = comp.compile(Arc::new(s.clone()), comp.active(), comp.active().checked_next().unwrap(), &mut make).unwrap();
    let mut mix = MixEngine::realtime(target);
    assert_eq!(mix.adopt_plan(&mut env), transit_mix::Adoption::Adopted);
    comp.reclaim(env);
    mix.render_active(1200, 256, &mut vec![vec![0.0; 256]; 2]);
    drop(mix.into_retired());
    calls.load(Ordering::Relaxed)
}

#[test]
fn default_bounce_keeps_source_visible_preserves_takes_and_stops_plugins() {
    let (mut e, t) = fixture();
    let before = e.session().track(t).unwrap().clone();
    assert!(process_calls(e.session()) > 0, "the insert really processes before bounce");
    let steps = e.undo_history().len();
    let result = e.execute("track.bounce", &json!({"tracks": [t.0]})).unwrap();
    let dest = TrackId(result["tracks"][0].as_u64().unwrap());
    let source = e.session().track(t).unwrap();
    assert!(!source.hidden, "source must remain reachable");
    assert!(source.inactive && source.mixer.mute);
    assert_eq!(source.playlists, before.playlists);
    assert_eq!(source.take_stacks(), before.take_stacks());
    assert_eq!(source.active_playlist, before.active_playlist);
    let bounced = e.session().track(dest).unwrap();
    assert_eq!(bounced.name, "Vox bounce");
    assert_eq!(e.session().track_index(dest), e.session().track_index(t).map(|i| i + 1));
    assert_eq!(bounced.mixer.volume_db, before.mixer.volume_db);
    assert_eq!(bounced.mixer.output, before.mixer.output);
    assert_eq!(bounced.mixer.sends, before.mixer.sends);
    assert_eq!((bounced.clips()[0].start, bounced.clips()[0].length), (0, 102_000));
    assert_eq!(process_calls(e.session()), 0, "the inactive source insert no longer processes");
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(e.undo());
    assert_eq!(e.session().track(t).unwrap(), &before);
    assert!(e.session().track(dest).is_none());
}

#[test]
fn leave_and_hide_preserve_every_source_take_and_commit_is_an_alias() {
    for mode in ["leave", "hide"] {
        let (mut e, t) = fixture();
        let mut expected = e.session().track(t).unwrap().clone();
        if mode == "hide" {
            expected.hidden = true;
            expected.inactive = true;
        }
        e.execute("track.commit", &json!({"tracks": [t.0], "source": mode})).unwrap();
        assert_eq!(e.session().track(t).unwrap(), &expected, "{mode} preserves the entire track apart from its flags");
        assert_eq!(e.session().track(t).unwrap().take_stacks(), expected.take_stacks());
    }
}

#[test]
fn existing_destination_overwrites_only_its_active_playlist_and_one_undo_restores_both() {
    let (mut e, t) = fixture();
    let s = e.session_mut();
    let dest = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Print"));
    let src = s.sources[0].id;
    let id = s.new_clip_id();
    let tr = s.track_mut(dest).unwrap();
    tr.playlists[0].clips.push(Clip::audio(id, "old", src, 0, -1000, 120_000));
    tr.playlists.push(tr.playlists[0].clone());
    tr.active_playlist = 1;
    let before = e.session_arc();
    let steps = e.undo_history().len();
    let result = e.execute("track.bounce", &json!({"tracks": [t.0], "destination": dest.0})).unwrap();
    assert_eq!(result["tracks"][0], dest.0, "the supplied destination is returned");
    assert_eq!(e.session().tracks.len(), before.tracks.len(), "no new track");
    let out = e.session().track(dest).unwrap();
    assert_eq!(out.active_playlist, 1);
    assert_eq!(out.playlists[0], before.track(dest).unwrap().playlists[0]);
    assert_eq!(out.clips().iter().map(|c| (c.start, c.end())).collect::<Vec<_>>(), [(-1000, 0), (0, 102_000), (102_000, 119_000)]);
    assert_eq!(out.clips()[1].name, "Vox bounce");
    assert_eq!(out.mixer, before.track(dest).unwrap().mixer);
    assert_eq!(e.session().track(t).unwrap().playlists, before.track(t).unwrap().playlists);
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(e.undo());
    assert_eq!(e.session().tracks, before.tracks);
    assert_eq!(e.session().sources, before.sources);
}

#[test]
fn invalid_destinations_are_refused_before_rendering_or_adding_sources() {
    let (mut e, t) = fixture();
    let midi = e.session_mut().add_track(TrackKind::Midi, ChannelFormat::Stereo, Some("MIDI"));
    let hidden = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Hidden"));
    e.session_mut().track_mut(hidden).unwrap().hidden = true;
    let frozen = TrackId(e.execute("track.bounce", &json!({"tracks": [t.0], "source": "leave"})).unwrap()["tracks"][0].as_u64().unwrap());
    e.execute("track.freeze", &json!({"tracks": [frozen.0]})).unwrap();
    for (dest, reason) in [(t, "source track"), (midi, "audio track"), (frozen, "frozen"), (hidden, "hidden"), (TrackId(u64::MAX), "does not exist")]
    {
        let before = e.session_arc();
        let steps = e.undo_history().len();
        let err = e.execute("track.bounce", &json!({"tracks": [t.0], "destination": dest.0})).unwrap_err();
        assert!(err.to_string().contains(reason), "{err}");
        assert_eq!(e.session().sources, before.sources);
        assert_eq!(e.session().tracks, before.tracks);
        assert_eq!(e.undo_history().len(), steps);
    }
    // Validate the whole selection before printing the first source.
    assert!(e.execute("track.bounce", &json!({"tracks": [t.0, hidden.0], "destination": hidden.0})).is_err());
}

#[test]
fn show_hidden_defaults_to_all_hidden_and_preserves_inactive_and_mute() {
    let (mut e, t) = fixture();
    e.execute("track.bounce", &json!({"tracks": [t.0], "source": "hide"})).unwrap();
    let other = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Other"));
    e.session_mut().track_mut(other).unwrap().hidden = true;
    e.execute("track.show_hidden", &json!({"tracks": [t.0]})).unwrap();
    assert!(!e.session().track(t).unwrap().hidden);
    assert!(e.session().track(t).unwrap().inactive);
    assert!(!e.session().track(t).unwrap().mixer.mute);
    assert!(e.session().track(other).unwrap().hidden);
    e.execute("track.show_hidden", &json!({})).unwrap();
    assert!(e.session().tracks.iter().all(|t| !t.hidden));
    e.execute("track.show_hidden", &json!({})).unwrap();
}

/// #332: the print of a mono take stack (the maintainer's guitar track) is the comped take, bit for
/// bit from its first frame, then the two seconds of silence the print adds after the last clip.
#[test]
fn a_plain_bounce_of_a_take_stack_prints_the_active_take_bit_for_bit_from_frame_zero() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!("transit-bounce-332-{}-{}.wav", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    // Two passes of 4800 frames: the second (the active take) holds 0.5, then a step to -0.25.
    let signal: Vec<f32> = (0..9600).map(|i| if i < 6000 { 0.5 } else { -0.25 }).collect();
    let buffer = AudioBuffer { sample_rate: 48_000, channels: vec![signal.clone()] };
    std::fs::write(&path, transit_audio_io::encode(&buffer, &EncodeOptions::default()).unwrap()).unwrap();
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"kind": "audio", "name": "Guitar"})).unwrap()["tracks"][0].as_u64().unwrap());
    let plan = io::Recording { start: 0, pass: Some(4800), punch: None, rate: 48_000, takes: vec![], midi_journal: None, midi_placed: false };
    io::place_recording_take(&mut e, &plan, vec![(t, path.clone())], true);
    std::fs::remove_file(path).unwrap();
    let source = e.session().track(t).unwrap();
    assert_eq!(source.channels(), 1);
    assert_eq!(source.take_stacks()[0].takes.len(), 2);
    let dest = TrackId(e.execute("track.bounce", &json!({"tracks": [t.0]})).unwrap()["tracks"][0].as_u64().unwrap());
    let clip = &e.session().track(dest).unwrap().clips()[0];
    let transit_model::ClipContent::Audio { source, .. } = clip.content else { panic!("an audio clip") };
    let print = e.session().pool.get(source).unwrap().buffer.channels.clone();
    assert_eq!(print.len(), 1, "a mono track prints mono");
    let mut want = signal[4800..].to_vec();
    want.resize(4800 + 96_000, 0.0);
    let first = print[0].iter().zip(&want).position(|(g, w)| g.to_bits() != w.to_bits());
    assert_eq!(first.map(|i| (i, print[0][i], want[i])), None);
}

/// #333: an inactive source printed silence without a word. The print is refused before it renders and
/// the document stays byte for byte as it was, hidden or not, through either command id.
#[test]
fn an_inactive_source_refuses_the_bounce_and_changes_nothing() {
    for hidden in [false, true] {
        for id in ["track.bounce", "track.commit"] {
            let (mut e, t) = fixture();
            e.execute("track.make_inactive", &json!({"tracks": [t.0], "inactive": true})).unwrap();
            e.session_mut().track_mut(t).unwrap().hidden = hidden;
            let before = e.session().to_json().unwrap();
            let steps = e.undo_history().len();
            let err = e.execute(id, &json!({"tracks": [t.0]})).unwrap_err();
            let transit_engine::EngineError::BadParams(_, why) = &err else { panic!("{err:?}") };
            assert_eq!(why, "Vox is inactive: Track > Make Active first");
            assert_eq!(e.session().to_json().unwrap(), before, "{id} hidden={hidden}");
            assert_eq!(e.undo_history().len(), steps);
            assert!(io::bounce_track(&mut e, t, None, io::BounceSource::Leave).is_err());
            assert_eq!(e.session().to_json().unwrap(), before);
        }
    }
}

#[test]
fn one_inactive_source_in_a_selection_stops_the_whole_bounce_before_any_print() {
    let (mut e, t) = fixture();
    let other = TrackId(e.execute("track.new", &json!({"kind": "audio", "name": "Idle"})).unwrap()["tracks"][0].as_u64().unwrap());
    e.execute("track.make_inactive", &json!({"tracks": [other.0], "inactive": true})).unwrap();
    let before = e.session().to_json().unwrap();
    assert!(e.execute("track.bounce", &json!({"tracks": [t.0, other.0]})).is_err());
    assert_eq!(e.session().to_json().unwrap(), before, "the active source was not printed either");
    e.execute("track.make_inactive", &json!({"tracks": [other.0], "inactive": false})).unwrap();
    e.execute("track.bounce", &json!({"tracks": [t.0]})).unwrap();
}
