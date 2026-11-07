//! The main bus monitoring set through the device callback (#366): Mono, Dim and Ref act on what
//! reaches the device only, ramp a change within one render block, never reach a render, and the
//! live loudness reads the mix and resets. Every callback here is measured for heap use.
use super::*;
use crate::monitor_path::DIM_DB;
use crate::realtime_render::{RenderTap, TapState};

const SR: usize = 48_000;
const CALLBACK: usize = 480;

/// A stereo audio track playing `left` and `right` (each `SR * seconds` long) from 0 to Main.
fn stereo_session(left: Vec<f32>, right: Vec<f32>) -> Session {
    use transit_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
    let mut s = Session::default();
    s.edit.click = false;
    let len = left.len();
    let src = SourceId(s.next_id + 10_000);
    s.pool.insert(src, Arc::new(SourceAudio::new(transit_audio_io::AudioBuffer { sample_rate: SR as u32, channels: vec![left, right] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Tone"));
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "tone", src, 0, 0, len as i64));
    s
}

fn constant(left: f32, right: f32, seconds: usize) -> Session {
    stereo_session(vec![left; SR * seconds], vec![right; SR * seconds])
}

fn sine_session(dbfs: f32, seconds: usize) -> Session {
    let amp = transit_dsp::db_to_gain(dbfs);
    let x: Vec<f32> = (0..SR * seconds).map(|i| amp * (std::f32::consts::TAU * 997.0 * i as f32 / SR as f32).sin()).collect();
    stereo_session(x.clone(), x)
}

/// A playing fixture of `session`.
fn playing(session: Session) -> (Control<Command, ActionResult>, Audio, Arc<Shared>) {
    playing_with(session, |_| ())
}

/// A playing fixture of `session` with the monitor set by `set` before the first callback (whose
/// silent block takes any ramp).
fn playing_with(session: Session, set: impl Fn(&Shared)) -> (Control<Command, ActionResult>, Audio, Arc<Shared>) {
    let (mut control, mut audio, shared) = fixture_session(48_000, Arc::new(session));
    set(&shared);
    fill(&mut audio, &mut [0.0; 8]);
    control.service(&mut make);
    control.send_action(Command::Play { from: 0, end: None, looped: None }, true).unwrap();
    (control, audio, shared)
}

/// `n` callbacks of the device output, interleaved stereo, each measured for heap use; `between`
/// runs on "Main" before callback `k` (atomic stores only, also measured).
fn callbacks(audio: &mut Audio, shared: &Shared, n: usize, mut between: impl FnMut(usize, &Shared)) -> Vec<f32> {
    let mut all = Vec::with_capacity(n * CALLBACK * 2);
    let mut out = vec![0.0f32; CALLBACK * 2];
    for k in 0..n {
        let ((), heap) = measure(|| between(k, shared));
        assert!(heap.is_zero(), "control side: {heap:?}");
        let ((), heap) = measure(|| fill(audio, &mut out));
        assert!(heap.is_zero(), "callback {k}: {heap:?}");
        all.extend_from_slice(&out);
    }
    all
}

fn ratio(a: &[f32], reference: &[f32]) -> Vec<f32> {
    a.iter().zip(reference).map(|(a, r)| if r.abs() > 1e-9 { a / r } else { f32::NAN }).collect()
}

#[test]
fn dim_cuts_the_device_output_by_20_db_within_one_render_block_and_without_a_step() {
    let reference = {
        let (control, mut audio, shared) = playing(constant(0.5, 0.25, 2));
        let out = callbacks(&mut audio, &shared, 40, |_, _| ());
        control.retire(audio.plans, audio.actions);
        out
    };
    let (control, mut audio, shared) = playing(constant(0.5, 0.25, 2));
    let dimmed = callbacks(&mut audio, &shared, 40, |k, s| {
        if k == 20 {
            s.monitor.set_dim(true);
        }
    });
    control.retire(audio.plans, audio.actions);
    let r = ratio(&dimmed, &reference);
    let cut = transit_dsp::db_to_gain(DIM_DB);
    let first = r.iter().position(|v| (v - 1.0).abs() > 1e-6).expect("Dim changed the output");
    let last = r.iter().rposition(|v| (v - cut).abs() > 1e-6).unwrap();
    assert!(first >= 2 * CALLBACK * 10, "nothing changes before Dim is pressed (sample {first})");
    // The ramp: within one render block (at most 512 frames: 1,024 interleaved samples), no step
    // larger than the whole cut spread over that block.
    assert!(last - first < 2 * 512, "the cut lands within one block: samples {first}..{last}");
    let biggest = r[first.saturating_sub(2)..=last + 2].windows(3).map(|w| (w[2] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(biggest <= (1.0 - cut) / 100.0, "a ramp, not a step: {biggest}");
    let tail = &r[r.len() - 2 * CALLBACK..];
    assert!(tail.iter().all(|v| (v - cut).abs() < 1e-6), "-20 dB from then on");
    // Control: the device output really is -20 dB (both sides).
    let n = dimmed.len();
    assert!((dimmed[n - 2] - 0.5 * cut).abs() < 1e-6 && (dimmed[n - 1] - 0.25 * cut).abs() < 1e-6, "{:?}", &dimmed[n - 2..]);
}

#[test]
fn mono_sums_the_sides_at_equal_power_and_ref_recalls_the_stored_level() {
    let (control, mut audio, shared) = playing(constant(0.4, 0.1, 3));
    let last = |out: &[f32]| (out[out.len() - 2], out[out.len() - 1]);
    let stereo = last(&callbacks(&mut audio, &shared, 5, |_, _| ()));
    assert!((stereo.0 - 0.4).abs() < 1e-6 && (stereo.1 - 0.1).abs() < 1e-6, "Main plays the sides as they are: {stereo:?}");
    let mono = last(&callbacks(&mut audio, &shared, 5, |_, s| s.monitor.set_mono(true)));
    // The matrix's mono pan law: -3 dB at center.
    let h = transit_dsp::pan::gains(0.0, transit_dsp::pan::PanLaw::Minus3).0;
    assert!((h - transit_dsp::db_to_gain(-3.0)).abs() < 1e-6, "{h}");
    assert!((mono.0 - h * 0.5).abs() < 1e-6 && (mono.1 - h * 0.5).abs() < 1e-6, "each side at -3 dB into both: {mono:?}");
    // The monitor level, the stored reference and Ref.
    let at = |db: f32| transit_dsp::db_to_gain(db) * h * 0.5;
    let level = last(&callbacks(&mut audio, &shared, 5, |_, s| {
        s.monitor.set_level_db(-6.0);
        s.monitor.set_reference_db(-12.0);
    }));
    assert!((level.0 - at(-6.0)).abs() < 1e-6, "the monitor level: {level:?}");
    let reference = last(&callbacks(&mut audio, &shared, 5, |_, s| s.monitor.set_reference(true)));
    assert!((reference.0 - at(-12.0)).abs() < 1e-6, "Ref plays the stored level: {reference:?}");
    let both = last(&callbacks(&mut audio, &shared, 5, |_, s| s.monitor.set_dim(true)));
    assert!((both.0 - at(-32.0)).abs() < 1e-6, "Dim acts on the reference level too: {both:?}");
    let back = last(&callbacks(&mut audio, &shared, 5, |_, s| {
        s.monitor.set_dim(false);
        s.monitor.set_reference(false);
    }));
    assert!((back.0 - at(-6.0)).abs() < 1e-6, "Ref off returns to the monitor level: {back:?}");
    let silent = last(&callbacks(&mut audio, &shared, 5, |_, s| s.monitor.set_level_db(f32::NEG_INFINITY)));
    assert_eq!(silent, (0.0, 0.0), "-inf is silence");
    control.retire(audio.plans, audio.actions);
}

/// A real-time render's capture of 9,600 frames from 4,800, with the monitor set as `set` leaves it,
/// and the device output it played.
fn render(set: impl Fn(&Shared)) -> (Vec<f32>, Vec<f32>) {
    let (control, mut audio, shared) = playing_with(sine_session(-12.0, 2), set);
    let tap = Arc::new(RenderTap::new(SR as u32));
    assert!(tap.arm(4_800, 9_600, 0, 2, &[]));
    audio.set_render_tap(Arc::clone(&tap));
    let mut device = Vec::new();
    for _ in 0..60 {
        device.extend(callbacks(&mut audio, &shared, 1, |_, _| ()));
        if tap.state() == TapState::Captured {
            break;
        }
    }
    assert_eq!(tap.state(), TapState::Captured);
    let mut captured = Vec::new();
    tap.drain(&mut captured, usize::MAX);
    control.retire(audio.plans, audio.actions);
    (captured, device)
}

#[test]
fn a_render_with_mono_dim_and_ref_on_is_bit_identical_to_one_with_them_off() {
    let (plain, plain_device) = render(|_| ());
    let (monitored, monitored_device) = render(|s| {
        s.monitor.set_mono(true);
        s.monitor.set_dim(true);
        s.monitor.set_level_db(-3.0);
        s.monitor.set_reference_db(-9.0);
        s.monitor.set_reference(true);
    });
    assert_eq!(plain.len(), 9_600 * 2);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&monitored), bits(&plain), "the render never sees the monitor path");
    // Control: what the device played did change.
    let loud = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    // Ref at -9 dB, Dim -20 dB, and the identical sides summed at -3 dB each (+3 dB in all).
    let h = transit_dsp::pan::gains(0.0, transit_dsp::pan::PanLaw::Minus3).0;
    let want = transit_dsp::db_to_gain(-29.0) * 2.0 * h;
    let got = loud(&monitored_device) / loud(&plain_device);
    assert!((got / want - 1.0).abs() < 0.01, "the device played {got} of the level, expected {want}");
}

#[test]
fn the_live_loudness_reads_the_mix_as_the_offline_integrator_does_and_resets() {
    // Dim on the whole time: the figure is the mix's, not the monitor path's.
    let (mut control, mut audio, shared) = playing_with(sine_session(-20.0, 6), |s| s.monitor.set_dim(true));
    assert_eq!(shared.monitor.integrated_lufs(), None, "nothing measured yet");
    let out = callbacks(&mut audio, &shared, 500, |_, _| ());
    let live = shared.monitor.integrated_lufs().expect("a figure after five seconds");
    // The reference: the offline meter over the same mix (the device output undone by Dim's gain).
    let cut = transit_dsp::db_to_gain(DIM_DB);
    let (l, r): (Vec<f32>, Vec<f32>) = out.as_chunks::<2>().0.iter().map(|f| (f[0] / cut, f[1] / cut)).unzip();
    let mut offline = transit_dsp::meter::LoudnessMeter::new(SR as f32, 2);
    offline.process(&[l.clone(), r], l.len());
    let offline = offline.integrated_lufs();
    eprintln!("live {live:.4} LUFS, offline {offline:.4} LUFS");
    assert!((live - offline).abs() <= 0.1, "live {live} offline {offline}");
    assert!((live + 20.0).abs() < 0.1, "a -20 dBFS sine reads -20 LUFS: {live}");
    // A click on the readout resets it: no figure until the next 400 ms block.
    let _ = callbacks(&mut audio, &shared, 1, |_, s| s.monitor.reset_loudness());
    assert_eq!(shared.monitor.integrated_lufs(), None, "reset");
    let _ = callbacks(&mut audio, &shared, 50, |_, _| ());
    assert!(shared.monitor.integrated_lufs().is_some(), "measuring again");
    // A start from a stop resets it too.
    control.stop().unwrap();
    let _ = callbacks(&mut audio, &shared, 5, |_, _| ());
    assert!(shared.monitor.integrated_lufs().is_some(), "a stop keeps the figure");
    control.send_action(Command::Play { from: 0, end: None, looped: None }, true).unwrap();
    let _ = callbacks(&mut audio, &shared, 2, |_, _| ());
    assert_eq!(shared.monitor.integrated_lufs(), None, "a start from a stop resets it");
    control.retire(audio.plans, audio.actions);
}
