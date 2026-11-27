//! Chain edits (ADR 0057, decision 2.13, slice P3a), decision 13's P3a evidence, through the mix's
//! event state and the MIDI effect host with the test-only fixtures (the owning pass-through, the
//! gated generator) and Transpose.
//!
//! Each case runs at the first insertion (no chain before: the mix gave its notes to the instrument
//! directly) and at a later edit (the owning pass-through already in the chain), and at three
//! instruments that accept every event unless a case says otherwise: a plain one, which applies
//! every event in order, and two models of the VST3 adapter's one point per controller and offset
//! (`crates/vst3-host/src/ffi/events.rs`, `fill`): at each offset the last value of each
//! controller, applied before, or after, the note events of that offset.
//!
//! Expected results are written by hand from 2.13's rules. The block runner calls what the strip
//! calls, in its order (`strip.rs`): the host's begin with the edit steps, the mix's retries, clip
//! events, the queue's delivery, the host's flush, then the instrument's `process`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use super::super::{Admit, EventStatus, InstrumentEvents, Message, NoteId, PluginEvent, Source};
use transit_dsp::{Plugin, Transport};
use transit_model::{ChannelFormat, MidiEffectSlot, TRACK_COLORS, Track, TrackId, TrackKind};

mod edits;
mod limits;
mod pedals;

/// How the instrument applies the events of one offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// In the order given.
    Plain,
    /// The VST3 adapter's one point per controller and offset, applied before the note events.
    Vst3Before,
    /// The same, applied after the note events.
    Vst3After,
}

pub(super) const MODES: [Mode; 3] = [Mode::Plain, Mode::Vst3Before, Mode::Vst3After];

/// Where the edit falls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum At {
    /// The track's first effect: no chain before.
    First,
    /// A later edit: the owning pass-through is already in the chain.
    Later,
}

pub(super) const ATS: [At; 2] = [At::First, At::Later];

pub(super) const OWN: &str = "midi_test_owning";
pub(super) const GATED: &str = "midi_test_gated";
pub(super) const TRANSPOSE: &str = "midi_transpose";

/// One voice of the model instrument, from its note-on to the frame it stops sounding.
#[derive(Clone, Copy, Debug)]
pub(super) struct Voice {
    pub(super) channel: u8,
    pub(super) key: u8,
    pub(super) id: i32,
    /// Its key is down (no note-off yet).
    down: bool,
    /// Captured by the sostenuto pedal's press.
    captured: bool,
    pub(super) on: u64,
    pub(super) end: Option<u64>,
}

/// An instrument that keeps voices the way a MIDI synthesizer does: a voice sounds while its key
/// is down, or while the sustain pedal (CC 64) or hold 2 (CC 69) is at 64 or above, or while the
/// sostenuto pedal (CC 66) that captured it (its key was down at the press) stays down. CC 120 ends
/// a channel's voices, CC 123 releases its keys, CC 121 lifts its pedals. Its `reset` is a built-in
/// instrument's: every voice ends. It writes down every event it accepts at its absolute frame.
pub(super) struct Model {
    mode: Mode,
    pending: Vec<(u32, PluginEvent)>,
    /// Every event accepted: (absolute frame, event).
    pub(super) given: Vec<(u64, PluginEvent)>,
    /// `process` calls so far.
    pub(super) block: u64,
    frame: u64,
    pub(super) voices: Vec<Voice>,
    /// Controller values per channel.
    pub(super) ctl: [[u8; 128]; 16],
    pub(super) bend: [u16; 16],
    /// Blocks (by `block`) in which every note-off is refused.
    pub(super) refuse_offs: std::ops::Range<u64>,
    /// Blocks in which every release (note-off, pedal below 64, CC 120, 121, 123) is refused.
    pub(super) refuse_releases: std::ops::Range<u64>,
}

impl Model {
    fn new(mode: Mode) -> Model {
        Model {
            mode,
            pending: Vec::new(),
            given: Vec::new(),
            block: 0,
            frame: 0,
            voices: Vec::new(),
            ctl: [[0; 128]; 16],
            bend: [8192; 16],
            refuse_offs: 0..0,
            refuse_releases: 0..0,
        }
    }

    fn settle(&mut self, ch: u8, f: u64) {
        let c = &self.ctl[usize::from(ch)];
        let (sustain, sostenuto) = (c[64] >= 64 || c[69] >= 64, c[66] >= 64);
        for v in self.voices.iter_mut().filter(|v| v.channel == ch && v.end.is_none()) {
            if !sostenuto {
                v.captured = false;
            }
            if !v.down && !v.captured && !sustain {
                v.end = Some(f);
            }
        }
    }

    fn apply(&mut self, f: u64, ev: &PluginEvent) {
        let ch = ev.channel & 0x0F;
        match ev.message {
            Message::NoteOn { key, velocity, .. } if velocity > 0 => {
                self.voices.push(Voice { channel: ch, key, id: ev.note_id, down: true, captured: false, on: f, end: None });
            }
            Message::NoteOn { key, .. } | Message::NoteOff { key, .. } => {
                let v = self.voices.iter_mut().find(|v| {
                    v.end.is_none() && v.down && v.channel == ch && if ev.note_id == PluginEvent::NO_ID { v.key == key } else { v.id == ev.note_id }
                });
                if let Some(v) = v {
                    v.down = false;
                }
                self.settle(ch, f);
            }
            Message::Control { cc, value } => {
                let old = self.ctl[usize::from(ch)][usize::from(cc & 0x7F)];
                self.ctl[usize::from(ch)][usize::from(cc & 0x7F)] = value;
                match cc {
                    66 if old < 64 && value >= 64 => {
                        for v in self.voices.iter_mut().filter(|v| v.channel == ch && v.end.is_none() && v.down) {
                            v.captured = true;
                        }
                    }
                    120 => {
                        for v in self.voices.iter_mut().filter(|v| v.channel == ch && v.end.is_none()) {
                            v.end = Some(f);
                        }
                    }
                    121 => {
                        for p in [64, 66, 67, 69] {
                            self.ctl[usize::from(ch)][p] = 0;
                        }
                    }
                    123 => {
                        for v in self.voices.iter_mut().filter(|v| v.channel == ch) {
                            v.down = false;
                        }
                    }
                    _ => {}
                }
                self.settle(ch, f);
            }
            Message::PitchBend { value } => self.bend[usize::from(ch)] = value,
            _ => {}
        }
    }

    /// The keys of the voices that sound now.
    pub(super) fn sounding(&self) -> Vec<u8> {
        self.voices.iter().filter(|v| v.end.is_none()).map(|v| v.key).collect()
    }

    /// The voices of `key`: (on frame, end frame).
    pub(super) fn spans(&self, key: u8) -> Vec<(u64, Option<u64>)> {
        self.voices.iter().filter(|v| v.key == key).map(|v| (v.on, v.end)).collect()
    }
}

fn is_controller(m: &Message) -> bool {
    matches!(
        m,
        Message::Control { .. } | Message::Control14 { .. } | Message::PitchBend { .. } | Message::ChannelPressure { .. } | Message::Program { .. }
    )
}

impl Plugin for Model {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    /// A built-in instrument's reset: every voice ends. Releases already accepted are kept for the
    /// next block, as the plug-in hosts keep them.
    fn reset(&mut self) {
        let f = self.frame;
        for v in self.voices.iter_mut().filter(|v| v.end.is_none()) {
            v.end = Some(f);
        }
        for c in &mut self.ctl {
            for p in [64, 66, 67, 69] {
                c[p] = 0;
            }
        }
        self.pending.retain(|(_, e)| e.message.survives_reset());
    }
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn all_notes_off(&mut self) {
        let f = self.frame;
        for v in &mut self.voices {
            v.down = false;
        }
        for ch in 0..16 {
            self.settle(ch, f);
        }
    }
    fn process(&mut self, _: &mut [Vec<f32>], frames: usize) {
        let mut pending = std::mem::take(&mut self.pending);
        pending.sort_by_key(|(o, _)| *o);
        let mut i = 0;
        while i < pending.len() {
            let o = pending[i].0;
            let j = pending[i..].iter().position(|(x, _)| *x != o).map_or(pending.len(), |k| i + k);
            let group = &pending[i..j];
            let f = self.frame + u64::from(o);
            if self.mode == Mode::Plain {
                for (_, e) in group {
                    self.apply(f, e);
                }
            } else {
                // One point per controller and offset: the last value of each.
                let mut points: Vec<PluginEvent> = Vec::new();
                for (_, e) in group.iter().filter(|(_, e)| is_controller(&e.message)) {
                    let same = |p: &PluginEvent| {
                        p.channel == e.channel
                            && match (p.message, e.message) {
                                (Message::Control { cc: a, .. }, Message::Control { cc: b, .. }) => a == b,
                                (a, b) => std::mem::discriminant(&a) == std::mem::discriminant(&b),
                            }
                    };
                    match points.iter_mut().find(|p| same(p)) {
                        Some(p) => *p = *e,
                        None => points.push(*e),
                    }
                }
                let notes: Vec<PluginEvent> = group.iter().filter(|(_, e)| !is_controller(&e.message)).map(|(_, e)| *e).collect();
                let (first, then) = if self.mode == Mode::Vst3Before { (&points, &notes) } else { (&notes, &points) };
                for e in first.iter().chain(then.iter()) {
                    self.apply(f, e);
                }
            }
            i = j;
        }
        self.block += 1;
        self.frame += frames as u64;
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        let off = matches!(ev.message, Message::NoteOff { .. } | Message::NoteOn { velocity: 0, .. });
        if (off && self.refuse_offs.contains(&self.block)) || (ev.is_release() && self.refuse_releases.contains(&self.block)) {
            return false;
        }
        self.pending.push((offset, *ev));
        self.given.push((self.frame + u64::from(offset), *ev));
        true
    }
}

/// The mix's event state for one instrument, the model instrument, and the block runner.
pub(super) struct Rig {
    pub(super) e: Box<InstrumentEvents>,
    pub(super) m: Model,
    pub(super) frame: u64,
    /// Clip pedal values (CC 64) by frame, given in their block as the strip's clips are.
    clip: Vec<(u64, u8)>,
    st: EventStatus,
}

pub(super) const A: Source = Source { port: 1, channel: 0, generation: 1 };
pub(super) const B: Source = Source { port: 2, channel: 0, generation: 1 };

impl Rig {
    pub(super) fn new(mode: Mode) -> Rig {
        Rig { e: InstrumentEvents::new(), m: Model::new(mode), frame: 0, clip: Vec::new(), st: EventStatus::default() }
    }

    /// A live event from `source`, due at `at` (render frame).
    pub(super) fn live(&mut self, at: u64, source: Source, message: Message) {
        let r = self.e.admit(at, source, None, message, &mut self.st);
        assert!(!matches!(r, Admit::Shed(_)), "admitted: {message:?} at {at}");
    }

    /// A clip's CC 64 value at `at`.
    pub(super) fn clip_pedal(&mut self, at: u64, value: u8) {
        self.clip.push((at, value));
    }

    /// One block of `frames` with the chain of `t`.
    pub(super) fn run(&mut self, t: &Track, frames: usize) {
        let start = self.frame;
        let end = start + frames as u64;
        let (e, m) = (&mut self.e, &mut self.m);
        e.begin_fx(m, t, None, 0, &Transport::default(), 48_000.0, start, frames);
        e.retry_releases(m);
        for &(at, value) in self.clip.iter().filter(|(at, _)| (start..end).contains(at)) {
            e.clip_ctrl(m, u32::try_from(at - start).unwrap(), [0xB0, 64, value], false);
        }
        e.deliver(m, start, frames);
        e.fx.flush(m);
        m.process(&mut [], frames);
        self.frame = end;
    }

    /// Blocks of `frames` (the last one shorter) with the chain of `t` until frame `end`.
    pub(super) fn run_until(&mut self, t: &Track, end: u64, frames: usize) {
        while self.frame < end {
            let n = usize::try_from(end - self.frame).unwrap().min(frames);
            self.run(t, n);
        }
    }

    /// Stop, as the engine's reset: the host's and the mix's releases, then the instrument's
    /// all-notes-off and reset (`reset_render_state`); then one block.
    pub(super) fn stop(&mut self, t: &Track, frames: usize) {
        self.e.release_all(&mut self.m);
        self.m.all_notes_off();
        self.m.reset();
        self.run(t, frames);
    }

    /// What the instrument accepted from frame `from` on, summarized.
    pub(super) fn given_from(&self, from: u64) -> Vec<(u64, &'static str, u8, u8)> {
        self.m.given.iter().filter(|(f, _)| *f >= from).map(|(f, e)| summary(*f, e)).collect()
    }
}

pub(super) fn summary(f: u64, e: &PluginEvent) -> (u64, &'static str, u8, u8) {
    match e.message {
        Message::NoteOn { key, velocity, .. } => (f, "on", key, velocity),
        Message::NoteOff { key, .. } => (f, "off", key, 0),
        Message::Control { cc, value } => (f, "cc", cc, value),
        Message::PitchBend { value } => (f, "bend", (value >> 7) as u8, (value & 0x7F) as u8),
        _ => (f, "other", 0, 0),
    }
}

/// A track whose MIDI effects are `chain`: (id, parameters, bypassed).
pub(super) fn track(chain: &[(&str, &[(&str, f32)], bool)]) -> Track {
    let mut t = Track::new(TrackId(1), "Keys", TrackKind::Instrument, ChannelFormat::Stereo, TRACK_COLORS[0]);
    for (id, params, bypass) in chain {
        let mut fx = MidiEffectSlot::new(*id);
        for (k, v) in *params {
            fx.params.insert((*k).into(), *v);
        }
        fx.bypass = *bypass;
        t.midi_effects.push(fx);
    }
    t
}

/// The chain before the edit, and after it with `added` at its end. The effects keep their slot
/// identities (an effect the edit keeps is the same slot), as the session's edits do.
pub(super) fn chains(at: At, added: &[(&str, &[(&str, f32)], bool)]) -> (Track, Track) {
    let before = match at {
        At::First => track(&[]),
        At::Later => track(&[(OWN, &[], false)]),
    };
    let mut after = before.clone();
    after.midi_effects.extend(track(added).midi_effects);
    (before, after)
}

pub(super) fn on(key: u8) -> Message {
    Message::NoteOn { key, velocity: 100, id: NoteId::NONE }
}

pub(super) fn on_id(key: u8, id: u32) -> Message {
    Message::NoteOn { key, velocity: 100, id: NoteId(id) }
}

pub(super) fn off(key: u8) -> Message {
    Message::NoteOff { key, velocity: 0, id: NoteId::NONE }
}

pub(super) fn off_id(key: u8, id: u32) -> Message {
    Message::NoteOff { key, velocity: 0, id: NoteId(id) }
}

pub(super) fn cc(cc: u8, value: u8) -> Message {
    Message::Control { cc, value }
}
