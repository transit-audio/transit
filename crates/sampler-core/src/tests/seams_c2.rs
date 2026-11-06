//! The C2 seams: Reset All Controllers as an event, a cleared zone's voices fading over
//! [`CHOKE_FRAMES`] instead of ending at once, and [`SamplerCore::set_zone_glide`]'s level and
//! pan ramp.

use super::{SR, core, unity};
use crate::event::{Context, Control, Event, EventKind};
use crate::patch::{Pad, PlayMode, Zone};
use crate::playhead::Region;
use crate::reader::SliceSource;
use crate::{CHOKE_FRAMES, PADS, SamplerCore, ZONES, math};

const ONES: [f32; 512] = [1.0; 512];

/// An unbound Gate zone 3 over the constant-one source, keys and velocities all, Gate 1's controls
/// with release `release` frames.
fn gate_zone(c: &mut SamplerCore, release: u32) -> Zone {
    let zone = Zone { playback: unity(PlayMode::Gate, 60, release), ..Zone::new(Region::whole(0, 512)) };
    c.set_zone(3, Some(zone)).unwrap();
    zone
}

fn run(c: &mut SamplerCore, frame: u64, events: &[Event], frames: usize) -> (Vec<f32>, Vec<f32>) {
    let sources = [SliceSource::mono(&ONES, SR)];
    let mut l = vec![f32::NAN; frames];
    let mut r = vec![f32::NAN; frames];
    c.process(&Context { frame, ..Context::default() }, &sources[..], events, &mut [&mut l, &mut r]);
    (l, r)
}

fn fingerprint(c: &SamplerCore) -> u64 {
    use core::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    c.fingerprint(&mut h);
    h.finish()
}

#[test]
fn reset_controllers_clears_every_controller_and_note_pressure() {
    let mut a = core();
    let b = core();
    let controls = [
        Control::ModWheel(0.5),
        Control::PitchBend(-0.5),
        Control::Aftertouch(0.25),
        Control::PolyAftertouch { key: 60, value: 1.0 },
        Control::PolyAftertouch { key: 127, value: 0.5 },
        Control::Sustain(true),
    ];
    let mut events: Vec<Event> = controls.iter().map(|c| Event::new(0, EventKind::Control(*c))).collect();
    events.push(Event::new(1, EventKind::ResetControllers));
    run(&mut a, 0, &events, 4);
    assert_eq!(fingerprint(&a), fingerprint(&b), "back to the rest state, note pressure included");
}

#[test]
fn reset_controllers_releases_sustained_voices_at_its_frame() {
    let mut c = core();
    gate_zone(&mut c, 8);
    let events = [
        Event::new(0, EventKind::Control(Control::Sustain(true))),
        Event::on(0, 60, 127),
        Event::off(2, 60),
        Event::new(8, EventKind::ResetControllers),
    ];
    let (out, _) = run(&mut c, 0, &events, 24);
    for (n, y) in out.iter().enumerate() {
        let want = match n {
            0..8 => 1.0,
            8..16 => 1.0 - (n - 7) as f32 / 8.0,
            _ => 0.0,
        };
        assert_eq!(y.to_bits(), want.to_bits(), "frame {n}");
    }
}

#[test]
fn a_cleared_zone_fades_its_voices_over_the_choke_frames() {
    for whole in [false, true] {
        let mut c = core();
        gate_zone(&mut c, 0);
        run(&mut c, 0, &[Event::on(0, 60, 127)], 8);
        if whole {
            let mut zones: Vec<Option<Zone>> = (0..ZONES).map(|z| c.zone(z).copied()).collect();
            zones[3] = None;
            let pads: [Pad; PADS] = core::array::from_fn(|p| *c.pad(p).unwrap());
            c.set_patch(&zones, &pads).unwrap();
        } else {
            c.set_zone(3, None).unwrap();
        }
        let (out, _) = run(&mut c, 8, &[], 48);
        for (n, y) in out.iter().enumerate() {
            let want = if n < CHOKE_FRAMES as usize { (31 - n) as f32 / 32.0 } else { 0.0 };
            assert_eq!(y.to_bits(), want.to_bits(), "set_patch {whole}: frame {n}");
        }
        assert_eq!(c.active_voices(), 0);
        let (after, _) = run(&mut c, 56, &[Event::on(0, 60, 127)], 4);
        assert!(after.iter().all(|y| *y == 0.0), "a cleared zone starts nothing");
    }
}

#[test]
fn a_glide_ramps_level_and_pan_linearly_over_its_frames() {
    let mut c = core();
    let zone = gate_zone(&mut c, 0);
    run(&mut c, 0, &[Event::on(0, 60, 127)], 16);
    let mut next = zone;
    next.playback.level_db = -6.0;
    next.playback.pan = 0.5;
    c.set_zone_glide(3, next, 16, 64).unwrap();
    let (l, r) = run(&mut c, 16, &[], 80);
    let to = math::powf(10.0, -6.0 / 20.0);
    for n in 0..80 {
        let k = if n < 64 { (n + 1) as f32 / 64.0 } else { 1.0 };
        let level = if n < 64 { 1.0 + (to - 1.0) * k } else { to };
        let pan = if n < 64 { 0.5 * k } else { 0.5 };
        // Balance: a pan to the right lowers the left side.
        assert_eq!(l[n].to_bits(), (level * (1.0 - pan)).to_bits(), "left, frame {n}");
        assert_eq!(r[n].to_bits(), level.to_bits(), "right, frame {n}");
    }
    // No step on the right side is larger than the glide's per-frame increment.
    let step = (1.0 - to) / 64.0;
    let mut prev = 1.0f32;
    for (n, y) in r.iter().enumerate() {
        assert!((prev - y) <= step + f32::EPSILON, "frame {n}: step {}", prev - y);
        prev = *y;
    }
}

#[test]
fn a_glide_from_an_unchanged_level_is_no_glide() {
    let mut c = core();
    let zone = gate_zone(&mut c, 0);
    run(&mut c, 0, &[Event::on(0, 60, 127)], 4);
    let mut next = zone;
    next.playback.transpose = 0.0;
    c.set_zone_glide(3, next, 4, 64).unwrap();
    let (l, _) = run(&mut c, 4, &[], 8);
    assert!(l.iter().all(|y| *y == 1.0));
}

#[test]
fn a_reset_completes_a_glide_in_progress_to_its_target() {
    let mut c = core();
    let zone = gate_zone(&mut c, 0);
    run(&mut c, 0, &[Event::on(0, 60, 127)], 1000);
    let mut next = zone;
    next.playback.level_db = -12.0;
    next.playback.pan = -0.5;
    // A glide over 512 frames from frame 1000, interrupted by a reset after 100 of them.
    c.set_zone_glide(3, next, 1000, 512).unwrap();
    run(&mut c, 1000, &[], 100);
    c.reset();
    // The caller's frame starts again at 0, far before the glide's own frames.
    let (l, r) = run(&mut c, 0, &[Event::on(0, 60, 127)], 64);
    let to = math::powf(10.0, -12.0 / 20.0);
    for (n, (a, b)) in l.iter().zip(&r).enumerate() {
        // Pan to the left lowers the right side: (1, 1 + pan).
        assert_eq!(a.to_bits(), to.to_bits(), "left, frame {n}: the target level, not a stale or interpolated one");
        assert_eq!(b.to_bits(), (to * (1.0 - 0.5)).to_bits(), "right, frame {n}: the target pan");
    }
    // A later glide starts from the target, not from the old value.
    let mut again = next;
    again.playback.level_db = 0.0;
    again.playback.pan = 0.0;
    c.set_zone_glide(3, again, 64, 4).unwrap();
    let (l, _) = run(&mut c, 64, &[], 4);
    assert_eq!(l[0].to_bits(), (to + (1.0 - to) * 0.25).to_bits(), "the glide starts at the value reset made current");
}
