//! The hosted stage (ADR 0064 HM2) with an in-process fake note plug-in in the slot
//! ([`super::hosted_fake`]) and a recording instrument: the stage ledger's pairing, every case of
//! 5.5, the wildcard combinations of 5.3, event-time order (5.2) and capacity (5.6). Expected values
//! are written from the record, at host block sizes 1, 7, 64 and 512 for well-formed scripts.

use super::hosted_fake::{Fake, Handle, midi, out};
use super::*;
use crate::{MIDI_FX_SLOT, PluginSlot, SlotGen};
use transit_dsp::hosted_out::{OutDrop, OutKind};

pub(super) const SIZES: [usize; 4] = [1, 7, 64, 512];
pub(super) const CLAP: &str = "clap:test.notes";
pub(super) const VST3: &str = "vst3:0123456789ABCDEF0123456789ABCDEF";

/// An instrument that writes down every event at its absolute frame.
#[derive(Default)]
pub(super) struct Inst {
    pub(super) at: u64,
    pub(super) heard: Vec<(u64, PluginEvent)>,
}

impl Plugin for Inst {
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
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        self.heard.push((self.at + u64::from(offset), *ev));
        true
    }
}

/// A plan cell holding `plugin` for a hosted stage of slot uid `uid`, generation `generation`.
pub(super) fn cell(plugin: Box<dyn Plugin>, id: &str, uid: u64, generation: u64) -> PluginSlot {
    let mut s = PluginSlot::new(plugin, id.to_string(), 2, MIDI_FX_SLOT);
    s.generation = SlotGen(generation);
    s.midi_stage = Some(crate::midi_fx::HostedStore::new(uid, 512));
    s
}

pub(super) struct Rig {
    pub(super) host: Box<MidiFxHost>,
    pub(super) inst: Inst,
    pub(super) track: Track,
    pub(super) cells: Vec<Option<PluginSlot>>,
    pub(super) fake: Handle,
    pub(super) frame: u64,
    pub(super) edits: Vec<u64>,
}

impl Rig {
    /// A track whose chain is one hosted stage `id`, its fake prepared for native calls of at most
    /// `native` frames, in the plan.
    pub(super) fn new(id: &str, native: usize) -> Rig {
        let mut track = track(&[]);
        track.midi_effects.push(MidiEffectSlot::new(id));
        let (mut fake, h) = Fake::new(native);
        fake.prepare(48_000.0, native, 2);
        let uid = track.midi_effects[0].uid.get();
        Rig { host: MidiFxHost::new(), inst: Inst::default(), track, cells: vec![Some(cell(Box::new(fake), id, uid, 7))], fake: h, frame: 0, edits: Vec::new() }
    }

    /// One block of `n` frames with the input events of `events` that fall in it (absolute frames).
    pub(super) fn block(&mut self, n: usize, events: &[(u64, PluginEvent)]) {
        self.inst.at = self.frame;
        let edit = self.host.begin_with(&mut self.inst, &mut self.cells, &self.track, None, 0, &Transport::default(), 48_000.0, self.frame, n);
        if edit.is_some() {
            // The mix's edit steps (ADR 0057, 2.13): the host's pedal-ups.
            self.host.release_pedals(&mut self.inst);
            self.edits.push(self.frame);
        }
        for (f, e) in events.iter().filter(|(f, _)| *f >= self.frame && *f < self.frame + n as u64) {
            let o = u32::try_from(f - self.frame).unwrap();
            if self.host.active() {
                assert!(self.host.take(&mut self.inst, o, e));
            } else {
                self.inst.event(o, e);
            }
        }
        if self.host.flush_with(&mut self.inst, &mut self.cells) {
            self.edits.push(self.frame);
        }
        self.frame += n as u64;
    }

    /// Blocks of `size` (the last one shorter) until `end`.
    pub(super) fn run(&mut self, end: u64, size: usize, events: &[(u64, PluginEvent)]) {
        while self.frame < end {
            let n = usize::try_from(end - self.frame).unwrap().min(size);
            self.block(n, events);
        }
    }

    /// An event sent between blocks.
    pub(super) fn between(&mut self, e: PluginEvent) {
        self.inst.at = self.frame;
        assert!(self.host.take(&mut self.inst, 0, &e));
    }

    /// Stop (or panic): the host's barrier, between blocks.
    pub(super) fn stop(&mut self) {
        self.inst.at = self.frame;
        self.host.panic(&mut self.inst);
    }

    /// The instrument's notes and controllers: (frame, kind, channel, key or controller, velocity
    /// or value, note id).
    pub(super) fn heard(&self) -> Vec<(u64, &'static str, u8, u16, u16, i32)> {
        self.inst.heard.iter().map(|(f, e)| desc(*f, e)).collect()
    }

    /// [`Self::heard`] from frame `from` on.
    pub(super) fn heard_from(&self, from: u64) -> Vec<(u64, &'static str, u8, u16, u16, i32)> {
        self.heard().into_iter().filter(|h| h.0 >= from).collect()
    }
}

pub(super) fn desc(f: u64, e: &PluginEvent) -> (u64, &'static str, u8, u16, u16, i32) {
    match e.message {
        Message::NoteOn { key, velocity, .. } => (f, "on", e.channel, u16::from(key), u16::from(velocity), e.note_id),
        Message::NoteOff { key, velocity, .. } => (f, "off", e.channel, u16::from(key), u16::from(velocity), e.note_id),
        Message::PolyPressure { key, value, .. } => (f, "pp", e.channel, u16::from(key), u16::from(value), e.note_id),
        Message::Control { cc, value } => (f, "cc", e.channel, u16::from(cc), u16::from(value), e.note_id),
        Message::PitchBend { value } => (f, "bend", e.channel, 0, value, e.note_id),
        Message::ChannelPressure { value } => (f, "at", e.channel, 0, u16::from(value), e.note_id),
        Message::Program { program } => (f, "pc", e.channel, 0, u16::from(program), e.note_id),
        Message::Control14 { msb, value } => (f, "cc14", e.channel, u16::from(msb), value, e.note_id),
    }
}

/// The instrument's voices after replaying what it heard: notes by channel and note id; panics on
/// a note-off for nothing sounding (a stray) or a note-on for a voice already sounding.
pub(super) fn sounding(heard: &[(u64, PluginEvent)]) -> Vec<(u8, i32)> {
    let mut open: Vec<(u8, i32)> = Vec::new();
    for (f, e) in heard {
        match e.message {
            Message::NoteOn { .. } => {
                assert!(!open.contains(&(e.channel, e.note_id)), "voice {} on {} started twice at {f}", e.note_id, e.channel);
                open.push((e.channel, e.note_id));
            }
            Message::NoteOff { .. } => {
                let i = open.iter().position(|v| *v == (e.channel, e.note_id));
                assert!(i.is_some(), "a stray note-off for voice {} on {} at {f}", e.note_id, e.channel);
                if let Some(i) = i {
                    open.remove(i);
                }
            }
            _ => {}
        }
    }
    open
}

/// Ends with a Stop and checks that nothing sounds afterwards and no note-off was a stray.
pub(super) fn stop_and_settle(r: &mut Rig, what: &str) {
    r.stop();
    assert!(sounding(&r.inst.heard).is_empty(), "{what}: nothing sounds after the barrier");
}

fn on(ch: i16, key: i16, id: i32, v: f64) -> transit_dsp::hosted_out::HostedOut {
    out(OutKind::NoteOn, ch, key, id, v)
}

fn off(ch: i16, key: i16, id: i32) -> transit_dsp::hosted_out::HostedOut {
    out(OutKind::NoteOff, ch, key, id, 0.0)
}

#[test]
fn a_generator_drives_the_instrument_at_every_scripted_frame_identically_at_every_block_size() {
    // Decision 3's translation after the ledger: velocities round(127 v) half away from zero; MIDI
    // bytes as written; 9n with velocity 0 a release.
    let want = vec![
        (10, "on", 0, 60, 64, 0),
        (100, "on", 1, 64, 90, 1),
        (250, "cc", 0, 7, 99, -1),
        (260, "bend", 2, 0, 8192, -1),
        (300, "off", 0, 60, 32, 0),
        (700, "off", 1, 64, 0, 1),
        (1000, "on", 3, 72, 127, 2),
    ];
    for size in SIZES {
        let mut r = Rig::new(CLAP, 512);
        r.fake.lock().at = vec![
            (10, on(0, 60, 7, 0.5)),
            (100, midi(OutKind::NoteOn, [0x91, 64, 90])),
            (250, midi(OutKind::Controller, [0xB0, 7, 99])),
            (260, midi(OutKind::PitchBend, [0xE2, 0x00, 0x40])),
            (300, out(OutKind::NoteOff, 0, 60, 7, 0.25)),
            (700, midi(OutKind::NoteOn, [0x91, 64, 0])),
            (1000, on(3, 72, -1, 1.0)),
        ];
        r.run(1024, size, &[]);
        assert_eq!(r.heard(), want, "block {size}");
        stop_and_settle(&mut r, &format!("block {size}"));
    }
}

#[test]
fn every_note_carries_a_host_id_and_overlapping_identified_notes_on_one_key_pair_exactly() {
    for size in SIZES {
        let mut r = Rig::new(CLAP, 512);
        // Two identified notes on one key, overlapping, with plug-in ids the stage never passes on.
        r.fake.lock().at = vec![(10, on(0, 60, 900, 1.0)), (20, on(0, 60, 901, 1.0)), (30, off(0, 60, 900)), (40, off(0, 60, 901))];
        r.run(512, size, &[]);
        let h = r.heard();
        assert_eq!(h, vec![(10, "on", 0, 60, 127, 0), (20, "on", 0, 60, 127, 1), (30, "off", 0, 60, 0, 0), (40, "off", 0, 60, 0, 1)], "block {size}");
        assert!(h.iter().all(|x| x.5 >= 0 && x.5 != 900 && x.5 != 901), "host ids only");
        stop_and_settle(&mut r, "pairing");
    }
}

/// One case of 5.5: the script, then Stop at `stop` (with the plug-in's later script), and the
/// instrument's notes expected by the record.
fn case(id: &str, script: Vec<(u64, transit_dsp::hosted_out::HostedOut)>, stop: Option<u64>, want: &[(u64, &str, u8, u16, u16, i32)], unmatched: u64) {
    for size in SIZES {
        let mut r = Rig::new(id, 512);
        r.fake.lock().at = script.clone();
        match stop {
            Some(at) => {
                r.run(at, size, &[]);
                r.stop();
                r.run(1536, size, &[]);
            }
            None => r.run(1536, size, &[]),
        }
        let got: Vec<(u64, &str, u8, u16, u16, i32)> = r.heard().into_iter().map(|(f, k, c, a, _, i)| (f, k, c, a, 0, i)).collect();
        assert_eq!(got, want, "{id} block {size}");
        assert_eq!(r.host.counts.unmatched_releases, unmatched, "{id} block {size}: releases consumed");
        stop_and_settle(&mut r, &format!("{id} block {size}"));
    }
}

#[test]
fn a_reused_id_closes_the_second_note_early_and_its_own_release_is_consumed() {
    // on(7), off(7), on(7), a duplicate off(7), then the second note's own off(7).
    let s = vec![(10, on(0, 60, 7, 1.0)), (20, off(0, 60, 7)), (30, on(0, 60, 7, 1.0)), (40, off(0, 60, 7)), (50, off(0, 60, 7))];
    case(CLAP, s, None, &[(10, "on", 0, 60, 0, 0), (20, "off", 0, 60, 0, 0), (30, "on", 0, 60, 0, 1), (40, "off", 0, 60, 0, 1)], 1);
}

#[test]
fn duplicate_open_ids_close_oldest_first() {
    let s = vec![(10, on(0, 60, 7, 1.0)), (20, on(0, 60, 7, 1.0)), (30, off(0, 60, 7)), (40, off(0, 60, 7))];
    case(CLAP, s, None, &[(10, "on", 0, 60, 0, 0), (20, "on", 0, 60, 0, 1), (30, "off", 0, 60, 0, 0), (40, "off", 0, 60, 0, 1)], 0);
}

#[test]
fn a_vst3_anonymous_release_closes_an_identified_note_and_its_later_identified_release_is_consumed() {
    let s = vec![(10, on(0, 60, 7, 1.0)), (20, off(0, 60, -1)), (30, off(0, 60, 7))];
    case(VST3, s, None, &[(10, "on", 0, 60, 0, 0), (20, "off", 0, 60, 0, 0)], 1);
}

#[test]
fn a_clap_wildcard_id_release_closes_every_open_note_of_its_key() {
    let s = vec![(10, on(0, 60, 7, 1.0)), (20, on(0, 60, 8, 1.0)), (25, on(0, 60, -1, 1.0)), (28, on(0, 61, 9, 1.0)), (30, off(0, 60, -1))];
    case(
        CLAP,
        s,
        None,
        &[
            (10, "on", 0, 60, 0, 0),
            (20, "on", 0, 60, 0, 1),
            (25, "on", 0, 60, 0, 2),
            (28, "on", 0, 61, 0, 3),
            (30, "off", 0, 60, 0, 0),
            (30, "off", 0, 60, 0, 1),
            (30, "off", 0, 60, 0, 2),
        ],
        0,
    );
}

#[test]
fn an_anonymous_duplicate_release_closes_the_second_anonymous_note_early() {
    let s = vec![(10, on(0, 60, -1, 1.0)), (20, off(0, 60, -1)), (30, on(0, 60, -1, 1.0)), (40, off(0, 60, -1)), (50, off(0, 60, -1))];
    case(VST3, s, None, &[(10, "on", 0, 60, 0, 0), (20, "off", 0, 60, 0, 0), (30, "on", 0, 60, 0, 1), (40, "off", 0, 60, 0, 1)], 1);
}

#[test]
fn a_stale_release_after_a_barrier_is_consumed_or_closes_the_note_that_reuses_its_address() {
    // Without reuse: the barrier (Stop at 512) ends the note; its release at 600 matches nothing.
    let s = vec![(10, on(0, 60, 7, 1.0)), (600, off(0, 60, 7))];
    case(CLAP, s, Some(512), &[(10, "on", 0, 60, 0, 0), (512, "off", 0, 60, 0, 0)], 1);
    // With reuse: a new note with the same id at 600; the stale release at 620 closes it early, and
    // the new note's own release at 700 is consumed.
    let s = vec![(10, on(0, 60, 7, 1.0)), (600, on(0, 60, 7, 1.0)), (620, off(0, 60, 7)), (700, off(0, 60, 7))];
    case(CLAP, s, Some(512), &[(10, "on", 0, 60, 0, 0), (512, "off", 0, 60, 0, 0), (600, "on", 0, 60, 0, 1), (620, "off", 0, 60, 0, 1)], 1);
    // The anonymous form (VST3): the same key reused.
    let s = vec![(10, on(0, 60, -1, 1.0)), (600, on(0, 60, -1, 1.0)), (620, off(0, 60, -1)), (700, off(0, 60, -1))];
    case(VST3, s, Some(512), &[(10, "on", 0, 60, 0, 0), (512, "off", 0, 60, 0, 0), (600, "on", 0, 60, 0, 1), (620, "off", 0, 60, 0, 1)], 1);
}

/// The open notes of the wildcard cases: A (0, 60, id 1), B (0, 60, id 2), C (0, 62, id 3),
/// D (1, 60, id 4), E (0, 60, anonymous), host ids 0 to 4 in that order.
fn wildcard_case(release: transit_dsp::hosted_out::HostedOut, want: &[i32], what: &str) {
    for size in SIZES {
        let mut r = Rig::new(CLAP, 512);
        r.fake.lock().at = vec![
            (10, on(0, 60, 1, 1.0)),
            (11, on(0, 60, 2, 1.0)),
            (12, on(0, 62, 3, 1.0)),
            (13, on(1, 60, 4, 1.0)),
            (14, on(0, 60, -1, 1.0)),
            (20, release),
        ];
        r.run(512, size, &[]);
        let closed: Vec<i32> = r.heard_from(20).iter().filter(|h| h.1 == "off").map(|h| h.5).collect();
        assert_eq!(closed, want, "{what}, block {size}: each closed entry with its own note-off, oldest first");
        assert!(r.heard_from(20).iter().all(|h| h.0 == 20));
        stop_and_settle(&mut r, what);
    }
}

#[test]
fn every_wildcard_combination_closes_exactly_the_entries_it_matches() {
    let rel = |ch: i16, key: i16, id: i32| off(ch, key, id);
    wildcard_case(rel(0, 60, -1), &[0, 1, 4], "id");
    wildcard_case(rel(0, -1, 1), &[0], "key");
    wildcard_case(rel(-1, 60, 4), &[3], "channel");
    wildcard_case(rel(0, -1, -1), &[0, 1, 2, 4], "id and key");
    wildcard_case(rel(-1, 60, -1), &[0, 1, 3, 4], "id and channel");
    wildcard_case(rel(-1, -1, 3), &[2], "key and channel");
    wildcard_case(rel(-1, -1, -1), &[0, 1, 2, 3, 4], "all");
    // The port: a wildcard port (-1) applies to the read port; the adapter hands an event on another
    // port as dropped, and it closes nothing.
    wildcard_case(transit_dsp::hosted_out::HostedOut { port: -1, ..rel(0, 62, 3) }, &[2], "port");
    wildcard_case(transit_dsp::hosted_out::HostedOut { port: -1, ..rel(-1, -1, -1) }, &[0, 1, 2, 3, 4], "port with all");
    wildcard_case(transit_dsp::hosted_out::HostedOut { port: 0, kind: OutKind::Dropped(OutDrop::OtherOutput), ..rel(-1, -1, -1) }, &[], "another port");
    // A choke is a release like a note-off.
    wildcard_case(transit_dsp::hosted_out::HostedOut { kind: OutKind::Choke, ..rel(0, 60, -1) }, &[0, 1, 4], "choke");
}

#[test]
fn pressure_reaches_exactly_the_matching_entries() {
    for size in SIZES {
        let mut r = Rig::new(CLAP, 512);
        r.fake.lock().at = vec![
            (10, on(0, 60, 1, 1.0)),
            (11, on(0, 60, 2, 1.0)),
            (12, on(0, 62, 3, 1.0)),
            (13, on(1, 60, 4, 1.0)),
            (14, on(0, 60, -1, 1.0)),
            (20, out(OutKind::PolyPressure, 0, 60, -1, 0.5)),
            (21, out(OutKind::PolyPressure, 0, 62, 3, 1.0)),
            (22, out(OutKind::PolyPressure, -1, -1, -1, 0.0)),
            (23, out(OutKind::PolyPressure, 0, 63, 9, 0.5)),
        ];
        r.run(512, size, &[]);
        let pp: Vec<(u64, u16, u16, i32)> = r.heard().iter().filter(|h| h.1 == "pp").map(|h| (h.0, h.3, h.4, h.5)).collect();
        let mut want = vec![(20, 60, 64, 0), (20, 60, 64, 1), (20, 60, 64, 4), (21, 62, 127, 2)];
        want.extend([(22, 60, 0, 0), (22, 60, 0, 1), (22, 62, 0, 2), (22, 60, 0, 3), (22, 60, 0, 4)]);
        assert_eq!(pp, want, "block {size}");
        assert_eq!(r.host.counts.unmatched_releases, 1, "the pressure for nothing open is consumed and counted");
        assert!(r.heard().iter().all(|h| h.1 != "off"), "pressure changes no ownership");
        stop_and_settle(&mut r, "pressure");
    }
}

#[test]
fn an_on_written_before_an_earlier_off_in_one_call_stays_open_until_the_next_barrier() {
    // 5.2: on at 10 written first, then off at 0, in one native call (host blocks of 64 and 512: the
    // case cannot run inside a block of one frame).
    for size in [64, 512] {
        let mut r = Rig::new(CLAP, 512);
        r.fake.lock().at = vec![(10, on(0, 60, 7, 1.0)), (0, off(0, 60, 7))];
        r.run(1024, size, &[]);
        assert_eq!(r.heard(), vec![(10, "on", 0, 60, 127, 0)], "block {size}: the off at 0 matched nothing and the note stays open");
        assert_eq!(r.host.counts.unmatched_releases, 1);
        r.stop();
        assert_eq!(r.heard_from(1024), vec![(1024, "off", 0, 60, 0, 0)], "the barrier ends it");
    }
}

#[test]
fn an_off_in_a_second_native_call_is_released_at_that_calls_start() {
    // Host blocks of 64, a native maximum of 16: on at local 10 of the first call, off at local 0 of
    // the second, which translates to block offset 16 (never earlier).
    let mut r = Rig::new(CLAP, 16);
    r.fake.lock().at_call = vec![(0, 10, on(0, 60, 7, 1.0)), (1, 0, off(0, 60, 7))];
    r.run(64, 64, &[]);
    assert_eq!(r.heard(), vec![(10, "on", 0, 60, 127, 0), (16, "off", 0, 60, 0, 0)]);
    assert_eq!(r.fake.lock().calls, 4, "four native calls");
}

#[test]
fn the_257th_held_note_is_rejected_and_its_releases_follow_the_record() {
    for size in SIZES {
        // CLAP, identified: 256 notes held, the 257th (id 999) rejected and recorded; its release
        // closes the record and writes nothing; every held note still ends at its own release.
        let mut r = Rig::new(CLAP, 512);
        let mut at: Vec<_> = (0..256).map(|k| (10, on(k / 128, k % 128, 1000 + i32::from(k), 1.0))).collect();
        at.push((10, on(0, 60, 999, 1.0)));
        at.push((20, off(0, 60, 999)));
        at.push((30, off(0, 60, 1060)));
        r.fake.lock().at = at;
        r.run(512, size, &[]);
        let h = r.heard();
        assert_eq!(h.iter().filter(|x| x.1 == "on").count(), 256, "block {size}: 256 delivered");
        assert_eq!(r.host.counts.refused_ons, 1, "the 257th refused and counted");
        assert_eq!(h.iter().filter(|x| x.1 == "off").map(|x| (x.0, x.3)).collect::<Vec<_>>(), vec![(30, 60)], "its release wrote nothing");
        assert_eq!(r.host.counts.unmatched_releases, 0);
        stop_and_settle(&mut r, "identified rejection");

        // VST3, anonymous: the 257th is an anonymous note on (0, 5), where an identified note (id 5)
        // is open and older: the first anonymous release closes the older identified note, the second
        // closes the record and writes nothing, and the identified release then matches nothing.
        let mut r = Rig::new(VST3, 512);
        let mut at: Vec<_> = (0..256).map(|k| (10, on(k / 128, k % 128, i32::from(k), 1.0))).collect();
        at.push((10, on(0, 5, -1, 1.0)));
        at.push((20, off(0, 5, -1)));
        at.push((30, off(0, 5, -1)));
        at.push((40, off(0, 5, 5)));
        r.fake.lock().at = at;
        r.run(512, size, &[]);
        let offs: Vec<_> = r.heard().into_iter().filter(|x| x.1 == "off").map(|x| (x.0, x.3, x.5)).collect();
        assert_eq!(offs, vec![(20, 5, 5)], "block {size}: one note-off, for the identified note's host id");
        assert_eq!(r.host.counts.unmatched_releases, 1);
        stop_and_settle(&mut r, "anonymous rejection");
    }
}

#[test]
fn with_the_record_table_full_a_rejections_release_shortens_a_note_and_never_writes_a_second_note_off() {
    for size in SIZES {
        let mut r = Rig::new(VST3, 512);
        // 256 identified notes open (keys 0..127 on channels 0 and 1); 256 rejected ones recorded
        // (anonymous, channel 2); one more rejected with the table full: anonymous on (0, 5).
        let mut at: Vec<_> = (0..256).map(|k| (10, on(k / 128, k % 128, i32::from(k), 1.0))).collect();
        at.extend((0..256).map(|k| (600, on(2, k % 128, -1, 1.0))));
        at.push((1200, on(0, 5, -1, 1.0)));
        // Its release closes the oldest open entry of (0, 5): the identified note, early; that note's
        // own release then matches nothing.
        at.push((1300, off(0, 5, -1)));
        at.push((1400, off(0, 5, 5)));
        r.fake.lock().at = at;
        r.run(1536, size, &[]);
        assert_eq!(r.host.counts.rejected_unrecorded, 1, "block {size}");
        let offs: Vec<_> = r.heard().into_iter().filter(|x| x.1 == "off").map(|x| (x.0, x.2, x.3, x.5)).collect();
        assert_eq!(offs, vec![(1300, 0, 5, 5)], "block {size}: shortened once, never a second note-off");
        stop_and_settle(&mut r, "saturation");
    }
}

/// Observed, not the card's expectation (5.7 stopped on, see the hand-back): a note-off that finds
/// the stage's output full (512 events) is refused, which is ADR 0057 P3b's reset-barrier trigger,
/// so the barrier ends the note at the block's last frame and drops the owed release with the
/// ledger; nothing is written at offset 0 of the next block.
#[test]
fn a_note_off_a_full_stage_output_refuses_is_ended_by_the_reset_barrier() {
    let mut r = Rig::new(VST3, 512);
    // 300 notes in block 0; in block 1, 212 controller events (non-releases) and 300 note-offs: 384
    // non-releases at most, 512 events in all, so the 301st event of the releases is refused.
    let mut at: Vec<_> = (0..256).map(|k| (10, on(k / 128, k % 128, i32::from(k), 1.0))).collect();
    at.extend((0..300).map(|k| (520, midi(OutKind::Controller, [0xB0, 1, (k % 128) as u8]))));
    at.extend((0..256).map(|k| (530, off(k / 128, k % 128, i32::from(k)))));
    r.fake.lock().at = at;
    r.run(1024, 512, &[]);
    let offs = r.heard().into_iter().filter(|x| x.1 == "off").count();
    assert_eq!(offs, 256, "every note ended once");
    assert_eq!(r.host.counts.safety_resets, 1, "the reset barrier ran");
    let late: Vec<_> = r.heard().into_iter().filter(|x| x.1 == "off" && x.0 != 530).map(|x| x.0).collect();
    assert!(late.iter().all(|f| *f == 1023), "the refused ones ended by the barrier at the block's last frame: {late:?}");
    stop_and_settle(&mut r, "full output");
}
