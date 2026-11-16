//! #725: Play and Record from a stop run the output converter on. The timeline reaches the start
//! on the output frame that presents the first block rendered for it, the playhead shows the start
//! until then, and what was already sounding stays continuous.
use super::count_in::track_clock;
use super::*;

/// The render step at speed 1 (the plan runs at 48 kHz).
const STEP: u64 = 48_000 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    Play,
    Record,
}

/// A clocked recorder waiting for a ticket, for an audio pass.
fn recorder(audio: &mut Audio, rate: u32) -> (start::Control, start::Reader) {
    let (mut start, publisher) = start::pair(transit_mix::EngineEpoch(1));
    audio.transport.record_start = publisher;
    recording_clock(audio, rate);
    let reader = start.prepare().unwrap();
    reader.mark_audio();
    (start, reader)
}

/// Start `kind` from `from`: Play, or Record without a count-in.
fn send(control: &mut Control<Command, ActionResult>, kind: Start, reader: &start::Reader, from: Samples) {
    let command = match kind {
        Start::Play => Command::Play { from, end: None, looped: None },
        Start::Record => Command::Record { ticket: reader.ticket(), from: Some(from), end: None, looped: None, count_in: None },
    };
    control.send_action(command, true).unwrap();
}

/// One callback of `frames`, with the clock kept current; the left channel heard.
fn callback(control: &mut Control<Command, ActionResult>, audio: &mut Audio, rate: u32, frames: usize, heard: &mut Vec<f32>) {
    let now = audio.transport.timeline.next_output();
    track_clock(audio, rate, now);
    let mut out = vec![0.0f32; frames * 2];
    fill(audio, &mut out);
    control.service(&mut make);
    heard.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
}

/// The fixture after its first callback, with a recorder waiting.
fn stopped(rate: u32, session: Session) -> (Control<Command, ActionResult>, Audio, Arc<Shared>, start::Control, start::Reader) {
    let (mut control, mut audio, shared) = fixture_session(rate, Arc::new(session));
    fill(&mut audio, &mut [0.0; 8]);
    control.service(&mut make);
    let (start, reader) = recorder(&mut audio, rate);
    (control, audio, shared, start, reader)
}

/// The largest move from one output frame to the next over `range`.
fn largest_step(heard: &[f32], range: std::ops::Range<usize>) -> f32 {
    heard[range].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max)
}

/// A steady 1 kHz sine from a track's Signal Generator sounds while the transport is stopped and
/// on through Play or Record from a stop: from one output frame to the next around the start it
/// moves by no more than it does anywhere before it. The start used to restart the converter,
/// which skipped the frames it still had to present and cleared its history.
#[test]
fn a_tone_sounding_across_a_plain_start_has_no_step_at_any_rate() {
    let mut session = Session::default();
    session.edit.click = false;
    let track = session.add_track(transit_model::TrackKind::Audio, transit_model::ChannelFormat::Stereo, None);
    session.track_mut(track).unwrap().mixer.inserts.push(Some(transit_model::Insert::new("signal_generator")));
    for kind in [Start::Play, Start::Record] {
        for (rate, frames) in
            [(44_100u32, 441usize), (44_100, 128), (44_100, 333), (96_000, 512), (96_000, 700), (96_000, 333), (48_000, 480), (48_000, 700)]
        {
            let (mut control, mut audio, shared, _start, reader) = stopped(rate, session.clone());
            let first = audio.transport.timeline.next_output();
            let mut heard = Vec::new();
            let mut at = None;
            while heard.len() < rate as usize {
                if at.is_none() && heard.len() >= rate as usize / 2 {
                    send(&mut control, kind, &reader, 0);
                    at = Some((audio.transport.timeline.next_output() - first) as usize);
                }
                callback(&mut control, &mut audio, rate, frames, &mut heard);
            }
            let at = at.unwrap();
            assert!(shared.playing.load(Ordering::Relaxed), "{kind:?} at {rate}: playing");
            if kind == Start::Record {
                assert!(matches!(reader.status(), start::Status::Started(_)), "{kind:?} at {rate}: {:?}", reader.status());
            }
            // Steady: from a quarter of a second in (the level has settled) to well before the start.
            let normal = largest_step(&heard, rate as usize / 4..at - rate as usize / 10);
            let across = largest_step(&heard, at - 200..at + 400);
            assert!(normal > 0.005, "{kind:?} at {rate}: the tone sounds while stopped ({normal})");
            assert!(across <= normal * 1.001, "{kind:?} at {rate} on a callback of {frames}: a step of {across} at the start against {normal}");
            control.retire(audio.plans, audio.actions);
        }
    }
}

/// The start position is presented on the output frame where the timeline reaches it, and a
/// Record start latches that frame: the first click of the played bar has the same shape against
/// that frame as a click played from the first callback, when the converter had rendered nothing
/// and render frame 0 is the first output frame. The start is taken on a callback boundary where
/// the render frame after the pending block falls on a whole output frame, so the two clicks are
/// read at the same converter phases.
#[test]
fn a_plain_start_is_presented_on_the_frame_its_timeline_reaches_it() {
    let peak = |out: &[f32], from: usize| {
        let window = &out[from..from + 600];
        window.iter().enumerate().fold((0, 0.0f32), |best, (k, v)| if v.abs() > best.1 { (k, v.abs()) } else { best }).0
    };
    let mut session = Session::default();
    session.edit.click = true;
    for kind in [Start::Play, Start::Record] {
        for (rate, sizes) in [(44_100u32, &[37usize, 128, 59, 441][..]), (96_000, &[512, 333, 700][..])] {
            let reference = {
                let (mut control, mut audio, _) = fixture_session(rate, Arc::new(session.clone()));
                control.send_action(Command::Play { from: 0, end: None, looped: None }, true).unwrap();
                let mut heard = Vec::new();
                while heard.len() < 2_000 {
                    callback(&mut control, &mut audio, rate, 128, &mut heard);
                }
                control.retire(audio.plans, audio.actions);
                peak(&heard, 0)
            };
            let (mut control, mut audio, shared, _start, reader) = stopped(rate, session.clone());
            let mut calls = 0;
            for &frames in sizes.iter().cycle() {
                callback(&mut control, &mut audio, rate, frames, &mut Vec::new());
                calls += 1;
                if calls > 40 && audio.read.ahead_after_is_whole(audio.pending, STEP) && audio.read.ahead_after(audio.pending, STEP) > 0 {
                    break;
                }
                assert!(calls < 20_000, "{rate}: no callback boundary at a whole lead");
            }
            let base = audio.transport.timeline.next_output();
            let latch = base + audio.read.ahead_after(audio.pending, STEP);
            send(&mut control, kind, &reader, 0);
            let mut heard = Vec::new();
            while heard.len() < rate as usize / 10 {
                callback(&mut control, &mut audio, rate, 128, &mut heard);
                // From the latch on, the timeline runs from the start position.
                let next = audio.transport.timeline.next_output();
                let expected = ((next - latch) * 48_000 / u64::from(rate)) as i64;
                assert_eq!(
                    shared.position.load(Ordering::Relaxed),
                    expected,
                    "{kind:?} at {rate}: the position {} frames after the latch",
                    next - latch
                );
            }
            if kind == Start::Record {
                let start::Status::Started(point) = reader.status() else { panic!("{rate}: Record did not start: {:?}", reader.status()) };
                assert_eq!(point.output_frame, latch, "{rate}: the latch is the frame presenting the start");
                assert_eq!((point.position.sample, point.position.fraction), (0, 0), "{rate}: the latch is at the start position");
            }
            assert_eq!(peak(&heard, (latch - base) as usize), reference, "{kind:?} at {rate}: the first click against the latch");
            control.retire(audio.plans, audio.actions);
        }
    }
}

/// While a start leads in at a converted rate, the playhead shows the start position until the
/// output frame that presents it, then moves on from it: it never steps back. At a count-in's end
/// the frames before the latch are the count-in's tail and the timeline already reads them before
/// the start; a plain Play is the same. Callbacks of 4 frames around the start see every frame of
/// the lead.
#[test]
fn the_playhead_holds_the_start_position_until_the_frame_that_presents_it() {
    const FROM: Samples = 1_000;
    for counted in [true, false] {
        for rate in [44_100u32, 96_000] {
            let (mut control, mut audio, shared, _start, reader) = stopped(rate, Session::default());
            for _ in 0..20 {
                callback(&mut control, &mut audio, rate, 128, &mut Vec::new());
            }
            let mut latch = None;
            if counted {
                let count_in = CountIn { frames: 96_000, beat: 24_000, per_bar: 4 };
                let command = Command::Record { ticket: reader.ticket(), from: Some(FROM), end: None, looped: None, count_in: Some(count_in) };
                control.send_action(command, true).unwrap();
            } else {
                latch = Some(audio.transport.timeline.next_output() + audio.read.ahead_after(audio.pending, STEP));
                send(&mut control, Start::Play, &reader, FROM);
            }
            let (mut shown, mut lead) = (Vec::new(), 0);
            loop {
                let frames = if shared.metro.count_in_left.load(Ordering::Relaxed) > 4_000 { 441 } else { 4 };
                callback(&mut control, &mut audio, rate, frames, &mut Vec::new());
                if let start::Status::Started(point) = reader.status() {
                    latch.get_or_insert(point.output_frame);
                }
                let next = audio.transport.timeline.next_output();
                let position = shared.position.load(Ordering::Relaxed);
                if audio.transport.playing {
                    shown.push((next, position));
                    // The frames before the latch the timeline reads ahead of the start.
                    if let Some(latch) = latch
                        && next < latch
                    {
                        lead += 1;
                        assert!(audio.transport.timeline.position().unwrap().sample < FROM);
                    }
                }
                if latch.is_some_and(|latch| next > latch + 256) {
                    break;
                }
            }
            let latch = latch.unwrap();
            assert!(lead > 0, "counted {counted} at {rate}: the lead was seen");
            for &(next, position) in &shown {
                if next <= latch {
                    assert_eq!(position, FROM, "counted {counted} at {rate}: shown {} output frames before the latch", latch - next);
                }
            }
            assert!(shown.windows(2).all(|w| w[1].1 >= w[0].1), "counted {counted} at {rate}: the playhead stepped back: {shown:?}");
            assert!(shown.last().unwrap().1 > FROM + 64, "counted {counted} at {rate}: moved on");
            control.retire(audio.plans, audio.actions);
        }
    }
}
