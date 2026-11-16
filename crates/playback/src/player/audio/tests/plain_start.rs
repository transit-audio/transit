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
    // Every case runs before the assertion, so a failure lists each start kind and rate it hits.
    let mut failures = Vec::new();
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
            if across > normal * 1.001 {
                failures.push(format!("{kind:?} at {rate} on a callback of {frames}: a step of {across} at the start against {normal}"));
            }
            control.retire(audio.plans, audio.actions);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
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
    // Every case runs before the assertion; each records its first failure.
    let mut failures = Vec::new();
    for kind in [Start::Play, Start::Record] {
        for (rate, sizes) in [(44_100u32, &[37usize, 128, 59, 441][..]), (96_000, &[512, 333, 700][..])] {
            let mut failure = None;
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
                let shown = shared.position.load(Ordering::Relaxed);
                if shown != expected && failure.is_none() {
                    failure = Some(format!("{kind:?} at {rate}: the position {} frames after the latch is {shown}, not {expected}", next - latch));
                }
            }
            if kind == Start::Record {
                let start::Status::Started(point) = reader.status() else { panic!("{rate}: Record did not start: {:?}", reader.status()) };
                if point.output_frame != latch || (point.position.sample, point.position.fraction) != (0, 0) {
                    failure.get_or_insert(format!("{kind:?} at {rate}: latched {point:?}, not output frame {latch} at position 0"));
                }
            }
            let first = peak(&heard, (latch - base) as usize);
            if first != reference {
                failure.get_or_insert(format!("{kind:?} at {rate}: the first click peaks {first} frames after the latch, not {reference}"));
            }
            failures.extend(failure);
            control.retire(audio.plans, audio.actions);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
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

/// #725 review: a start at speed 1 leaves equal rates passing samples through exactly. A transport
/// end at half speed leaves the converter at a fractional phase (the host returns to speed 1 and
/// sends no Stop); Play or Record from there must still give the source bit for bit from the frame
/// that presents the start, as a start from a whole phase does.
#[test]
fn a_start_at_speed_one_after_a_half_speed_end_passes_samples_through_at_equal_rates() {
    const FROM: Samples = 17;
    const FRAMES: usize = 2_000;
    let source = signal();
    // A start from a whole phase: the converter passes every sample through.
    let reference = {
        let (mut control, mut audio, _shared, _start, _reader) = stopped(48_000, (*source).clone());
        for _ in 0..20 {
            callback(&mut control, &mut audio, 48_000, 333, &mut Vec::new());
        }
        assert_eq!(audio.read.ahead_after(audio.pending, STEP), 0);
        control.send_action(Command::Play { from: FROM, end: None, looped: None }, true).unwrap();
        let mut heard = Vec::new();
        while heard.len() < FRAMES {
            callback(&mut control, &mut audio, 48_000, 256, &mut heard);
        }
        control.retire(audio.plans, audio.actions);
        heard
    };
    assert!(reference.iter().any(|v| *v != 0.0));
    for kind in [Start::Play, Start::Record] {
        let (mut control, mut audio, shared, _start, reader) = stopped(48_000, (*source).clone());
        shared.speed.store(500, Ordering::Relaxed);
        control.send_action(Command::Play { from: 0, end: Some(1_001), looped: None }, true).unwrap();
        let mut calls = 0;
        while audio.transport.playing || calls < 3 {
            callback(&mut control, &mut audio, 48_000, 333, &mut Vec::new());
            calls += 1;
            assert!(calls < 100, "the half-speed pass never ended");
        }
        if audio.read.position(STEP / 2).0.fract() == 0.0 {
            callback(&mut control, &mut audio, 48_000, 1, &mut Vec::new());
        }
        shared.speed.store(1_000, Ordering::Relaxed);
        callback(&mut control, &mut audio, 48_000, 333, &mut Vec::new());
        assert_ne!(audio.read.position(STEP).0.fract(), 0.0, "{kind:?}: the half-speed end left a fractional phase");
        let base = audio.transport.timeline.next_output();
        let latch = base + audio.read.ahead_after(audio.pending, STEP);
        send(&mut control, kind, &reader, FROM);
        let mut heard = Vec::new();
        while heard.len() < (latch - base) as usize + FRAMES {
            callback(&mut control, &mut audio, 48_000, 256, &mut heard);
        }
        if kind == Start::Record {
            let start::Status::Started(point) = reader.status() else { panic!("Record did not start: {:?}", reader.status()) };
            assert_eq!(point.output_frame, latch);
        }
        let played = &heard[(latch - base) as usize..][..FRAMES];
        let first = played.iter().zip(&reference).position(|(a, b)| a.to_bits() != b.to_bits());
        assert_eq!(first, None, "{kind:?}: the first frame after the start that is not the source sample");
        control.retire(audio.plans, audio.actions);
    }
}

/// #725 review: at equal rates a start has no lead, so the converter running on gives exactly what
/// the old restart gave. Play and Record from a stop, then Play from another position while
/// playing, against the same run with the converter restarted before each start's callback: every
/// sample bit-identical, the lead 0 at every callback and no hold.
#[test]
fn equal_rate_starts_are_bit_identical_to_a_converter_restart() {
    let mut session = Session::default();
    session.edit.click = true;
    let track = session.add_track(transit_model::TrackKind::Audio, transit_model::ChannelFormat::Stereo, None);
    session.track_mut(track).unwrap().mixer.inserts.push(Some(transit_model::Insert::new("signal_generator")));
    for kind in [Start::Play, Start::Record] {
        for sizes in [&[480usize][..], &[700], &[37, 128, 700, 333, 1, 513]] {
            let run = |restart: bool| {
                let (mut control, mut audio, _shared, _start, reader) = stopped(48_000, session.clone());
                let mut heard = Vec::new();
                let mut frames = sizes.iter().cycle();
                for n in 0..120 {
                    if n == 40 || n == 80 {
                        assert_eq!(audio.read.ahead_after(audio.pending, STEP), 0, "the lead at equal rates");
                        // The second start is a Play from another position while playing.
                        let (kind, from) = if n == 40 { (kind, 0) } else { (Start::Play, 3_000) };
                        send(&mut control, kind, &reader, from);
                        if restart {
                            audio.restart_converter();
                        }
                    }
                    callback(&mut control, &mut audio, 48_000, *frames.next().unwrap(), &mut heard);
                    assert_eq!(audio.read.ahead_after(audio.pending, STEP), 0, "the lead at equal rates after a callback");
                    assert!(audio.transport.hold.is_none(), "no hold at equal rates");
                }
                assert!(audio.transport.playing);
                if kind == Start::Record {
                    assert!(matches!(reader.status(), start::Status::Started(_)), "{:?}", reader.status());
                }
                control.retire(audio.plans, audio.actions);
                heard
            };
            let (running_on, restarted) = (run(false), run(true));
            assert!(running_on.iter().any(|v| *v != 0.0));
            let first = running_on.iter().zip(&restarted).position(|(a, b)| a.to_bits() != b.to_bits());
            assert_eq!(first, None, "{kind:?} on callbacks of {sizes:?}: the first frame that differs from a restart");
        }
    }
}

/// #725 review: a Play started within the converter's reach of a loop end wraps while it still
/// leads in. The playhead holds the start position across that wrap until the frame that presents
/// the start; a wrap used to end the hold early and show the lead's positions, a step back.
#[test]
fn the_hold_lasts_across_a_loop_wrap_inside_the_lead() {
    const FROM: Samples = 995;
    for rate in [44_100u32, 96_000] {
        let (mut control, mut audio, shared, _start, _reader) = stopped(rate, Session::default());
        for _ in 0..20 {
            callback(&mut control, &mut audio, rate, 128, &mut Vec::new());
        }
        let latch = audio.transport.timeline.next_output() + audio.read.ahead_after(audio.pending, STEP);
        control.send_action(Command::Play { from: FROM, end: None, looped: Some(Range { start: 0, end: 1_000 }) }, true).unwrap();
        let mut wrapped_in_the_lead = false;
        loop {
            callback(&mut control, &mut audio, rate, 1, &mut Vec::new());
            let next = audio.transport.timeline.next_output();
            if next > latch {
                break;
            }
            wrapped_in_the_lead |= audio.transport.render_position < FROM;
            let shown = shared.position.load(Ordering::Relaxed);
            assert_eq!(shown, FROM, "{rate}: shown {} output frames before the latch", latch - next);
        }
        assert!(wrapped_in_the_lead, "{rate}: the render wrapped before the latch");
        control.retire(audio.plans, audio.actions);
    }
}
