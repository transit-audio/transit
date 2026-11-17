//! The mix-state gate (ADR 0029, amendment for #742, section 14): the strip's own state, apart
//! from its plug-ins' internals (each entry's declaration and its fallback window cover those).
//!
//! It is three things:
//! - **the recursive inventory**, in code: `transit_mix`'s strip print destructures `Strip`,
//!   `PluginSlot`, `InstrumentEvents` (with `Queued`, `NoteEntry`, `PedalEntry`), `MidiFxHost`,
//!   `NoteDelay`, `NoteView`, `KeyIn`, the key bank entries, the mix `DelayLine`, `EdgeTap`,
//!   `TouchRamps`, `ClipFxTable`, `ClipFx` and `Applied` exhaustively, so a new field does not
//!   compile until its treatment is decided;
//! - **the logical-state comparison**: two renders of one session print the same at every block,
//!   and a strip whose events a render worker's pool parked prints as one that queued them (the
//!   canonical pending form, through the scheduling gate's continuity session);
//! - **targeted omission controls**: for each encoded field or group, two strips driven into
//!   states that differ only in that field ([`Nudge`]) must print differently, in the part that
//!   holds the field; and **poison controls** for the storage the inventory calls unobservable
//!   ([`Poison`]): one of two identical renders fills it with a fixed pattern between blocks, the
//!   outputs and prints must stay identical, and the buffer's write site must reach it.
//!
//! No restoration: the strip is not rebuilt from its record. Nothing here holds a strip with a
//! hardware insert or an External Instrument (the scope assertions of `sched.rs` and the print).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod common;

use common::workers::{Rig, SR};
use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_mix::{Arrival, Due, INSTRUMENT_SLOT, InstrumentEvent, Message, MixEngine, NoteId, Nudge, Poison, RenderFrame, Source, StripPrint};
use transit_model::{
    ChannelFormat, Clip, Insert, KeyFrom, KeySource, MidiEffectSlot, Route, SendSlot, Session, SourceAudio, SourceId, Tap, TrackId, TrackKind,
};

const BLOCK: usize = 256;
const BLOCKS: usize = 24;
const LEN: usize = BLOCK * 64;

fn kick() -> Vec<f32> {
    (0..LEN).map(|i| if i % 3_000 < 500 { 0.8 } else { 0.0 }).collect()
}

fn tone(step: f32) -> Vec<f32> {
    (0..LEN).map(|i| 0.1 * (i as f32 * step).sin()).collect()
}

fn audio(s: &mut Session, name: &str, sig: Vec<f32>, format: ChannelFormat) -> (TrackId, u64) {
    let src = SourceId(s.next_id + 10_000);
    let channels = if format == ChannelFormat::Mono { vec![sig] } else { vec![sig.clone(), sig] };
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels })));
    let t = s.add_track(TrackKind::Audio, format, Some(name));
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "c", src, 0, 0, LEN as i64));
    tr.mixer.output = Route::Main;
    (t, cid.0)
}

fn compressor(lookahead: f32, key: Option<KeySource>) -> Insert {
    let mut c = Insert::new("compressor");
    for p in transit_dsp::plugin_info("compressor").unwrap().params {
        c.params.insert(p.id.to_string(), p.default);
    }
    for (k, v) in [("threshold", -24.0), ("ratio", 8.0), ("attack", 1.0), ("release", 40.0), ("lookahead", lookahead)] {
        c.params.insert(k.into(), v);
    }
    c.key = key;
    c
}

/// The gate's session, built so every claimed read path runs:
/// - Kick: a stereo audio track with a clip whose clip effects (EQ and compressor) are on
///   (`clipbuf`), a pre-fader send to a bus (`pre`), and a lookahead compressor (latency);
/// - Synth: a mono MIDI track with an instrument (rendered in stereo: its instrument writes
///   `scratch` as its second channel, which the strip folds in)
///   with a MIDI effect (`a`, `b`, `merged`), a Filter that reads the track's notes (its note
///   delay), and a compressor keyed from the latent Kick (a late key: `KeyIn::scratch`);
/// - Bass and Pad: Pad's compressor keyed from Bass behind a lookahead compressor (an early key:
///   the key entry's `buf`, line and `scratch`); Bass, with no latency of its own, reaches Main through its
///   delay line (`outbuf`);
/// - an aux reading the send bus.
struct Gate {
    session: Session,
    kick: TrackId,
    synth: TrackId,
    bass: TrackId,
    pad: TrackId,
}

fn gate() -> Gate {
    let mut s = Session::default();
    s.edit.click = false;
    s.edit.delay_compensation = true;
    let bus = s.add_bus("Send", ChannelFormat::Stereo);
    let (kick, clip) = audio(&mut s, "Kick", kick(), ChannelFormat::Stereo);
    for (module, id) in [("eq", "eq_7band"), ("comp", "compressor")] {
        let p = transit_dsp::plugin_info(id).unwrap().params.first().unwrap();
        s.edit.values.insert(format!("clip_fx.{clip}.{module}.{}", p.id), f64::from(p.default));
    }
    {
        let t = s.track_mut(kick).unwrap();
        t.mixer.inserts[0] = Some(compressor(5.0, None));
        let mut send = SendSlot::new(Route::Bus(bus));
        send.level_db = -6.0;
        send.pre_fader = true;
        t.mixer.sends[0] = Some(send);
    }
    let synth = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Synth"));
    {
        let t = s.track_mut(synth).unwrap();
        t.instrument = Some(Insert::new("texture_synth"));
        t.midi_effects.push(MidiEffectSlot::new("midi_transpose"));
        t.mixer.inserts[0] = Some(Insert::new("filter"));
        t.mixer.inserts[1] = Some(compressor(0.0, Some(KeySource { from: KeyFrom::Track(kick), tap: Tap::Post })));
        t.mixer.output = Route::Main;
    }
    let (bass, _) = audio(&mut s, "Bass", tone(0.013), ChannelFormat::Stereo);
    let (pad, _) = audio(&mut s, "Pad", tone(0.007), ChannelFormat::Stereo);
    {
        let t = s.track_mut(pad).unwrap();
        t.mixer.inserts[0] = Some(compressor(5.0, None));
        t.mixer.inserts[1] = Some(compressor(0.0, Some(KeySource { from: KeyFrom::Track(bass), tap: Tap::Post })));
    }
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Return"));
    {
        let t = s.track_mut(aux).unwrap();
        t.mixer.input = Route::Bus(bus);
        t.mixer.output = Route::Main;
    }
    Gate { session: s, kick, synth, bass, pad }
}

fn rig(g: &Gate) -> Rig {
    MixEngine::force_serial(true);
    Rig::with_session(g.session.clone(), Vec::new(), BLOCK, |_| None)
}

/// Queues `message` on the Synth's instrument, `ahead` frames past the next block start.
fn send(rig: &mut Rig, synth: TrackId, ahead: u64, message: Message, seq: u64) {
    let binding = *rig.comp.active_bindings().iter().find(|b| b.0 == synth && b.1 == INSTRUMENT_SLOT).unwrap();
    let ev = InstrumentEvent {
        engine: rig.epoch,
        target: binding,
        due: Due::Frame(RenderFrame(rig.mix.next_frame().0 + ahead)),
        transport: None,
        source: Source { port: 1, channel: 0, generation: 1 },
        message,
        arrival: Arrival { host_ns: 0, sequence: seq },
    };
    rig.mix.queue_event(&ev);
}

/// Plays the Synth: a note every few blocks, its off two blocks later.
fn play(rig: &mut Rig, synth: TrackId, b: usize) {
    let key = 48 + (b % 12) as u8;
    if b % 4 == 0 {
        send(rig, synth, 17, Message::NoteOn { key, velocity: 100, id: NoteId::NONE }, b as u64 * 2);
    }
    if b % 4 == 2 {
        send(rig, synth, 33, Message::NoteOff { key: 48 + ((b - 2) % 12) as u8, velocity: 0, id: NoteId::NONE }, b as u64 * 2 + 1);
    }
}

fn print_of(rig: &Rig, track: TrackId) -> StripPrint {
    let i = rig.mix.strip_index(track).unwrap();
    rig.mix.strip_prints()[i].unwrap()
}

#[test]
fn two_renders_of_one_session_print_the_same_at_every_block() {
    let g = gate();
    let (mut a, mut b) = (rig(&g), rig(&g));
    for k in 0..BLOCKS {
        play(&mut a, g.synth, k);
        play(&mut b, g.synth, k);
        a.block();
        b.block();
        assert_eq!(a.out, b.out, "block {k}");
        assert_eq!(a.mix.strip_prints(), b.mix.strip_prints(), "block {k}");
    }
}

/// Section 14's targeted omission controls: each nudge changes one field of one strip, and that
/// strip's print must differ in the part holding the field, nowhere else.
#[test]
fn a_strip_differing_in_one_inventoried_field_prints_differently() {
    let g = gate();
    let controls = [
        (Nudge::MidiEpoch, g.synth, "midi"),
        (Nudge::MidiRetry, g.synth, "midi"),
        (Nudge::NoteViewFull, g.synth, "midi"),
        (Nudge::RackExit, g.synth, "midi"),
        (Nudge::NoteOpen, g.synth, "midi"),
        (Nudge::LastOn, g.synth, "midi"),
        (Nudge::RestrikeCost, g.synth, "midi"),
        (Nudge::AppliedValue, g.kick, "strip"),
        (Nudge::AppliedSentinel, g.kick, "strip"),
        (Nudge::NoteDelayStandIn, g.synth, "strip"),
        (Nudge::TouchRamp, g.kick, "strip"),
        (Nudge::Latency, g.kick, "strip"),
        (Nudge::ClipFxToggle, g.kick, "strip"),
        (Nudge::KeyPending, g.pad, "lines"),
    ];
    for (what, track, part) in controls {
        let (mut a, mut b) = (rig(&g), rig(&g));
        for k in 0..6 {
            play(&mut a, g.synth, k);
            play(&mut b, g.synth, k);
            a.block();
            b.block();
        }
        // A note-on two blocks ahead: a queued entry and an open note in the ledger.
        send(&mut a, g.synth, 2 * BLOCK as u64, Message::NoteOn { key: 70, velocity: 90, id: NoteId::NONE }, 1_000);
        send(&mut b, g.synth, 2 * BLOCK as u64, Message::NoteOn { key: 70, velocity: 90, id: NoteId::NONE }, 1_000);
        assert_eq!(print_of(&a, track), print_of(&b, track), "{what:?}: equal before the nudge");
        assert!(b.mix.nudge(track, what), "{what:?}: the strip has the field");
        let (x, y) = (print_of(&a, track), print_of(&b, track));
        assert_eq!(x.differing(&y), [part], "{what:?}: only the part holding the field differs");
    }
}

/// Section 14's poison controls: storage the inventory calls unobservable, filled with a fixed
/// pattern between blocks in one of two identical renders, changes no output and no print, and
/// its write site reaches it.
#[test]
fn poisoning_unobservable_storage_changes_no_output_and_no_print() {
    let g = gate();
    let targets = [
        (Poison::Buf, g.kick),
        (Poison::Pre, g.kick),
        (Poison::Clipbuf, g.kick),
        (Poison::Outbuf, g.bass),
        (Poison::Scratch, g.synth),
        (Poison::KeyInScratch, g.synth),
        (Poison::KeyBuf, g.pad),
        (Poison::KeyScratch, g.pad),
        (Poison::MidiA, g.synth),
        (Poison::MidiB, g.synth),
        (Poison::MidiMerged, g.synth),
    ];
    for (target, track) in targets {
        let (mut a, mut b) = (rig(&g), rig(&g));
        let (mut changed, mut reached) = (0, 0);
        for k in 0..BLOCKS {
            if k >= 3 {
                let n = b.mix.poison(track, target);
                changed += n;
            }
            play(&mut a, g.synth, k);
            play(&mut b, g.synth, k);
            a.block();
            b.block();
            if k >= 3 {
                reached += usize::from(b.mix.poison_reached(track, target));
            }
            assert_eq!(a.out, b.out, "{target:?}, block {k}: the output");
            assert_eq!(a.mix.strip_prints(), b.mix.strip_prints(), "{target:?}, block {k}: the prints");
        }
        assert!(changed > 0, "{target:?}: the pattern changed the storage");
        // A float buffer is overwritten in every block; the MIDI host's storage in the blocks
        // whose events go through the chain.
        let midi = matches!(target, Poison::MidiA | Poison::MidiB | Poison::MidiMerged);
        let poisoned = BLOCKS - 3;
        assert!(
            if midi { reached > 0 } else { reached == poisoned },
            "{target:?}: the write site reached the poisoned storage ({reached} of {poisoned} blocks)"
        );
    }
}

/// Adopts `s` into `rig` (the same structure, other values: its instances are kept).
fn adopt(rig: &mut Rig, s: &Session, generation: u64) {
    let mut make = |spec: &transit_mix::InstanceSpec, _: transit_mix::SlotGen| {
        transit_mix::create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch)
    };
    let mut env = rig.comp.compile(Arc::new(s.clone()), rig.comp.active(), transit_mix::PlanGeneration::from_raw(generation), &mut make).unwrap();
    assert_eq!(rig.mix.adopt_plan(&mut env), transit_mix::Adoption::Adopted);
    rig.comp.reclaim(env);
}

/// The idle delay compensation line's contents (the mix `DelayLine` while not live): Kick, the
/// latent strip, reads no edge delayed, so its lines are idle; poisoned in one render, they change
/// nothing, and once Kick's lookahead goes (block 8) its edges are delayed to Pad's latency, the
/// line comes alive and its first write clears the ring before any read.
#[test]
fn poisoning_an_idle_delay_line_changes_no_output_and_no_print() {
    let g = gate();
    let mut later = g.session.clone();
    later.track_mut(g.kick).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("lookahead".into(), 0.0);
    let (mut a, mut b) = (rig(&g), rig(&g));
    let (mut changed, mut reached) = (0, 0);
    for k in 0..BLOCKS {
        if k == 8 {
            adopt(&mut a, &later, 2);
            adopt(&mut b, &later, 2);
        }
        if k >= 3 {
            changed += b.mix.poison(g.kick, Poison::IdleTap);
        }
        play(&mut a, g.synth, k);
        play(&mut b, g.synth, k);
        a.block();
        b.block();
        reached += usize::from(k >= 3 && b.mix.poison_reached(g.kick, Poison::IdleTap));
        assert_eq!(a.out, b.out, "block {k}: the output");
        assert_eq!(a.mix.strip_prints(), b.mix.strip_prints(), "block {k}: the prints");
    }
    assert!(changed > 0, "the pattern changed the idle line");
    assert!(reached > 0, "the line came alive and its write reached the poisoned contents");
}
