//! [`SamplerCore::voice_positions`] (the Slices view's playheads, #470): a stepped render moves the
//! reported phase by exactly the frames the voice consumed, at the source's own rate (unity, a
//! source rate unlike the session's, a transposed voice, a start offset, reverse), through a loop
//! wrap and a ping-pong turn; a voice that ends leaves the list; two voices report two places; and
//! reading the list never changes the audio.

use super::{SR, bind, core, note, render, unity};
use crate::event::{Event, EventKind};
use crate::patch::PlayMode;
use crate::playhead::{LoopMode, Region};
use crate::reader::SliceSource;
use crate::{SamplerCore, VoicePosition};

const ONES: [f32; 1024] = [0.25; 1024];

fn on(frame: u32, pad: usize) -> Event {
    Event::new(frame, EventKind::NoteOn { channel: 0, key: note(pad), velocity: 127, id: -1 })
}

fn positions(c: &SamplerCore) -> Vec<(u16, f64)> {
    c.voice_positions().map(|VoicePosition { zone, phase }| (zone, phase)).collect()
}

/// Pad 0 on zone 0 over the whole 1024 frames of source 0, a one-shot at unity.
fn one_shot() -> SamplerCore {
    let mut c = core();
    bind(&mut c, 0, 0, 1024, unity(PlayMode::OneShot, 60, 0), 0);
    c
}

#[test]
fn the_phase_moves_by_the_frames_rendered_at_unity_and_from_a_start_offset() {
    let src = [SliceSource::mono(&ONES, SR)];
    let mut c = one_shot();
    assert!(positions(&c).is_empty(), "nothing sounds before the note");
    render(&mut c, &src, &[on(0, 0)], 64);
    assert_eq!(positions(&c), [(0, 64.0)]);
    render(&mut c, &src, &[], 100);
    assert_eq!(positions(&c), [(0, 164.0)]);
    // A note ten frames into a block reads 54 frames of it.
    let mut c = one_shot();
    render(&mut c, &src, &[on(10, 0)], 64);
    assert_eq!(positions(&c), [(0, 54.0)]);
}

#[test]
fn the_phase_counts_source_frames_at_the_source_rate_and_under_transposition() {
    // A source at half the session rate: one source frame per two output frames.
    let half = [SliceSource::mono(&ONES, SR / 2.0)];
    let mut c = one_shot();
    render(&mut c, &half, &[on(0, 0)], 64);
    assert_eq!(positions(&c), [(0, 32.0)]);
    // An octave up: two source frames per output frame.
    let src = [SliceSource::mono(&ONES, SR)];
    let mut c = core();
    bind(&mut c, 0, 0, 1024, crate::patch::Playback { transpose: 12.0, ..unity(PlayMode::OneShot, 60, 0) }, 0);
    render(&mut c, &src, &[on(0, 0)], 64);
    assert_eq!(positions(&c), [(0, 128.0)]);
    // A fifth up at a 44.1 kHz source: the phase is the voice's step added once per frame (the
    // same arithmetic in-process, no pinned value).
    let cd = [SliceSource::mono(&ONES, 44_100.0)];
    let mut c = core();
    bind(&mut c, 0, 0, 1024, crate::patch::Playback { transpose: 7.0, ..unity(PlayMode::OneShot, 60, 0) }, 0);
    render(&mut c, &cd, &[on(0, 0)], 100);
    let step = (44_100.0 / f64::from(SR)) * 2f64.powf(7.0 / 12.0);
    let expected = (0..100).fold(0.0f64, |p, _| p + step);
    assert_eq!(positions(&c), [(0, expected)]);
}

#[test]
fn reverse_reads_down_from_the_end() {
    let src = [SliceSource::mono(&ONES, SR)];
    let mut c = core();
    bind(&mut c, 0, 0, 256, crate::patch::Playback { reverse: true, ..unity(PlayMode::OneShot, 60, 0) }, 0);
    render(&mut c, &src, &[on(0, 0)], 64);
    assert_eq!(positions(&c), [(0, 191.0)]);
}

/// Pad 0 held on a Gate zone over 256 frames looping `64..128` in `mode`.
fn looping(mode: LoopMode) -> SamplerCore {
    let mut c = core();
    let zone = crate::patch::Zone { playback: unity(PlayMode::Gate, 60, 0), ..crate::patch::Zone::new(Region::whole(0, 256).looped(mode, 64, 128)) };
    c.set_zone(0, Some(zone)).unwrap();
    c.set_pad(0, crate::patch::Pad { zone: Some(0), ..crate::patch::Pad::new(0) }).unwrap();
    c
}

#[test]
fn the_phase_wraps_with_a_forward_loop() {
    let src = [SliceSource::mono(&ONES, SR)];
    let mut c = looping(LoopMode::Forward);
    render(&mut c, &src, &[on(0, 0)], 120);
    assert_eq!(positions(&c), [(0, 120.0)]);
    // Eight frames reach 128, which wraps to 64; 72 more frames: 64 + (8 + 72 - 8) mod 64.
    render(&mut c, &src, &[], 80);
    assert_eq!(positions(&c), [(0, 72.0)], "200 frames: 64 + (200 - 64) mod 64");
}

#[test]
fn the_phase_turns_at_a_ping_pong_end() {
    let src = [SliceSource::mono(&ONES, SR)];
    let mut c = looping(LoopMode::PingPong);
    render(&mut c, &src, &[on(0, 0)], 127);
    assert_eq!(positions(&c), [(0, 127.0)], "the loop's last frame");
    render(&mut c, &src, &[], 23);
    assert_eq!(positions(&c), [(0, 104.0)], "23 frames back down from 127");
    render(&mut c, &src, &[], 41);
    assert_eq!(positions(&c), [(0, 65.0)], "down to 64 (40 frames), then up one");
}

#[test]
fn a_voice_that_ends_leaves_and_two_voices_report_two_places() {
    let src = [SliceSource::mono(&ONES, SR)];
    let mut c = core();
    bind(&mut c, 0, 0, 100, unity(PlayMode::OneShot, 60, 0), 0);
    bind(&mut c, 1, 0, 1024, unity(PlayMode::OneShot, 61, 0), 0);
    render(&mut c, &src, &[on(0, 0), on(30, 1)], 64);
    assert_eq!(positions(&c), [(0, 64.0), (1, 34.0)]);
    // Pad 0's region ends at frame 100: its voice is gone; pad 1's plays on.
    render(&mut c, &src, &[], 64);
    assert_eq!(positions(&c), [(1, 98.0)]);
    render(&mut c, &src, &[], 1024);
    assert!(positions(&c).is_empty(), "both ended with their regions");
}

#[test]
fn reading_the_positions_never_changes_the_audio() {
    let src = [SliceSource::mono(&ONES, SR)];
    let (mut read, mut unread) = (looping(LoopMode::PingPong), looping(LoopMode::PingPong));
    let mut seen = 0usize;
    for b in 0..8u32 {
        let events = if b == 0 { vec![on(5, 0)] } else { Vec::new() };
        let x = render(&mut read, &src, &events, 61);
        seen += read.voice_positions().count();
        let y = render(&mut unread, &src, &events, 61);
        assert_eq!(x.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), y.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), "block {b}");
    }
    assert_eq!(seen, 8, "the read side did read a voice every block");
}
