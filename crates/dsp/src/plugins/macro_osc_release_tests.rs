//! #290: event order inside a voice render must not leave the shaper gated without a key.
//! All time is rendered sample counts; the assertions inspect each voice's envelopes.

use super::*;
use crate::plugins::instrument_queue::Stage;

const SR: f32 = 48_000.0;
const TICK: usize = 6000; // A sixteenth at 120 BPM.
// The default 300 ms release is to -60 dB; the envelope rests below -100 dB.
const RELEASE: usize = 24_024;

fn instrument(on: bool) -> MacroOscillator {
    let mut m = MacroOscillator::new();
    assert!(m.set_param("voices", 15.0));
    assert!(m.set_param("shaper", if on { 1.0 } else { 0.0 }));
    m.prepare(SR, 512, 2);
    m
}

fn released(stage: Stage) -> bool {
    matches!(stage, Stage::Release | Stage::Idle)
}

/// 384 sixteenths in G minor, varying velocity and length around 68% gate. The re-strikes
/// include off/on and on/off at one sample, adjacent samples in one voice block, and an on
/// while the key is held. This instrument is omni and reuses one voice per key: a later off
/// releases that voice even if another generator gate previously struck the same key.
#[test]
fn scripted_sixteenths_release_every_ungated_voice() {
    for mode in ["off", "on", "held-toggle"] {
        let mut m = instrument(mode == "on");
        let mut io = [vec![0.0; BLOCK], vec![0.0; BLOCK]];
        let mut deadlines = [None; MAX_VOICES];
        let mut shaped = [false; MAX_VOICES];
        let mut rng = 1u32;
        for step in 0..384 {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let key = [55, 57, 58, 60, 62, 63, 65, 67, 69, 70, 72, 74, 75, 77][step % 14];
            let velocity = 24 + (rng % 104) as u8;
            let length = (4080.0 * (0.16 + 0.84 * f64::from(rng >> 8) / 16_777_215.0)) as usize;
            // Keep the collision away from a render boundary so both orders are exercised
            // within one 12-frame render, regardless of the pseudo-random length.
            let end = length / BLOCK * BLOCK + 6;
            m.note_on(0, key, velocity);
            match step % 5 {
                0 => m.note_off(end, key),
                1 => {
                    m.note_off(end, key);
                    m.note_on(end, key, 127);
                    m.note_off(end + 600, key);
                }
                2 => {
                    m.note_off(end, key);
                    m.note_on(end + 1, key, 32);
                    m.note_off(end + 600, key);
                }
                3 => {
                    m.note_on(end - 1, key, 91);
                    m.note_off(end, key);
                    m.note_off(end + 600, key);
                }
                _ => {
                    m.note_on(end, key, 61);
                    m.note_off(end, key);
                    m.note_off(end + 600, key);
                }
            }
            for at in (0..TICK).step_by(BLOCK) {
                if mode == "held-toggle" && step == 2 && at == BLOCK {
                    assert!(m.set_param("shaper", 1.0));
                }
                m.process(&mut io, BLOCK);
                let now = step * TICK + at + BLOCK;
                for (i, s) in m.slots.iter().enumerate() {
                    if s.sounding() {
                        deadlines[i] = None;
                        shaped[i] = m.shaper.on;
                        continue;
                    }
                    assert!(
                        released(s.shaper.amp.stage) && released(s.shaper.filter_env.stage),
                        "mode={mode} step={step} sample={now} voice={i} key={} has no held key but amp={:?} filter={:?}",
                        s.note,
                        s.shaper.amp.stage,
                        s.shaper.filter_env.stage
                    );
                    let deadline = *deadlines[i].get_or_insert(now + RELEASE);
                    if now >= deadline {
                        assert_eq!(s.shaper.amp.stage, Stage::Idle, "mode={mode} voice={i}: amp release expired");
                        // A tail released before the switch keeps its old path (#262).
                        if shaped[i] {
                            assert!(s.idle, "mode={mode} voice={i}: shaped release expired");
                        }
                    }
                }
            }
        }
        // No further note can hide a stuck tail by stealing or restarting its voice.
        for _ in 0..m.tail_samples().div_ceil(BLOCK) {
            m.process(&mut io, BLOCK);
        }
        assert!(m.slots.iter().all(|s| s.idle && s.shaper.amp_idle()), "mode={mode}: all voices must rest");
        assert!(io.iter().flatten().all(|y| *y == 0.0));
    }
}

fn render(m: &mut MacroOscillator, frames: usize, host_block: usize) -> f32 {
    let mut io = [vec![0.0; host_block], vec![0.0; host_block]];
    let mut peak = 0.0f32;
    for at in (0..frames).step_by(host_block) {
        let n = host_block.min(frames - at);
        m.process(&mut io, n);
        peak = io[0][..n].iter().fold(peak, |p, y| p.max(y.abs()));
    }
    peak
}

/// A phrase can begin and end before its first render, or restart a sounding voice and end
/// before that restart renders. Sweep every sample within the internal block and host blocks
/// that do not divide it. Envelopes are sources even while the shaper is bypassed.
#[test]
fn a_phrase_ending_before_its_first_render_releases_both_envelopes() {
    for on in [false, true] {
        for held in [false, true] {
            for host in [1, 7, 12, 13, 256] {
                for off in 1..=BLOCK {
                    let mut m = instrument(on);
                    if held {
                        m.note_on(0, 60, 100);
                        render(&mut m, 4800, host);
                    }
                    m.note_on(1, 60, 80);
                    m.note_off(off, 60);
                    render(&mut m, 2 * BLOCK, host);
                    let s = &m.slots[0];
                    assert!(!s.sounding());
                    assert!(
                        released(s.shaper.amp.stage) && released(s.shaper.filter_env.stage),
                        "shaper={on} held={held} host={host} off={off}: amp={:?} filter={:?}",
                        s.shaper.amp.stage,
                        s.shaper.filter_env.stage
                    );
                    render(&mut m, RELEASE, host);
                    assert!(m.slots[0].shaper.amp_idle());
                    if on {
                        assert!(m.slots[0].idle);
                    }
                }
            }
        }
    }
}

/// Toggle at the off's sample or one sample on either side, both within and across a voice
/// boundary. A pending restart must not mask the off; a release keeps its selected audio path.
#[test]
fn shaper_toggles_around_a_release_do_not_leave_a_gate_high() {
    for initially_on in [false, true] {
        for off in [6usize, 12] {
            for toggle in [off - 1, off, off + 1] {
                for restart in [false, true] {
                    let mut m = instrument(initially_on);
                    m.note_on(0, 60, 100);
                    render(&mut m, 4800, 256);
                    // Single-frame host slices place the parameter change exactly. MIDI is
                    // still consumed only at the documented 12-frame voice boundaries.
                    for sample in 0..=2 * BLOCK {
                        if sample == toggle {
                            assert!(m.set_param("shaper", if initially_on { 0.0 } else { 1.0 }));
                        }
                        if restart && sample == off - 1 {
                            m.note_on(0, 60, 91);
                        }
                        if sample == off {
                            m.note_off(0, 60);
                        }
                        render(&mut m, 1, 1);
                    }
                    let s = &m.slots[0];
                    assert!(!s.sounding());
                    assert!(
                        released(s.shaper.amp.stage) && released(s.shaper.filter_env.stage),
                        "initially_on={initially_on} off={off} toggle={toggle} restart={restart}: amp={:?} filter={:?}",
                        s.shaper.amp.stage,
                        s.shaper.filter_env.stage
                    );
                    render(&mut m, RELEASE, 256);
                    assert!(m.slots[0].shaper.amp_idle());
                    let tail = m.tail_samples();
                    render(&mut m, tail, 256);
                    assert!(m.slots[0].idle);
                }
            }
        }
    }
}

/// Keep the reported clearing case in the repro: after the colliding on/off, switching off
/// alone preserves the released voice's path, but the next phrase takes the new setting.
/// Print the pre-clear state before the regression assertion so the negative control records
/// the stuck sustain, audible output, and successful clearing without changing the test.
#[test]
fn a_colliding_restrike_releases_without_needing_an_unshaped_restart() {
    let mut m = instrument(true);
    m.note_on(0, 55, 100);
    render(&mut m, 4800, 256);
    m.note_on(5, 55, 91);
    m.note_off(6, 55);
    render(&mut m, RELEASE + 2 * BLOCK, 256);
    let stage = m.slots[0].shaper.amp.stage;
    let idle = m.slots[0].idle;
    let before = render(&mut m, 4800, 256);
    assert!(m.set_param("shaper", 0.0));
    let switched = render(&mut m, 4800, 256);
    m.note_on(0, 55, 100);
    render(&mut m, 4800, 256);
    m.note_off(0, 55);
    let tail = m.tail_samples();
    render(&mut m, tail, 256);
    let cleared = m.slots[0].idle;
    println!("after collision: amp={stage:?} idle={idle} peak={before}; switch-off peak={switched}; unshaped restart cleared={cleared}");
    assert!(cleared, "the unshaped restart ends");
    assert!(idle && stage == Stage::Idle, "colliding re-strike needs no clearing restart: amp={stage:?} idle={idle} peak={before}");
}
