//! ADR 0057 slice P2 (#559, third item): an event sent between blocks runs through each effect in a
//! call in which no time passes, at block sizes 1, 7, 64 and 512.
use super::*;

const SR: f64 = 48_000.0;

/// What the instrument was given: (timeline frame, given between blocks, event).
type Heard = Vec<(i64, bool, PluginEvent)>;

/// A host on a playing timeline at 120 BPM, its render clock running on through locates.
struct Player {
    host: Box<MidiFxHost>,
    rec: Rec,
    render: u64,
    heard: Heard,
}

impl Player {
    fn new() -> Self {
        Player { host: MidiFxHost::new(), rec: Rec::default(), render: 0, heard: Vec::new() }
    }

    /// One block of `frames` from timeline frame `start`, with `events` (offset, event).
    fn block(&mut self, t: &Track, start: i64, frames: usize, events: &[(u32, PluginEvent)]) {
        let transport = Transport {
            playing: true,
            tempo: 120.0,
            numerator: 4,
            denominator: 4,
            samples: start,
            seconds: start as f64 / SR,
            quarters: start as f64 * 2.0 / SR,
            ..Transport::default()
        };
        let from = self.rec.heard.len();
        self.host.begin(&mut self.rec, t, None, start, &transport, SR, self.render, frames);
        for (o, e) in events {
            assert!(self.host.take(&mut self.rec, *o, e));
        }
        self.host.flush(&mut self.rec);
        self.rec.process(&mut [], frames);
        self.render += frames as u64;
        self.heard.extend(self.rec.heard[from..].iter().map(|(_, o, e)| (start + i64::from(*o), false, *e)));
    }

    /// `e` sent between blocks, the next block starting at timeline frame `next`.
    fn between(&mut self, next: i64, e: &PluginEvent) {
        let from = self.rec.heard.len();
        assert!(self.host.take(&mut self.rec, 0, e));
        for (_, o, e) in &self.rec.heard[from..] {
            assert_eq!(*o, 0);
            self.heard.push((next, true, *e));
        }
    }
}

/// The fixture generator steps every 448 frames of its render clock. A pedal-up sent between
/// blocks at frame 3584 (eight steps, and a block boundary at every size compared) reaches the
/// instrument at once and changes nothing else: the same steps on the same frames as without it.
/// Run through `process` over one frame, as before P2, it would take the step at 3584 between
/// blocks and every later step one frame early.
#[test]
fn an_event_between_blocks_moves_no_clock_and_starts_no_step() {
    const AT: i64 = 3584;
    let t = track(&[("midi_test_generator", &[("period", 448.0)])]);
    let pedal_up = ev(PluginEvent::NO_ID, Message::Control { cc: 64, value: 0 });
    let play = |frames: usize, send: bool| {
        let mut p = Player::new();
        let mut at = 0;
        while at < 4600 {
            if send && at == AT {
                p.between(at, &pedal_up);
            }
            p.block(&t, at, frames, &[]);
            at += frames as i64;
        }
        p.heard
    };
    let mut steps: Heard = Vec::new();
    for k in 0..=10 {
        if k > 0 {
            steps.push((k * 448, false, off(PluginEvent::NO_ID, 60)));
        }
        steps.push((k * 448, false, on(PluginEvent::NO_ID, 60, 100)));
    }
    let mut sent = steps.clone();
    let i = sent.iter().position(|h| h.0 == AT).unwrap();
    sent.insert(i, (AT, true, pedal_up));
    for frames in [1, 7, 64, 512] {
        assert_eq!(play(frames, false), steps, "{frames}-frame blocks, nothing between blocks");
        assert_eq!(play(frames, true), sent, "{frames}-frame blocks, a pedal-up between blocks");
    }
}

/// #559, minimal case 3: the Random Sequencer with Thru on and its clock voice (a note every
/// sixteenth, 6000 frames at 120 BPM), a clip note sounding through it, the block before a locate
/// starting on a tick (12,000), and the locate releasing the clip note between blocks. Only that
/// release passes between blocks, and the sequencer plays exactly what it plays when nothing is
/// released there: no note-on and note-off at the locate that the sequence does not contain.
#[test]
fn the_random_sequencer_at_a_locate_writes_no_stray_note() {
    let t = track(&[("random_sequencer", &[("thru", 1.0), ("play", 3.0)])]);
    let play = |frames: usize, clip: bool| {
        let mut p = Player::new();
        let first = 12_000 - 2 * frames as i64;
        let mut at = first;
        while at <= 12_000 {
            let input: &[(u32, PluginEvent)] = if clip && at == first { &[(0, on(1, 40, 90))] } else { &[] };
            p.block(&t, at, frames, input);
            at += frames as i64;
        }
        if clip {
            p.between(96_000, &off(1, 40));
        }
        let mut at = 96_000;
        while at < 102_512 {
            p.block(&t, at, frames, &[]);
            at += frames as i64;
        }
        p.heard
    };
    let generated = |h: &Heard| h.iter().filter(|h| h.2.note_id == PluginEvent::NO_ID).copied().collect::<Vec<_>>();
    for frames in [1, 7, 64, 512] {
        let (with, without) = (play(frames, true), play(frames, false));
        let between: Vec<_> = with.iter().filter(|h| h.1).map(|h| h.2).collect();
        assert_eq!(between, [off(1, 40)], "{frames}-frame blocks: only the release passes between blocks");
        assert_eq!(generated(&with), generated(&without), "{frames}-frame blocks: the locate's release changes nothing the sequencer plays");
        let ons: Vec<i64> = generated(&without).iter().filter(|h| matches!(h.2.message, Message::NoteOn { .. })).map(|h| h.0).collect();
        assert_eq!(ons, [12_000, 96_000, 102_000], "{frames}-frame blocks: the clock voice's sixteenths");
    }
}
