//! ADR 0059 R1 through an offline render (Decision 5's freeze and offline row): a clip of
//! overlapping notes, a same-key re-strike and pedal edges reaches a ledger-backed instrument at
//! block sizes 1, 7, 64 and 512. The instrument records what each call delivers, independently of
//! its audio output, so the partitions can be compared event for event: the same events at the
//! same frames in the same order, every note-on with its note-off, and the ledger empty at the end.
//! With VST3's separation (3b) the same holds, and no controller-borne event shares a frame with
//! another voice event.

use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use transit_dsp::event::{Message, PluginEvent};
use transit_dsp::plugins::instrument_queue::delivery::{Delivery, Item};
use transit_model::Insert;

type Log = Arc<Mutex<Vec<(u64, Item)>>>;

/// A contract-conforming instrument: admission through the shared ledger, each call settled.
struct Owned {
    inner: Box<dyn Plugin>,
    queue: Delivery,
    log: Log,
    at: u64,
    outstanding: Arc<AtomicUsize>,
}

impl Plugin for Owned {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.queue.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let n = u32::try_from(frames).unwrap_or(u32::MAX);
        self.queue.prepare(n);
        let mut log = self.log.lock().unwrap();
        for it in self.queue.batch(n) {
            log.push((self.at + u64::from(it.offset), *it));
        }
        drop(log);
        self.queue.finish(true);
        self.queue.carry();
        self.outstanding.store(self.queue.ledger().outstanding(), Ordering::Relaxed);
        self.at += frames as u64;
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        self.queue.admit(offset, ev)
    }
    fn all_notes_off(&mut self) {
        self.queue.all_notes_off();
    }
}

/// An instrument track with one clip: 25 frames a tick (960 per quarter, 120 BPM, 48 kHz).
fn session() -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.owned"));
    let mut seq = transit_midi::Sequence::default();
    let note = |pitch, start, length| transit_midi::Note { pitch, velocity: 100, release_velocity: 0, channel: 0, start, length };
    seq.notes = vec![note(60, 0, 4), note(62, 1, 2), note(60, 2, 4), note(64, 3, 2), note(65, 5, 1)];
    let cc = |tick, n, value| transit_midi::CtrlEvent { tick, channel: 0, kind: transit_midi::CtrlKind::Cc(n), value };
    seq.ctrls = vec![cc(1, 64, 127), cc(3, 66, 127), cc(4, 64, 0), cc(5, 66, 0), cc(6, 64, 127), cc(7, 64, 0)];
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(id, "notes", 0, 48_000, seq));
    s
}

/// Renders the clip at `block`: what the instrument received, and its ledger at the end.
fn render(separate: bool, block: usize) -> (Vec<(u64, Message, i32)>, usize) {
    let s = session();
    let log: Log = Arc::default();
    let outstanding = Arc::new(AtomicUsize::new(usize::MAX));
    let mut make = |spec: &InstanceSpec, _: SlotGen| {
        let mut inner = transit_dsp::create("gain")?;
        inner.prepare(48_000.0, block, spec.ch);
        let plugin = Owned { inner, queue: Delivery::new(separate), log: Arc::clone(&log), at: 0, outstanding: Arc::clone(&outstanding) };
        Some(NewInstance { plugin: Box::new(plugin), refused_state: false })
    };
    let mut r = RangeRender::build_with(&s, None, Range::new(0, 400), block, None, &mut make).unwrap();
    while r.next_block().is_some() {}
    drop(r);
    let got = log.lock().unwrap().iter().map(|(f, it)| (*f, it.event.message, it.event.note_id)).collect();
    (got, outstanding.load(Ordering::Relaxed))
}

#[test]
fn an_offline_render_delivers_the_same_events_at_every_partition() {
    for separate in [false, true] {
        let (want, left) = render(separate, 1);
        assert!(!want.is_empty());
        assert_eq!(left, 0, "the ledger is empty at the end (separate: {separate})");
        let ons: Vec<i32> = want.iter().filter(|e| matches!(e.1, Message::NoteOn { velocity, .. } if velocity > 0)).map(|e| e.2).collect();
        assert_eq!(ons.len(), 5);
        for id in &ons {
            let offs = want.iter().filter(|e| e.2 == *id && matches!(e.1, Message::NoteOff { .. })).count();
            assert_eq!(offs, 1, "note {id} has exactly one off (separate: {separate})");
        }
        if separate {
            // No controller-borne voice event shares its frame with another voice event.
            for (f, m, _) in &want {
                if matches!(m, Message::Control { cc: 64 | 66, .. }) {
                    let others = want.iter().filter(|e| e.0 == *f && !matches!(e.1, Message::Control { .. })).count();
                    assert_eq!(others, 0, "frame {f}: {want:?}");
                }
            }
        }
        for block in [7, 64, 512] {
            let (got, left) = render(separate, block);
            assert_eq!(got, want, "block {block}, separate: {separate}");
            assert_eq!(left, 0);
        }
    }
}
