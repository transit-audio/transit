//! #556: an edit adopted while a clip note sounds must not take the note's release with it. Every
//! note-on that reaches the instrument gets exactly one note-off, whatever the edit: the note or
//! its clip deleted or muted, its pitch or the track's transpose changed, the note shortened or
//! moved, the clip moved, trimmed or split, the track's real-time duration or delay or its MIDI
//! Track Offset changed, the tempo edited, an edit undone and redone. For a built-in and a hosted
//! instrument, at block sizes 1, 7, 64 and 512. Seeded: no wall clock, no randomness at run time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use transit_dsp::Plugin;
use transit_midi::{Note, Sequence};
use transit_mix::{
    Adoption, DeviceGeneration, EngineEpoch, EventLimits, InstanceSpec, Message, MixEngine, NewInstance, PlanCompiler, PlanGeneration, PlanLimits,
    PluginEvent, RenderTarget, SlotGen,
};
use transit_model::{ChannelFormat, Clip, ClipContent, ClipId, Insert, Session, TrackId, TrackKind};
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const SR: u32 = 48_000;
const MAX_BLOCK: usize = 512;
/// At 120 bpm and 48 kHz a tick (960 per quarter) is 25 samples.
const TICK: i64 = 25;
/// A built-in instrument id and a hosted one: the mix treats the hosted slot as third party.
const INSTRUMENTS: [&str; 2] = ["macro_oscillator", "clap:test.rec"];

type Heard = Arc<Mutex<Vec<(u64, PluginEvent)>>>;

/// Writes down every note event it is given, at its absolute render frame.
struct Recorder {
    inner: Box<dyn Plugin>,
    heard: Heard,
    pending: Vec<(u32, PluginEvent)>,
    rendered: u64,
}

impl Plugin for Recorder {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let mut heard = self.heard.lock().unwrap();
        self.pending.sort_by_key(|(o, _)| *o);
        for (o, e) in self.pending.drain(..) {
            if matches!(e.message, Message::NoteOn { .. } | Message::NoteOff { .. }) {
                heard.push((self.rendered + u64::from(o), e));
            }
        }
        self.rendered += frames as u64;
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        self.pending.push((offset, *ev));
        true
    }
}

/// A realtime plan engine fed the way playback feeds it, rendering `block` frames per call.
struct Rig {
    mix: MixEngine,
    comp: PlanCompiler,
    heard: Heard,
    generation: PlanGeneration,
    block: usize,
    out: Vec<Vec<f32>>,
    made: usize,
}

impl Rig {
    fn new(block: usize) -> Rig {
        let target = RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: MAX_BLOCK as u32, out_channels: 0, in_channels: 0 };
        Rig {
            mix: MixEngine::realtime_with_events(target, EngineEpoch::FIRST, EventLimits::default()),
            comp: PlanCompiler::new(target, PlanLimits::default()),
            heard: Arc::new(Mutex::new(Vec::with_capacity(4096))),
            generation: PlanGeneration::EMPTY,
            block,
            out: vec![vec![0.0; MAX_BLOCK]; 2],
            made: 0,
        }
    }

    fn publish(&mut self, s: &Session) {
        self.generation = self.generation.checked_next().unwrap();
        let heard = Arc::clone(&self.heard);
        let made = &mut self.made;
        let mut make = |spec: &InstanceSpec, _: SlotGen| -> Option<NewInstance> {
            let mut inner = transit_dsp::create("gain")?;
            inner.prepare(SR as f32, MAX_BLOCK, spec.ch);
            *made += 1;
            let plugin = Box::new(Recorder { inner, heard: Arc::clone(&heard), pending: Vec::with_capacity(4096), rendered: 0 });
            Some(NewInstance { plugin, refused_state: false })
        };
        let mut env = self.comp.compile(Arc::new(s.clone()), self.comp.active(), self.generation, &mut make).unwrap();
        assert_eq!(self.mix.adopt_plan(&mut env), Adoption::Adopted);
        self.comp.reclaim(env);
        assert_eq!(self.made, 1, "the instrument instance moves through every edit");
    }

    fn render(&mut self, pos: i64) {
        self.mix.render_active(pos, self.block, &mut self.out);
    }

    /// The notes heard: (frame, on, key, voice id).
    fn notes(&self) -> Vec<(u64, bool, u8, i32)> {
        let heard = self.heard.lock().unwrap();
        heard
            .iter()
            .filter_map(|(f, e)| match e.message {
                Message::NoteOn { key, .. } => Some((*f, true, key, e.note_id)),
                Message::NoteOff { key, .. } => Some((*f, false, key, e.note_id)),
                _ => None,
            })
            .collect()
    }
}

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

fn session(id: &str, clips: &[(i64, i64, Vec<Note>)]) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new(id));
    for (start, length, notes) in clips {
        let cid = s.new_clip_id();
        let seq = Sequence { notes: notes.clone(), ..Default::default() };
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", *start, *length, seq));
    }
    (s, t)
}

fn notes_mut(s: &mut Session, cid: ClipId) -> &mut Vec<Note> {
    let ClipContent::Midi { sequence } = &mut s.find_clip_mut(cid).unwrap().content else { panic!("a MIDI clip") };
    &mut sequence.notes
}

fn clip_ids(s: &Session, t: TrackId) -> Vec<ClipId> {
    s.track(t).unwrap().clips().iter().map(|c| c.id).collect()
}

/// The issue's minimal case: one note (key 60, 1920 ticks = 48 000 samples) in a clip at 0; blocks
/// 0 to 23 552 played, then the edit published, then blocks 24 064 to 96 000.
fn minimal_case(id: &str, edit: impl FnOnce(&mut Session, TrackId, ClipId)) -> Vec<(u64, bool, u8, i32)> {
    let (s, t) = session(id, &[(0, 96_000, vec![note(60, 0, 1920)])]);
    let cid = clip_ids(&s, t)[0];
    let mut rig = Rig::new(512);
    rig.publish(&s);
    let mut pos = 0;
    while pos <= 23_552 {
        rig.render(pos);
        pos += 512;
    }
    let mut edited = s.clone();
    edit(&mut edited, t, cid);
    rig.publish(&edited);
    // The first block with the edited session releases the note: on the audio thread, without
    // allocating.
    let ((), stats) = measure(|| rig.render(pos));
    assert_eq!(stats.allocs, 0, "the release allocates nothing");
    pos += 512;
    while pos <= 96_000 {
        rig.render(pos);
        pos += 512;
    }
    rig.notes()
}

#[test]
fn an_edit_that_removes_a_sounding_notes_off_releases_it_once_at_the_next_block() {
    type Edit = fn(&mut Session, TrackId, ClipId);
    let variants: [(&str, Edit); 4] = [
        ("note removed", |s, _, c| notes_mut(s, c).clear()),
        ("clip muted", |s, _, c| s.find_clip_mut(c).unwrap().muted = true),
        ("transpose +12", |s, t, _| {
            s.edit.values.insert(format!("rtp.{}.transpose", t.0), 12.0);
        }),
        ("shortened to 480 ticks", |s, _, c| notes_mut(s, c)[0].length = 480),
    ];
    for id in INSTRUMENTS {
        for (name, edit) in variants {
            let got = minimal_case(id, edit);
            let shape: Vec<_> = got.iter().map(|&(f, on, key, _)| (f, on, key)).collect();
            assert_eq!(shape, [(0, true, 60), (24_064, false, 60)], "{id}, {name}: one on at 0 and its one off at the first edited block");
            assert_eq!(got[0].3, got[1].3, "{id}, {name}: the off ends the voice the on started");
        }
    }
}

#[test]
fn an_edit_that_keeps_the_sounding_note_leaves_it_alone() {
    // Lengthened, or another note added: the note sounds on and ends at its own (new) off.
    type Edit = fn(&mut Session, TrackId, ClipId);
    let variants: [(&str, Edit, u64); 2] = [
        ("lengthened to 2400 ticks", |s, _, c| notes_mut(s, c)[0].length = 2400, 60_000),
        ("another note added", |s, _, c| notes_mut(s, c).push(note(72, 1200, 240)), 48_000),
    ];
    for id in INSTRUMENTS {
        for (name, edit, off) in variants {
            let got = minimal_case(id, edit);
            let sixty: Vec<_> = got.iter().filter(|n| n.2 == 60).map(|&(f, on, _, _)| (f, on)).collect();
            assert_eq!(sixty, [(0, true), (off, false)], "{id}, {name}");
        }
    }
}

// ---- the property: random edits mid-note ----

/// A small deterministic generator (xorshift64*).
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

/// Moves `cid`'s start to `to` the way the editor does: the notes keep their place on the timeline.
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

/// Applies edit `kind` aimed at note `n` of clip `cid` (the one sounding at the edit) to `s`.
fn apply(kind: usize, s: &mut Session, t: TrackId, cid: ClipId, n: usize, at: i64, rng: &mut Rng) {
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
        other => panic!("{other} is not a session edit (at {at})"),
    }
}

/// One case: two clips of random notes on a few keys (overlaps on one key included), one random
/// edit (or an edit undone, or undone and redone) published at a block boundary while a note
/// sounds, then played past every clip's end. Returns what went wrong, if anything.
fn case(id: &str, block: usize, rng: &mut Rng) -> Result<(usize, Option<usize>), String> {
    const KEYS: [u8; 4] = [60, 62, 64, 67];
    let mut clips = Vec::new();
    for start in [0, 4800] {
        let notes = (0..5).map(|_| note(KEYS[rng.below(4) as usize], rng.range(0, 160), rng.range(8, 120))).collect();
        clips.push((start, 4800, notes));
    }
    let (s, t) = session(id, &clips);
    let ids = clip_ids(&s, t);
    let (c, n) = (rng.below(2) as usize, rng.below(5) as usize);
    let (cid, target) = (ids[c], clips[c].2[n]);
    // The edit goes in at the start of the block holding the middle of the target note.
    let on = clips[c].0 + target.start * TICK;
    let off = (on + target.length * TICK).min(clips[c].0 + 4800);
    let block_i = block as i64;
    let at = ((on + off) / 2 / block_i).max(1) * block_i;
    let kind = rng.below(EDITS.len() as u64) as usize;
    let mut edited = s.clone();
    let (first, later) = match EDITS[kind] {
        "undo" | "undo and redo" => {
            let k = rng.below(16) as usize;
            apply(k, &mut edited, t, cid, n, at, rng);
            (k, Some(kind))
        }
        _ => {
            apply(kind, &mut edited, t, cid, n, at, rng);
            (kind, None)
        }
    };
    // Each publish: when (block start) and which session.
    let step = if block_i >= 64 { block_i * rng.range(1, 4) } else { (rng.range(64, 1200) / block_i).max(1) * block_i };
    let mut publishes = vec![(at, edited.clone())];
    if later.is_some() {
        publishes.push((at + step, s.clone()));
    }
    if later == Some(17) {
        publishes.push((at + 2 * step, edited.clone()));
    }
    let what = format!(
        "{id}, block {block}, {} on clip {c} note {n} at {at}{}",
        EDITS[first],
        later.map_or(String::new(), |k| format!(" then {}", EDITS[k]))
    );
    let mut rig = Rig::new(block);
    rig.publish(&s);
    let mut current = s.clone();
    let mut pos = 0i64;
    loop {
        for (when, session) in &publishes {
            if *when == pos {
                rig.publish(session);
                current = session.clone();
            }
        }
        let end = current.track(t).unwrap().clips().iter().map(|c| c.end()).max().unwrap_or(0);
        let offset = current.edit.values.get(&format!("midi.offset.{}", t.0)).copied().unwrap_or(0.0).max(0.0) as i64;
        let last_publish = publishes.last().map_or(0, |p| p.0);
        if pos > end + offset + block_i && pos > last_publish {
            break;
        }
        rig.render(pos);
        pos += block_i;
    }
    check(&rig.notes()).map(|_| (first, later)).map_err(|e| format!("{what}: {e}"))
}

/// Every voice started once and released exactly once after its on; no off for a voice that is
/// not sounding; nothing left sounding. The number of note-ons on success.
fn check(notes: &[(u64, bool, u8, i32)]) -> Result<usize, String> {
    let mut open = std::collections::HashMap::new();
    let mut ons = 0;
    for &(f, on, key, voice) in notes {
        if on {
            if open.insert(voice, (f, key)).is_some() {
                return Err(format!("voice {voice} started again at {f} while it sounds"));
            }
            ons += 1;
        } else {
            match open.remove(&voice) {
                None => return Err(format!("off at {f} for key {key}, voice {voice}, which is not sounding")),
                Some((start, k)) if k != key || f < start => return Err(format!("off at {f} key {key} for voice {voice} (key {k}, on at {start})")),
                Some(_) => {}
            }
        }
    }
    if open.is_empty() { Ok(ons) } else { Err(format!("voices never released: {open:?}")) }
}

#[test]
fn every_note_on_gets_exactly_one_note_off_through_random_edits_mid_note() {
    let mut rng = Rng(0x0556_5EED_C0FF_EE01);
    let mut failures = Vec::new();
    let mut edits_seen = [0usize; EDITS.len()];
    for (block, cases) in [(1usize, 6usize), (7, 12), (64, 30), (512, 60)] {
        for id in INSTRUMENTS {
            for _ in 0..cases {
                let before = rng.0;
                match case(id, block, &mut rng) {
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
    }
    assert!(failures.is_empty(), "{} cases failed:\n{}", failures.len(), failures.join("\n"));
    assert!(edits_seen.iter().all(|&n| n > 0), "every edit kind ran: {edits_seen:?}");
}
