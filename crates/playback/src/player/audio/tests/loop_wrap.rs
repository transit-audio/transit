//! #422 through the live player: the transport's cycle wraps a looped one-bar kick clip at the
//! maintainer's numbers (48 kHz, 102.5 BPM, an eight-bar loop of 899,122 samples, the clip loop end
//! stored as 112,390), and a counting instrument hears exactly one kick per beat per pass.
use super::*;
use std::sync::Mutex;
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipId, ClipLoop, Insert, TrackKind};
use transit_time::{Range, TICKS_PER_QUARTER, TempoMap};

const KICK: u8 = 36;
const BAR: i64 = 4 * TICKS_PER_QUARTER;

/// Note-ons heard, as output frames counted from the first block the transport played from 0.
#[derive(Default)]
struct Log {
    frame: u64,
    origin: Option<u64>,
    ons: Vec<u64>,
}

struct Counter {
    inner: Box<dyn transit_dsp::Plugin>,
    log: Arc<Mutex<Log>>,
}

impl transit_dsp::Plugin for Counter {
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
        let mut log = self.log.lock().unwrap();
        if t.playing && t.samples == 0 && log.origin.is_none() {
            log.origin = Some(log.frame);
        }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
        self.log.lock().unwrap().frame += frames as u64;
    }
    fn note_on(&mut self, offset: usize, note: u8, _velocity: u8) {
        let mut log = self.log.lock().unwrap();
        if note == KICK {
            let at = log.frame + offset as u64;
            log.ons.push(at);
        }
    }
}

#[test]
fn the_player_wraps_the_kick_loop_with_one_kick_per_beat_per_pass() {
    let mut s = Session { tempo: TempoMap::new(102.5, 4, 4).unwrap(), ..Session::default() };
    let sr = s.sample_rate;
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut seq = Sequence::default();
    for beat in 0..4 {
        seq.notes.push(Note { pitch: KICK, velocity: 127, release_velocity: 64, channel: 0, start: beat * TICKS_PER_QUARTER, length: 240 });
    }
    let end = s.tempo.tick_to_samples(8 * BAR, sr);
    let mut c = Clip::midi(ClipId(1), "kick", 0, end, seq);
    c.set_loop(Some(ClipLoop { start: 0, end: 112_390 }));
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(c);
    tr.instrument = Some(Insert::new("clap:test.counter"));
    assert_eq!(end, 899_122);
    s.edit.loop_playback = true;
    s.edit.loop_range = Range::new(0, end);

    let log = Arc::new(Mutex::new(Log::default()));
    let (mut control, mut audio, _shared) = fixture(48_000);
    control.request(Arc::new(s.clone()), None).unwrap();
    let mut factory = |spec: &InstanceSpec, generation: SlotGen| {
        if spec.id == "clap:test.counter" {
            let inner = transit_dsp::create("gain")?;
            Some(NewInstance { plugin: Box::new(Counter { inner, log: Arc::clone(&log) }), refused_state: false })
        } else {
            make(spec, generation)
        }
    };
    control.service(&mut factory);
    // 700 frames per callback does not divide the loop: the wrap falls inside a callback.
    let mut out = vec![0.0f32; 700 * 2];
    fill(&mut audio, &mut out);
    control.service(&mut factory);
    control.send_action(Command::Play { from: 0, end: None, looped: Some(Range { start: 0, end }) }, true).unwrap();
    let passes = 3u64;
    for _ in 0..(passes * end as u64 / 700 + 2) {
        fill(&mut audio, &mut out);
    }
    let log = log.lock().unwrap();
    let origin = log.origin.expect("the transport played from 0");
    let heard: Vec<u64> = log.ons.iter().map(|at| at - origin).filter(|at| *at < passes * end as u64).collect();
    let tempo = &s.tempo;
    let want: Vec<u64> =
        (0..passes).flat_map(|p| (0..32).map(move |b| p * end as u64 + tempo.tick_to_samples(b * TICKS_PER_QUARTER, sr) as u64)).collect();
    assert_eq!(heard, want, "every kick the instrument heard, by output frame from the start");
    control.retire(audio.plans, audio.actions);
}
