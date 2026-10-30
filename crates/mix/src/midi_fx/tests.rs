#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use transit_model::{ChannelFormat, TRACK_COLORS, TrackId, TrackKind};

/// An instrument that writes down every event it is given, with its block number.
#[derive(Default)]
struct Rec {
    block: u32,
    heard: Vec<(u32, u32, PluginEvent)>,
    refuse: bool,
}

impl Plugin for Rec {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {
        self.block += 1;
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        if self.refuse {
            return false;
        }
        self.heard.push((self.block, offset, *ev));
        true
    }
}

const FRAMES: usize = 256;

fn track(chain: &[(&str, &[(&str, f32)])]) -> Track {
    let mut t = Track::new(TrackId(1), "Keys", TrackKind::Instrument, ChannelFormat::Stereo, TRACK_COLORS[0]);
    for (id, params) in chain {
        let mut fx = MidiEffectSlot::new(*id);
        for (k, v) in *params {
            fx.params.insert((*k).into(), *v);
        }
        t.midi_effects.push(fx);
    }
    t
}

fn ev(voice: i32, message: Message) -> PluginEvent {
    PluginEvent { channel: 0, note_id: voice, message }
}

fn on(voice: i32, key: u8, velocity: u8) -> PluginEvent {
    ev(voice, Message::NoteOn { key, velocity, id: NoteId::NONE })
}

fn off(voice: i32, key: u8) -> PluginEvent {
    ev(voice, Message::NoteOff { key, velocity: 0, id: NoteId::NONE })
}

/// One block: `events` (offset, event) taken in the given order, then flushed and processed.
fn block(host: &mut MidiFxHost, rec: &mut Rec, t: &Track, events: &[(u32, PluginEvent)]) {
    host.begin(rec, t, 0, &Transport::default(), FRAMES);
    for (o, e) in events {
        if host.active() {
            assert!(host.take(rec, *o, e));
        } else {
            rec.event(*o, e);
        }
    }
    host.flush(rec);
    rec.process(&mut [], FRAMES);
}

fn notes(rec: &Rec) -> Vec<(u32, u32, &'static str, u8, u8)> {
    rec.heard
        .iter()
        .map(|(b, o, e)| match e.message {
            Message::NoteOn { key, velocity, .. } => (*b, *o, "on", key, velocity),
            Message::NoteOff { key, .. } => (*b, *o, "off", key, 0),
            _ => (*b, *o, "other", 0, 0),
        })
        .collect()
}

#[test]
fn a_chain_runs_in_order_and_keeps_offsets_and_same_offset_order() {
    // +48 then -48 drops key 100 (148 is out of range); the other order keeps it.
    let up_down = track(&[("midi_transpose", &[("semitones", 48.0)]), ("midi_transpose", &[("semitones", -48.0)])]);
    let down_up = track(&[("midi_transpose", &[("semitones", -48.0)]), ("midi_transpose", &[("semitones", 48.0)])]);
    let input = [(9, on(1, 100, 90)), (2, on(2, 60, 80)), (2, on(3, 61, 70)), (200, off(2, 60))];
    let (mut a, mut b) = (MidiFxHost::new(), MidiFxHost::new());
    let (mut ra, mut rb) = (Rec::default(), Rec::default());
    block(&mut a, &mut ra, &up_down, &input);
    block(&mut b, &mut rb, &down_up, &input);
    assert_eq!(notes(&ra), vec![(0, 2, "on", 60, 80), (0, 2, "on", 61, 70), (0, 200, "off", 60, 0)]);
    assert_eq!(notes(&rb), vec![(0, 2, "on", 60, 80), (0, 2, "on", 61, 70), (0, 9, "on", 100, 90), (0, 200, "off", 60, 0)]);
}

#[test]
fn transpose_then_velocity_rewrites_at_the_same_offsets() {
    let t = track(&[("midi_transpose", &[("semitones", 12.0)]), ("midi_velocity", &[("scale", 50.0), ("offset", 1.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    block(&mut host, &mut rec, &t, &[(17, on(1, 60, 100)), (130, off(1, 60))]);
    assert_eq!(notes(&rec), vec![(0, 17, "on", 72, 51), (0, 130, "off", 72, 0)]);
}

#[test]
fn a_bypassed_chain_is_transparent() {
    let mut t = track(&[("midi_transpose", &[("semitones", 7.0)]), ("midi_velocity", &[("scale", 10.0)])]);
    for fx in &mut t.midi_effects {
        fx.bypass = true;
    }
    let bend = ev(-1, Message::PitchBend { value: 1234 });
    let input = [(0, on(1, 60, 100)), (5, bend), (5, on(2, 64, 3)), (77, off(1, 60)), (255, off(2, 64))];
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    block(&mut host, &mut rec, &t, &input);
    let direct: Vec<_> = input.iter().map(|(o, e)| (0, *o, *e)).collect();
    assert_eq!(rec.heard, direct);
    assert_eq!(host.counts, MidiFxCounts::default());
}

#[test]
fn events_for_a_later_block_are_carried_through_the_rest_of_the_chain() {
    let t = track(&[("midi_test_delay", &[("samples", 300.0)]), ("midi_transpose", &[("semitones", 1.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    block(&mut host, &mut rec, &t, &[(10, on(1, 60, 100)), (250, off(1, 60))]);
    assert!(rec.heard.is_empty(), "both are due in a later block");
    assert!(host.pending());
    // Next block: the carried on at 310 - 256 = 54 comes first, ahead of this block's own event at
    // the same offset (which is itself delayed out of the block).
    block(&mut host, &mut rec, &t, &[(54, on(2, 70, 100))]);
    assert_eq!(notes(&rec), vec![(1, 54, "on", 61, 100)]);
    block(&mut host, &mut rec, &t, &[]);
    assert_eq!(notes(&rec)[1..], [(2, 38, "off", 61, 0), (2, 98, "on", 71, 100)]);
    assert!(!host.pending());
}

#[test]
fn a_full_carry_delivers_releases_at_the_last_offset_and_counts() {
    let t = track(&[("midi_test_delay", &[("samples", 1000.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    let mut input: Vec<(u32, PluginEvent)> =
        (0..MIDI_FX_CARRY as i32).map(|i| (0, ev(-1, Message::Control { cc: 1, value: (i % 100) as u8 }))).collect();
    input.push((1, on(1, 60, 100)));
    input.push((2, off(1, 60)));
    block(&mut host, &mut rec, &t, &input);
    assert_eq!(notes(&rec), vec![(0, FRAMES as u32 - 1, "off", 60, 0)], "the release is not lost");
    assert_eq!(host.counts.carry_overflow, 2);
}

#[test]
fn a_full_block_refuses_and_counts() {
    let t = track(&[("midi_velocity", &[])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    host.begin(&mut rec, &t, 0, &Transport::default(), FRAMES);
    for _ in 0..MIDI_FX_BLOCK {
        assert!(host.take(&mut rec, 0, &ev(-1, Message::Control { cc: 1, value: 1 })));
    }
    assert!(!host.take(&mut rec, 0, &off(1, 60)), "refused: the mix retries a release next block");
    host.flush(&mut rec);
    assert_eq!(rec.heard.len(), MIDI_FX_BLOCK);
    assert_eq!(host.counts.overflow, 1);
}

#[test]
fn an_edit_ends_the_chains_notes_and_swallows_their_late_releases() {
    let with = track(&[("midi_transpose", &[("semitones", 12.0)])]);
    let mut without = with.clone();
    without.midi_effects.clear();
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    block(&mut host, &mut rec, &with, &[(5, on(1, 60, 100))]);
    // Removing the effect: the note it started (72) is ended at the next block's start, and the
    // mix's own release of 60 (voice 1) finds nothing to do.
    block(&mut host, &mut rec, &without, &[(9, off(1, 60)), (10, on(2, 60, 100))]);
    assert_eq!(notes(&rec), vec![(0, 5, "on", 72, 100), (1, 0, "off", 72, 0), (1, 10, "on", 60, 100)]);
    assert_eq!(host.counts.released_on_edit, 1);
    block(&mut host, &mut rec, &without, &[(1, off(2, 60))]);
    // Bypassing is an edit too, and so is a Stop.
    let mut bypassed = with.clone();
    bypassed.midi_effects[0].bypass = true;
    block(&mut host, &mut rec, &with, &[(1, on(3, 50, 100))]);
    block(&mut host, &mut rec, &bypassed, &[(3, off(3, 50))]);
    assert_eq!(notes(&rec)[3..], [(2, 1, "off", 60, 0), (3, 1, "on", 62, 100), (4, 0, "off", 62, 0)]);
    block(&mut host, &mut rec, &bypassed, &[(4, on(4, 40, 100))]);
    host.panic(&mut rec);
    assert!(host.take(&mut rec, 0, &off(4, 40)), "the Stop already ended it");
    assert_eq!(notes(&rec)[6..], [(5, 4, "on", 40, 100), (6, 0, "off", 40, 0)]);
    assert_eq!(host.counts.released_on_edit, 3);
}

#[test]
fn a_parameter_change_is_not_an_edit() {
    let t12 = track(&[("midi_transpose", &[("semitones", 12.0)])]);
    let mut t5 = t12.clone();
    t5.midi_effects[0].params.insert("semitones".into(), 5.0);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    block(&mut host, &mut rec, &t12, &[(5, on(1, 60, 100))]);
    block(&mut host, &mut rec, &t5, &[(9, off(1, 60)), (10, on(2, 60, 100))]);
    assert_eq!(notes(&rec), vec![(0, 5, "on", 72, 100), (1, 9, "off", 72, 0), (1, 10, "on", 65, 100)]);
    assert_eq!(host.counts.released_on_edit, 0);
}

#[test]
fn between_blocks_an_event_runs_through_at_once() {
    let t = track(&[("midi_transpose", &[("semitones", -2.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    block(&mut host, &mut rec, &t, &[(5, on(1, 60, 100))]);
    // A seek releases the clips' notes between blocks.
    assert!(host.take(&mut rec, 0, &off(1, 60)));
    assert_eq!(notes(&rec), vec![(0, 5, "on", 58, 100), (1, 0, "off", 58, 0)]);
}

#[test]
fn an_unknown_effect_passes_events_through() {
    let t = track(&[("clap:some.note.effect", &[]), ("midi_transpose", &[("semitones", 1.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    block(&mut host, &mut rec, &t, &[(5, on(1, 60, 100))]);
    assert_eq!(notes(&rec), vec![(0, 5, "on", 61, 100)]);
}

#[test]
fn automation_drives_an_effects_parameter() {
    let mut t = track(&[("midi_transpose", &[("semitones", 1.0)])]);
    let lane = AutoParam::Plugin { slot: AutoParam::midi_effect_slot(0).unwrap(), param: "semitones".into() };
    t.lane_mut(&lane).set_point(0, 7.0);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    block(&mut host, &mut rec, &t, &[(5, on(1, 60, 100))]);
    t.mixer.automation_mode = transit_model::AutomationMode::Off;
    block(&mut host, &mut rec, &t, &[(5, on(2, 60, 100))]);
    assert_eq!(notes(&rec), vec![(0, 5, "on", 67, 100), (1, 5, "on", 61, 100)]);
}
