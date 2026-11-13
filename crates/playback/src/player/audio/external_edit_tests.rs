//! #556 on the external MIDI path: an edit adopted while a clip note sounds on a hardware synth
//! must not take the note's release with it. Every note-on pushed for the output thread gets
//! exactly one note-off, and no off goes out for a note that never started. Seeded edits at
//! block sizes 1, 7, 64 and 512: no wall clock, no randomness at run time.
#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipContent, ClipId, MidiOutChannel, MidiOutPort, TrackId, TrackKind};

/// Host time of session sample 0, and a whole number of nanoseconds per sample, so a stamp maps
/// back to its sample exactly.
const H0: u64 = 1_000_000_000;
const NS: u64 = 1_000;
const TICK: i64 = 25;

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

fn session(clips: &[(i64, i64, Vec<Note>)]) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    for (start, length, notes) in clips {
        let cid = s.new_clip_id();
        let seq = Sequence { notes: notes.clone(), ..Default::default() };
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", *start, *length, seq));
    }
    let track = s.track_mut(t).unwrap();
    track.midi_output = MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::AsRecorded, offset_ms: 0.0 };
    (s, t)
}

fn notes_mut(s: &mut Session, cid: ClipId) -> &mut Vec<Note> {
    let ClipContent::Midi { sequence } = &mut s.find_clip_mut(cid).unwrap().content else { panic!("a MIDI clip") };
    &mut sequence.notes
}

fn clip_ids(s: &Session, t: TrackId) -> Vec<ClipId> {
    s.track(t).unwrap().clips().iter().map(|c| c.id).collect()
}

/// The audio thread's side of the external path with the output ring read back: each push as
/// (session sample, message bytes); markers are kept to show there were none.
struct Drive {
    external: External,
    rx: rtrb::Consumer<OutEvent>,
    plan: u64,
    block: usize,
    sent: Vec<(u64, Kind, [u8; 3])>,
}

impl Drive {
    fn new(block: usize) -> Self {
        let (out, rx) = crate::player::midi_out::test_ring(4096);
        Drive { external: External::new(out), rx, plan: 1, block, sent: Vec::new() }
    }

    /// An edit adopted: the next block plays another plan.
    fn adopt(&mut self) {
        self.plan += 1;
    }

    fn render(&mut self, s: &Session, pos: i64) {
        let block = Block {
            playing: true,
            position: pos,
            frames: self.block,
            host_ns: Some(H0 + pos as u64 * NS),
            delay_ns: 0,
            ns_per_frame: NS as f64,
            plan: PlanGeneration::from_raw(self.plan),
        };
        self.external.block(s, block);
        while let Ok(e) = self.rx.pop() {
            self.sent.push(((e.at_ns - H0) / NS, e.kind, e.bytes));
        }
    }
}

fn minimal_case(edit: impl FnOnce(&mut Session, TrackId, ClipId)) -> Vec<(u64, Kind, [u8; 3])> {
    let (s, t) = session(&[(0, 96_000, vec![note(60, 0, 1920)])]);
    let cid = clip_ids(&s, t)[0];
    let mut d = Drive::new(512);
    let mut pos = 0;
    while pos <= 23_552 {
        d.render(&s, pos);
        pos += 512;
    }
    let mut edited = s.clone();
    edit(&mut edited, t, cid);
    d.adopt();
    while pos <= 96_000 {
        d.render(&edited, pos);
        pos += 512;
    }
    d.sent
}

#[test]
fn an_edit_that_removes_a_sounding_notes_off_releases_it_once_on_the_external_port() {
    type Edit = fn(&mut Session, TrackId, ClipId);
    let variants: [(&str, Edit); 4] = [
        ("note removed", |s, _, c| notes_mut(s, c).clear()),
        ("clip muted", |s, _, c| s.find_clip_mut(c).unwrap().muted = true),
        ("transpose +12", |s, t, _| {
            s.edit.values.insert(format!("rtp.{}.transpose", t.0), 12.0);
        }),
        ("shortened to 480 ticks", |s, _, c| notes_mut(s, c)[0].length = 480),
    ];
    for (name, edit) in variants {
        let got = minimal_case(edit);
        assert_eq!(got, [(0, Kind::Message, [0x90, 60, 100]), (24_064, Kind::Message, [0x80, 60, 64])], "{name}");
    }
}

#[test]
fn an_edit_that_keeps_the_sounding_note_sends_its_off_where_the_clip_has_it() {
    let got = minimal_case(|s, _, c| notes_mut(s, c)[0].length = 2400);
    assert_eq!(got, [(0, Kind::Message, [0x90, 60, 100]), (60_000, Kind::Message, [0x80, 60, 64])]);
}

#[test]
fn a_track_that_goes_internal_or_changes_port_is_left_to_the_output_threads_release() {
    // The output thread ends what the track held on its old destination: no off from here, and
    // none later from the clip (the note-on went to the old destination).
    type Edit = fn(&mut Session, TrackId, ClipId);
    let variants: [(&str, Edit); 3] = [
        ("internal", |s, t, _| s.track_mut(t).unwrap().midi_output = MidiOutput::Internal),
        ("another port", |s, t, _| {
            if let MidiOutput::External { port, .. } = &mut s.track_mut(t).unwrap().midi_output {
                port.name = "Other".into();
            }
        }),
        ("another channel", |s, t, _| {
            if let MidiOutput::External { channel, .. } = &mut s.track_mut(t).unwrap().midi_output {
                *channel = MidiOutChannel::Channel(3);
            }
        }),
    ];
    for (name, edit) in variants {
        let got = minimal_case(edit);
        assert_eq!(got, [(0, Kind::Message, [0x90, 60, 100])], "{name}");
    }
}

#[test]
fn an_off_for_a_note_that_never_went_out_is_not_sent() {
    // Muted from the start: the note-on is not sent, so neither is its off.
    let (mut s, t) = session(&[(0, 96_000, vec![note(60, 0, 480)])]);
    s.track_mut(t).unwrap().mixer.mute = true;
    let mut d = Drive::new(512);
    for pos in (0..24_064).step_by(512) {
        d.render(&s, pos);
    }
    assert!(d.sent.is_empty(), "{:?}", d.sent);
}

// ---- the property: random edits mid-note ----

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo).max(1) as u64) as i64
    }
}

const EDITS: [&str; 18] = [
    "delete note",
    "delete clip",
    "mute clip",
    "change pitch",
    "transpose track",
    "shorten note",
    "lengthen note",
    "move note",
    "move clip",
    "trim clip end",
    "trim clip start",
    "split clip",
    "track duration",
    "track delay",
    "midi track offset",
    "tempo",
    "undo",
    "undo and redo",
];

fn trim_start(s: &mut Session, cid: ClipId, to: i64) {
    let (tempo, sr) = (s.tempo.clone(), s.sample_rate);
    let c = s.find_clip_mut(cid).unwrap();
    let ticks = tempo.samples_to_ticks(to, sr) - tempo.samples_to_ticks(c.start, sr);
    c.trim_start_to(to);
    if let ClipContent::Midi { sequence } = &mut c.content {
        sequence.notes.retain(|n| n.start + n.length > ticks);
        sequence.notes.iter_mut().for_each(|n| n.start -= ticks);
    }
}

fn apply(kind: usize, s: &mut Session, t: TrackId, cid: ClipId, n: usize, rng: &mut Rng) {
    let key = |name: &str| format!("{name}.{}", t.0);
    let clip = s.find_clip_mut(cid).unwrap().clone();
    match EDITS[kind] {
        "delete note" => {
            notes_mut(s, cid).remove(n);
        }
        "delete clip" => s.track_mut(t).unwrap().playlist_mut().unwrap().clips.retain(|c| c.id != cid),
        "mute clip" => s.find_clip_mut(cid).unwrap().muted = true,
        "change pitch" => {
            let p = &mut notes_mut(s, cid)[n].pitch;
            *p = if rng.below(2) == 0 { *p + 1 + rng.below(4) as u8 } else { *p - 1 - rng.below(4) as u8 };
        }
        "transpose track" => {
            s.edit.values.insert(key("rtp") + ".transpose", [12.0, -12.0, 1.0, -3.0][rng.below(4) as usize]);
        }
        "shorten note" => {
            let x = &mut notes_mut(s, cid)[n];
            x.length = rng.range(1, x.length.max(2));
        }
        "lengthen note" => notes_mut(s, cid)[n].length += rng.range(1, 200),
        "move note" => {
            let x = &mut notes_mut(s, cid)[n];
            x.start = (x.start + rng.range(-80, 80)).max(0);
        }
        "move clip" => s.find_clip_mut(cid).unwrap().start = (clip.start + rng.range(-3000, 3000)).max(0),
        "trim clip end" => s.find_clip_mut(cid).unwrap().trim_end_to(rng.range(clip.start + 1, clip.end())),
        "trim clip start" => trim_start(s, cid, rng.range(clip.start + 1, clip.end() - 1)),
        "split clip" => {
            let cut = rng.range(clip.start + 1, clip.end() - 1);
            let mut right = clip.clone();
            right.id = s.new_clip_id();
            s.find_clip_mut(cid).unwrap().trim_end_to(cut);
            s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(right.clone());
            trim_start(s, right.id, cut);
        }
        "track duration" => {
            s.edit.values.insert(key("rtp") + ".duration", [10.0, 50.0, 150.0, 300.0][rng.below(4) as usize]);
        }
        "track delay" => {
            s.edit.values.insert(key("rtp") + ".delay", rng.range(-120, 120) as f64);
        }
        "midi track offset" => {
            s.edit.values.insert(key("midi.offset"), rng.range(-3000, 3000) as f64);
        }
        "tempo" => s.tempo.set_tempo(0, [60.0, 90.0, 150.0, 200.0][rng.below(4) as usize]).unwrap(),
        other => panic!("{other} is not a session edit"),
    }
}

fn case(block: usize, rng: &mut Rng) -> Result<(usize, Option<usize>), String> {
    const KEYS: [u8; 4] = [60, 62, 64, 67];
    let mut clips = Vec::new();
    for start in [0, 4800] {
        let notes = (0..5).map(|_| note(KEYS[rng.below(4) as usize], rng.range(0, 160), rng.range(8, 120))).collect();
        clips.push((start, 4800, notes));
    }
    let (s, t) = session(&clips);
    let ids = clip_ids(&s, t);
    let (c, n) = (rng.below(2) as usize, rng.below(5) as usize);
    let (cid, target): (ClipId, Note) = (ids[c], clips[c].2[n]);
    let on = clips[c].0 + target.start * TICK;
    let off = (on + target.length * TICK).min(clips[c].0 + 4800);
    let block_i = block as i64;
    let at = ((on + off) / 2 / block_i).max(1) * block_i;
    let kind = rng.below(EDITS.len() as u64) as usize;
    let mut edited = s.clone();
    let (first, later) = match EDITS[kind] {
        "undo" | "undo and redo" => {
            let k = rng.below(16) as usize;
            apply(k, &mut edited, t, cid, n, rng);
            (k, Some(kind))
        }
        _ => {
            apply(kind, &mut edited, t, cid, n, rng);
            (kind, None)
        }
    };
    let step = if block_i >= 64 { block_i * rng.range(1, 4) } else { (rng.range(64, 1200) / block_i).max(1) * block_i };
    let mut publishes = vec![(at, edited.clone())];
    if later.is_some() {
        publishes.push((at + step, s.clone()));
    }
    if later == Some(17) {
        publishes.push((at + 2 * step, edited.clone()));
    }
    let what =
        format!("block {block}, {} on clip {c} note {n} at {at}{}", EDITS[first], later.map_or(String::new(), |k| format!(" then {}", EDITS[k])));
    let mut d = Drive::new(block);
    let mut current = s.clone();
    let mut pos = 0i64;
    loop {
        for (when, session) in &publishes {
            if *when == pos {
                d.adopt();
                current = session.clone();
            }
        }
        let end = current.track(t).unwrap().clips().iter().map(|c| c.end()).max().unwrap_or(0);
        let offset = current.edit.values.get(&format!("midi.offset.{}", t.0)).copied().unwrap_or(0.0).max(0.0) as i64;
        let last_publish = publishes.last().map_or(0, |p| p.0);
        if pos > end + offset + block_i && pos > last_publish {
            break;
        }
        d.render(&current, pos);
        pos += block_i;
    }
    check(&d.sent).map(|_| (first, later)).map_err(|e| format!("{what}: {e}"))
}

/// As the output thread counts them: per channel and key, an off ends one open note-on. No off
/// for a key with none open, nothing left open, no release or stop marker in continuous play.
fn check(sent: &[(u64, Kind, [u8; 3])]) -> Result<usize, String> {
    let mut open = std::collections::BTreeMap::<(u8, u8), u32>::new();
    let mut ons = 0;
    for &(at, kind, bytes) in sent {
        if kind != Kind::Message {
            return Err(format!("a {kind:?} marker at {at} in continuous play"));
        }
        let k = (bytes[0] & 0x0F, bytes[1]);
        match bytes[0] & 0xF0 {
            0x90 => {
                *open.entry(k).or_default() += 1;
                ons += 1;
            }
            0x80 => match open.get_mut(&k) {
                Some(n) if *n > 0 => *n -= 1,
                _ => return Err(format!("off at {at} for channel {} key {}, which is not sounding", k.0, k.1)),
            },
            _ => {}
        }
    }
    open.retain(|_, n| *n > 0);
    if open.is_empty() { Ok(ons) } else { Err(format!("notes never released (channel, key: count): {open:?}")) }
}

#[test]
fn every_note_on_sent_gets_exactly_one_note_off_through_random_edits_mid_note() {
    let mut rng = Rng(0x0556_E57E_4A11_0001);
    let mut failures = Vec::new();
    let mut edits_seen = [0usize; EDITS.len()];
    for (block, cases) in [(1usize, 12usize), (7, 24), (64, 60), (512, 120)] {
        for _ in 0..cases {
            let before = rng.0;
            match case(block, &mut rng) {
                Ok((first, later)) => {
                    edits_seen[first] += 1;
                    if let Some(k) = later {
                        edits_seen[k] += 1;
                    }
                }
                Err(e) => failures.push(format!("seed {before:#x}: {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{} cases failed:\n{}", failures.len(), failures.join("\n"));
    assert!(edits_seen.iter().all(|&n| n > 0), "every edit kind ran: {edits_seen:?}");
}
