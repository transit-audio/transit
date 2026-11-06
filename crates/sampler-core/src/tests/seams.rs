//! The adapter's two seams: [`SamplerCore::set_patch`] (a whole patch adopted at once, with the
//! rules of `set_zone` and `set_pad`) and [`SamplerCore::playing_sources`] (which sources the
//! sounding voices read).

use super::{bind, core, note, unity};
use crate::event::{Context, Event, EventKind};
use crate::patch::{Pad, PatchError, PlayMode, Zone};
use crate::playhead::Region;
use crate::reader::SliceSource;
use crate::{PADS, SamplerCore, ZONES};

const ONES: [f32; 256] = [1.0; 256];
const TWOS: [f32; 256] = [2.0; 256];

fn on(frame: u32, pad: usize) -> Event {
    Event::new(frame, EventKind::NoteOn { channel: 0, key: note(pad), velocity: 127, id: -1 })
}

/// The core's current patch, as `set_patch` takes it.
fn patch(c: &SamplerCore) -> (Vec<Option<Zone>>, [Pad; PADS]) {
    ((0..ZONES).map(|z| c.zone(z).copied()).collect(), core::array::from_fn(|p| *c.pad(p).unwrap()))
}

fn block(c: &mut SamplerCore, sources: &[SliceSource<'_>], events: &[Event], frames: usize) -> Vec<f32> {
    let mut l = vec![f32::NAN; frames];
    c.process(&Context::default(), sources, events, &mut [&mut l]);
    l
}

#[test]
fn set_patch_fades_the_changed_zone_and_lets_the_unchanged_one_play_on() {
    let sources = [SliceSource::mono(&ONES, 48_000.0), SliceSource::mono(&TWOS, 48_000.0)];
    let mut c = core();
    bind(&mut c, 0, 0, 256, unity(PlayMode::OneShot, 60, 0), 0);
    bind(&mut c, 1, 0, 256, unity(PlayMode::OneShot, 61, 0), 0);
    let first = block(&mut c, &sources, &[on(0, 0), on(0, 1)], 16);
    assert!(first.iter().all(|y| *y == 2.0));
    // Zone 1 moves to source 1: its voice fades over 32 frames still reading its own region;
    // zone 0 is unchanged and plays on at full level.
    let (mut zones, pads) = patch(&c);
    zones[1] = Some(Zone { region: Region::whole(1, 256), ..zones[1].unwrap() });
    c.set_patch(&zones, &pads).unwrap();
    let out = block(&mut c, &sources, &[], 48);
    for (n, y) in out.iter().enumerate() {
        let fade = if n < 32 { (31 - n) as f32 / 32.0 } else { 0.0 };
        assert_eq!(y.to_bits(), (1.0 + fade).to_bits(), "frame {n}");
    }
    assert_eq!(c.playing_sources().collect::<Vec<_>>(), [0]);
}

#[test]
fn set_patch_refuses_a_bad_patch_and_changes_nothing() {
    let mut c = core();
    bind(&mut c, 0, 0, 256, unity(PlayMode::OneShot, 60, 0), 0);
    let (zones, mut pads) = patch(&c);
    pads[5].note = pads[0].note;
    assert_eq!(c.set_patch(&zones, &pads), Err(PatchError::DuplicateNote));
    assert_eq!(c.pad(5).unwrap().note, note(5));
    let (mut zones, pads) = patch(&c);
    zones[0] = None;
    assert_eq!(c.set_patch(&zones, &pads), Err(PatchError::Zone), "a pad names a cleared zone");
    assert!(c.zone(0).is_some());
    assert_eq!(c.set_patch(&zones[..10], &pads), Err(PatchError::Zone), "the zone table is ZONES long");
}

#[test]
fn set_patch_fades_a_rebound_or_excluded_pad_voice() {
    let sources = [SliceSource::mono(&ONES, 48_000.0)];
    let mut c = core();
    bind(&mut c, 0, 0, 256, unity(PlayMode::OneShot, 60, 0), 0);
    bind(&mut c, 1, 0, 256, unity(PlayMode::OneShot, 61, 0), 0);
    block(&mut c, &sources, &[on(0, 0), on(0, 1)], 4);
    let (zones, mut pads) = patch(&c);
    pads[0].mute = true;
    c.set_patch(&zones, &pads).unwrap();
    let out = block(&mut c, &sources, &[], 40);
    for (n, y) in out.iter().enumerate() {
        let fade = if n < 32 { (31 - n) as f32 / 32.0 } else { 0.0 };
        assert_eq!(y.to_bits(), (1.0 + fade).to_bits(), "frame {n}");
    }
}

#[test]
fn playing_sources_follow_the_sounding_voices() {
    let sources = [SliceSource::mono(&ONES, 48_000.0), SliceSource::mono(&TWOS, 48_000.0)];
    let mut c = core();
    assert_eq!(c.playing_sources().count(), 0);
    bind(&mut c, 0, 1, 64, unity(PlayMode::OneShot, 60, 0), 0);
    block(&mut c, &sources, &[on(0, 0)], 8);
    assert_eq!(c.playing_sources().collect::<Vec<_>>(), [1]);
    block(&mut c, &sources, &[], 64);
    assert_eq!(c.playing_sources().count(), 0, "the one-shot ended with its region");
}
