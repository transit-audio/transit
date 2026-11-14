//! The pre-count zone through the live player (ADR 0052 slice 2): playing from a position before
//! bar 1 sounds a zone clip's notes on their samples, at callbacks of 1 and 512 frames, and the
//! click sounds on the zone's beats. No command makes a clip before bar 1 yet, so the session is
//! built with one directly.
use super::*;
use std::sync::Mutex;
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipId, Insert, TrackKind};

/// One beat at 120 BPM and 48 kHz.
const BEAT: i64 = 24_000;

/// Note-ons heard, as timeline samples: the block's transport position plus the event's offset.
#[derive(Default)]
struct Log {
    at: Option<i64>,
    ons: Vec<(i64, u8)>,
}

struct Listener {
    inner: Box<dyn transit_dsp::Plugin>,
    log: Arc<Mutex<Log>>,
}

impl transit_dsp::Plugin for Listener {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
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
    fn set_transport(&mut self, t: &transit_dsp::Transport) {
        self.log.lock().unwrap().at = t.playing.then_some(t.samples);
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn note_on(&mut self, offset: usize, note: u8, _velocity: u8) {
        let mut log = self.log.lock().unwrap();
        if let Some(at) = log.at {
            log.ons.push((at + offset as i64, note));
        }
    }
}

/// A zone clip from bar 0 to bar 2 whose notes sit on the zone's last beat and after bar 1.
fn zone_session() -> (Session, Vec<(i64, u8)>) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut seq = Sequence::default();
    // Clip ticks from its start at tick -3840: the zone's last beat, a tick-3000 note, bar 1, after it.
    let notes = [(2880, 60u8), (3000, 62), (3840, 64), (4000, 65)];
    for (tick, pitch) in notes {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start: tick, length: 60 });
    }
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(ClipId(1), "zone", -4 * BEAT, 8 * BEAT, seq));
    tr.instrument = Some(Insert::new("clap:test.listener"));
    let want = notes.iter().map(|(tick, pitch)| (-4 * BEAT + tick * 25, *pitch)).collect();
    (s, want)
}

#[test]
fn playing_from_before_bar_1_sounds_a_zone_clips_notes_on_their_samples() {
    let (s, want) = zone_session();
    assert_eq!(want, [(-24_000, 60), (-21_000, 62), (0, 64), (4_000, 65)]);
    for callback in [1usize, 512] {
        let log = Arc::new(Mutex::new(Log::default()));
        let (mut control, mut audio, _shared) = fixture(48_000);
        control.request(Arc::new(s.clone()), None).unwrap();
        let mut factory = |spec: &InstanceSpec, generation: SlotGen| {
            if spec.id == "clap:test.listener" {
                let inner = transit_dsp::create("gain")?;
                Some(NewInstance { plugin: Box::new(Listener { inner, log: Arc::clone(&log) }), refused_state: false })
            } else {
                make(spec, generation)
            }
        };
        control.service(&mut factory);
        let mut out = vec![0.0f32; callback * 2];
        fill(&mut audio, &mut out);
        control.service(&mut factory);
        control.send_action(Command::Play { from: -BEAT - 100, end: None, looped: None }, true).unwrap();
        for _ in 0..(BEAT as usize + 10_000) / callback {
            fill(&mut audio, &mut out);
        }
        assert_eq!(log.lock().unwrap().ons, want, "callbacks of {callback}");
        control.retire(audio.plans, audio.actions);
    }
}

/// Playing from bar 0 with Click on: a blip starts on each of the zone's four beats (the first at
/// tick -3840) and on bar 1, and nothing sounds between them. Compared in process, never by hash.
#[test]
fn the_click_sounds_on_the_zones_beats() {
    let mut s = Session::default();
    s.edit.click = true;
    let (mut control, mut audio, _shared) = fixture(48_000);
    control.request(Arc::new(s), None).unwrap();
    control.service(&mut make);
    let mut out = vec![0.0f32; 480 * 2];
    fill(&mut audio, &mut out);
    control.service(&mut make);
    control.send_action(Command::Play { from: -4 * BEAT, end: None, looped: None }, true).unwrap();
    let mut heard = Vec::new();
    for _ in 0..(5 * BEAT as usize + 480) / 480 {
        fill(&mut audio, &mut out);
        heard.extend(out.as_chunks::<2>().0.iter().map(|f| f[0].abs().max(f[1].abs())));
    }
    let peak = |a: i64| heard[a as usize..a as usize + 200].iter().fold(0.0f32, |m, x| m.max(*x));
    for beat in 0..5 {
        assert!(peak(beat * BEAT) > 0.05, "beat {beat} from bar 0 should click: {}", peak(beat * BEAT));
        assert_eq!(peak(beat * BEAT + BEAT / 2), 0.0, "silent between beats");
    }
    control.retire(audio.plans, audio.actions);
}
