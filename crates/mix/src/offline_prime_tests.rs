//! Issue #332: an offline print (Bounce in Place, Commit, Freeze, File > Bounce) must equal the
//! live path's output from its first frame. A live chain runs, transport stopped, from the moment
//! it is built until play is pressed; an offline chain is built for the render, so whatever a
//! plug-in does on its first blocks (an activation declick, smoothing toward the values the host
//! delivers, automation applied after its stored values) must not land in the print.

use super::*;
use transit_audio_io::AudioBuffer;
use transit_model::{Insert, SourceId};

/// 200 ms of a constant 0.5, then a step to -0.25 for another 200 ms.
const HALF: usize = 9_600;

fn signal() -> Vec<f32> {
    (0..2 * HALF).map(|i| if i < HALF { 0.5 } else { -0.25 }).collect()
}

/// A track of `fmt` holding one clip of [`signal`] on every channel from the session start.
fn session(fmt: ChannelFormat) -> (Session, TrackId) {
    let mut s = Session::default();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![signal(); fmt.channels()] };
    s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(buf)));
    let t = s.add_track(TrackKind::Audio, fmt, Some("Src"));
    let id = s.new_clip_id();
    if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
        pl.clips.push(Clip::audio(id, "src", SourceId(900), 0, 0, (2 * HALF) as i64));
    }
    (s, t)
}

fn range() -> Range {
    Range::new(0, (2 * HALF) as i64)
}

/// The first frame where `got` differs from `want`, with both values (none when they are equal).
fn first_difference(got: &[f32], want: &[f32]) -> Option<(usize, f32, f32)> {
    assert_eq!(got.len(), want.len());
    got.iter().zip(want).enumerate().find(|(_, (g, w))| g.to_bits() != w.to_bits()).map(|(i, (g, w))| (i, *g, *w))
}

#[test]
fn a_plain_print_is_the_source_bit_for_bit_from_frame_zero() {
    for fmt in [ChannelFormat::Stereo, ChannelFormat::Mono] {
        let (s, t) = session(fmt);
        let print = render_track_pre_fader(&s, t, range());
        assert_eq!(print.len(), fmt.channels(), "{fmt:?}");
        for (ch, got) in print.iter().enumerate() {
            assert_eq!(first_difference(got, &signal()), None, "{fmt:?} channel {ch}");
        }
    }
}

#[test]
#[ignore = "waits on the ADR 0038 decision for #332"]
fn an_automated_gain_insert_prints_its_lane_value_from_frame_zero() {
    let (mut s, t) = session(ChannelFormat::Stereo);
    let tr = s.track_mut(t).unwrap();
    // The stored value is off (-inf); the lane holds unity from the session start, so the print
    // is the source itself. Live, the stopped transport delivers the lane's value long before play.
    let mut gain = Insert::new("gain");
    gain.params.insert("gain".into(), -96.0);
    tr.mixer.inserts[0] = Some(gain);
    tr.lane_mut(&AutoParam::Plugin { slot: 0, param: "gain".into() }).set_point(0, 0.0);
    let want = signal();
    let print = render_track_pre_fader(&s, t, range());
    let live = render_range(&s, range(), 512);
    for ch in 0..2 {
        assert_eq!(first_difference(&print[ch], &want), None, "print channel {ch}");
        assert_eq!(first_difference(&live[ch], &want), None, "main mix channel {ch}");
    }
}

#[test]
fn a_dynamics_insert_prints_what_the_live_path_plays_from_frame_zero() {
    // The compressor's own attack from silence is what the live path plays as well.
    let (mut s, t) = session(ChannelFormat::Stereo);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("compressor"));
    let print = render_track_pre_fader(&s, t, range());
    let live = render_range(&s, range(), 1024);
    for ch in 0..2 {
        assert_eq!(first_difference(&print[ch], &live[ch]), None, "channel {ch}");
    }
}

/// Frames a [`WarmUp`] takes to reach unity after it is prepared or reset (half a second).
const WARM_UP: usize = 24_000;

/// A stand-in for a hosted plug-in that fades its output in after activation, as many do.
struct WarmUp {
    inner: Box<dyn transit_dsp::Plugin>,
    since: usize,
}

impl transit_dsp::Plugin for WarmUp {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, block: usize, ch: usize) {
        self.inner.prepare(sr, block, ch);
        self.since = 0;
    }
    fn reset(&mut self) {
        self.since = 0;
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for k in 0..frames {
            let g = (self.since + k).min(WARM_UP) as f32 / WARM_UP as f32;
            for c in io.iter_mut() {
                if let Some(x) = c.get_mut(k) {
                    *x *= g;
                }
            }
        }
        self.since = self.since.saturating_add(frames);
    }
}

#[test]
#[ignore = "waits on the ADR 0038 decision for #332"]
fn a_hosted_plug_in_warm_up_settles_before_the_first_printed_frame() {
    let (mut s, t) = session(ChannelFormat::Stereo);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.warm_up"));
    let mut make = |spec: &InstanceSpec, _: SlotGen| {
        assert_eq!(spec.id, "clap:test.warm_up");
        let mut inner = transit_dsp::create("gain")?;
        inner.prepare(48_000.0, 1024, spec.ch);
        let mut plugin = WarmUp { inner, since: 0 };
        transit_dsp::Plugin::prepare(&mut plugin, 48_000.0, 1024, spec.ch);
        Some(NewInstance { plugin: Box::new(plugin), refused_state: false })
    };
    let mut render = RangeRender::build_with(&s, None, range(), 1024, None, &mut make).unwrap();
    let mut print = vec![Vec::new(); 2];
    while let Some((chunk, n)) = render.next_block() {
        for (o, c) in print.iter_mut().zip(chunk) {
            o.extend_from_slice(&c[..n]);
        }
    }
    for (ch, got) in print.iter().enumerate() {
        assert_eq!(first_difference(got, &signal()), None, "channel {ch}");
    }
}
