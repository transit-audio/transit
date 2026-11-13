//! #81: Cycle toggled, or the cycle range edited, while playing reaches the running player at the
//! next block, without a restart. The real Player runs device-free: each step renders one block of
//! 512 frames on this thread, then runs the host loop's transport part as `logic` does.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{Services, StudioApp};
use serde_json::json;
use transit_engine::Engine;
use transit_time::Range;

/// Frames per `Player::step_device_free`.
const BLOCK: i64 = 512;

fn app() -> StudioApp {
    let engine = Engine::default();
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    steps(&mut app, 3);
    app
}

/// One audio block, then the host loop's transport part in `logic`'s order. The playhead after it.
fn step(app: &mut StudioApp) -> i64 {
    app.player.as_ref().unwrap().step_device_free();
    // `Player::idle` services the runtime (compiles the requested plan), as each frame does.
    app.apply_editor_edits();
    app.poll_player();
    app.handle_transport(0.0);
    if app.player_requests.play.is_none() && app.player_requests.locate.is_none() && app.player_requests.recording.is_none() {
        app.sync_player_session();
    }
    app.player.as_ref().unwrap().position()
}

fn steps(app: &mut StudioApp, n: usize) -> Vec<i64> {
    (0..n).map(|_| step(app)).collect()
}

fn region(app: &mut StudioApp, start: i64, end: i64) {
    app.run("transport.loop", json!({"start": start, "end": end})).unwrap();
}

fn cycle(app: &mut StudioApp, on: bool) {
    app.run("options.loop_playback", json!({"value": on})).unwrap();
}

/// Play from `at` and run until the transport has rendered a block; the playhead then.
fn play_from(app: &mut StudioApp, at: i64) -> i64 {
    app.set_cursor(at, None);
    app.run("transport.play", json!({})).unwrap();
    let mut last = 0;
    for _ in 0..10 {
        last = step(app);
        if app.is_playing() {
            break;
        }
    }
    assert!(app.is_playing(), "{}", app.status);
    last
}

/// Every block moves the playhead on by exactly one block, wrapping from `end` to `start` when it
/// gets there: no gap, no restart, no early jump.
fn assert_continuous(seen: &[i64], range: Option<Range>) {
    for w in seen.windows(2) {
        let next = w[0] + BLOCK;
        let want = match range {
            Some(r) if w[0] < r.end && next >= r.end => r.start + (next - r.end),
            _ => next,
        };
        assert_eq!(w[1], want, "after {} came {} (loop {range:?}): {seen:?}", w[0], w[1]);
    }
}

#[test]
fn cycle_on_inside_the_region_wraps_at_its_end_without_a_restart() {
    let mut app = app();
    let r = Range::new(4_096, 10_240);
    region(&mut app, r.start, r.end);
    let first = play_from(&mut app, 6_000);
    let mut seen = vec![first];
    seen.extend(steps(&mut app, 2));
    cycle(&mut app, true);
    seen.extend(steps(&mut app, 40));
    assert!(app.is_playing() && app.player_requests.play.is_none(), "no second Play");
    assert_continuous(&seen, Some(r));
    assert!(seen.windows(2).filter(|w| w[1] < w[0]).count() >= 2, "wrapped lap after lap: {seen:?}");
    assert!(seen.iter().all(|p| *p < r.end), "never ran past the region end: {seen:?}");
    assert!(!app.status.contains("audio engine"), "{}", app.status);
}

#[test]
fn cycle_on_inside_a_selection_with_no_region_loops_what_play_would() {
    // A session with Cycle and no loop region loops the selection at Play; while playing, the same.
    let mut app = app();
    let r = Range::new(2_048, 8_192);
    play_from(&mut app, 3_000);
    app.engine.session_mut().edit.selection = r;
    let mut seen = steps(&mut app, 1);
    app.engine.session_mut().edit.loop_playback = true;
    seen.extend(steps(&mut app, 30));
    assert_continuous(&seen, Some(r));
    assert!(seen.windows(2).any(|w| w[1] < w[0]), "{seen:?}");
}

#[test]
fn cycle_on_ahead_of_the_region_plays_on_and_wraps_when_it_gets_there() {
    let mut app = app();
    let r = Range::new(8_192, 12_288);
    region(&mut app, r.start, r.end);
    let mut seen = vec![play_from(&mut app, 0)];
    cycle(&mut app, true);
    seen.extend(steps(&mut app, 40));
    assert_continuous(&seen, Some(r));
    let wrap = seen.windows(2).position(|w| w[1] < w[0]).expect("wrapped");
    assert!(seen[wrap] >= r.end - BLOCK, "no early jump into the region: {seen:?}");
    assert!(seen[wrap + 1..].iter().all(|p| (r.start..r.end).contains(p)), "{seen:?}");
}

#[test]
fn cycle_on_past_the_region_plays_on() {
    let mut app = app();
    let r = Range::new(2_048, 4_096);
    region(&mut app, r.start, r.end);
    let mut seen = vec![play_from(&mut app, 10_000)];
    cycle(&mut app, true);
    seen.extend(steps(&mut app, 20));
    assert_continuous(&seen, None);
}

#[test]
fn cycle_off_mid_loop_plays_through() {
    let mut app = app();
    let r = Range::new(4_096, 8_192);
    region(&mut app, r.start, r.end);
    cycle(&mut app, true);
    let mut seen = vec![play_from(&mut app, 0)];
    assert!((r.start..r.end).contains(&seen[0]), "Play with Cycle on starts at the region: {seen:?}");
    seen.extend(steps(&mut app, 12));
    assert!(seen.windows(2).any(|w| w[1] < w[0]), "looping before Cycle goes off: {seen:?}");
    let mid = seen.len() - 1;
    cycle(&mut app, false);
    seen.extend(steps(&mut app, 20));
    assert_continuous(&seen[..=mid], Some(r));
    assert_continuous(&seen[mid..], None);
    assert!(*seen.last().unwrap() > r.end + 4 * BLOCK, "{seen:?}");
}

#[test]
fn moving_the_region_edges_while_looping_moves_the_wrap() {
    let mut app = app();
    let r = Range::new(4_096, 16_384);
    region(&mut app, r.start, r.end);
    cycle(&mut app, true);
    let mut seen = vec![play_from(&mut app, 0)];
    seen.extend(steps(&mut app, 2));
    // The end comes in ahead of the playhead: the wrap moves there.
    let shorter = Range::new(4_096, 10_240);
    region(&mut app, shorter.start, shorter.end);
    let at = seen.len() - 1;
    seen.extend(steps(&mut app, 30));
    assert_continuous(&seen[at..], Some(shorter));
    assert!(seen[at..].iter().all(|p| *p < shorter.end), "{seen:?}");
    // The start moves and the end goes out again: the next wrap lands on the new start.
    let moved = Range::new(6_144, 14_336);
    region(&mut app, moved.start, moved.end);
    let at = seen.len() - 1;
    seen.extend(steps(&mut app, 40));
    assert_continuous(&seen[at..], Some(moved));
    assert!(seen[at..].iter().any(|p| *p >= shorter.end), "the end went out: {seen:?}");
    assert!(seen[at + 30..].iter().all(|p| (moved.start..moved.end).contains(p)), "{seen:?}");
}

#[test]
fn cycle_set_before_play_behaves_as_before() {
    let mut app = app();
    let r = Range::new(4_096, 8_192);
    region(&mut app, r.start, r.end);
    app.set_cursor(1_000, None);
    cycle(&mut app, true);
    cycle(&mut app, false);
    cycle(&mut app, true);
    let idle = steps(&mut app, 4);
    assert!(!app.is_playing(), "toggling while stopped starts nothing");
    assert!(idle.iter().all(|p| *p == idle[0]), "{idle:?}");
    let mut seen = vec![play_from(&mut app, 1_000)];
    seen.extend(steps(&mut app, 20));
    assert_eq!(seen[0], r.start + BLOCK, "Play with Cycle on starts at the region start");
    assert_continuous(&seen, Some(r));
    assert!(seen.windows(2).any(|w| w[1] < w[0]), "{seen:?}");
}

#[test]
fn cycle_toggled_while_a_take_records_keeps_its_loop_until_the_take_ends() {
    let mut app = app();
    let r = Range::new(2_048, 6_144);
    region(&mut app, r.start, r.end);
    let mut seen = vec![play_from(&mut app, 0)];
    // A pass under way (punched in on the fly, so the player's loop is the one it has: none).
    app.take_cycle = app.wanted_loop();
    app.take_plan = Some(transit_engine::io::Recording {
        start: seen[0],
        pass: None,
        punch: None,
        rate: 48_000,
        takes: Vec::new(),
        midi_journal: None,
        midi_placed: false,
        cycle: None,
        end: None,
    });
    cycle(&mut app, true);
    seen.extend(steps(&mut app, 20));
    assert_continuous(&seen, None);
    assert_eq!(app.take_cycle, None, "the take is cut by the loop it started with");
    // The take ends with the transport still running: the change lands then.
    app.take_plan = None;
    let at = seen.len() - 1;
    seen.extend(steps(&mut app, 4));
    assert_continuous(&seen[at..], None);
    let past = *seen.last().unwrap();
    assert!(past > r.end, "past the region: it plays on");
    app.locate_playing(r.start);
    let looped = steps(&mut app, 20);
    assert!(looped.windows(2).any(|w| w[1] < w[0]), "Cycle is now in force: {looped:?}");
    assert!(looped.iter().all(|p| (r.start..r.end).contains(p)), "{looped:?}");
}

/// #522: a cycle recording through the device-free player, a MIDI track armed. Blocks render on
/// their own between host frames, as the audio thread runs on while the UI thread is busy.
mod lap_clock {
    use super::*;
    use transit_model::TrackId;

    /// A loop of 0.8 s at 48 kHz: 75 blocks.
    const LOOP: i64 = 38_400;
    const BLOCKS_PER_LAP: usize = 75;

    /// One host frame's recording part, in `logic`'s order: follow the pass, then the limit.
    fn frame(app: &mut StudioApp) {
        app.update_midi_live();
        app.end_recording_at_lap_limit();
    }

    /// Blocks with a host frame after each until `done`, at most `limit` of them: a clock that
    /// stopped counting fails here instead of running on.
    fn run_until(app: &mut StudioApp, limit: usize, done: impl Fn(&StudioApp) -> bool) {
        for _ in 0..limit {
            if done(app) {
                return;
            }
            step(app);
            frame(app);
        }
        assert!(done(app), "not reached in {limit} blocks: {}", app.status);
    }

    /// Render `n` blocks with no host frame between them (the UI thread stalled).
    fn stall(app: &mut StudioApp, n: usize) {
        for _ in 0..n {
            app.player.as_ref().unwrap().step_device_free();
        }
    }

    /// The laps the host counts as complete for the pass under way.
    fn completed(app: &StudioApp) -> i64 {
        let (Some(plan), Some(c)) = (&app.take_plan, app.record_clock) else { return -1 };
        let wrap = app.take_cycle.filter(|_| plan.pass.is_none());
        transit_engine::take_laps::completed(c.start, plan.pass, wrap, c.reach - c.start)
    }

    /// A session with an armed MIDI track and Cycle over `[LOOP, 2 * LOOP)`, the limit `limit`;
    /// its scratch folder stays alive with the app.
    fn armed(limit: i64) -> (StudioApp, transit_test_support::TestDir) {
        let dir = transit_test_support::TestDir::new("lap-clock");
        let mut app = app();
        app.autosave_dir = Some(dir.join("Autosave"));
        let t = TrackId(app.run("track.new", json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap());
        app.run("mix.record_arm", json!({"tracks": [t.0], "value": true})).unwrap();
        region(&mut app, LOOP, 2 * LOOP);
        cycle(&mut app, true);
        app.test_lap_limit = Some(limit);
        (app, dir)
    }

    /// Run blocks with a host frame after each until the pass has latched and is playing.
    fn until_latched(app: &mut StudioApp) {
        for _ in 0..200 {
            step(app);
            frame(app);
            if app.record_clock.is_some() {
                return;
            }
        }
        panic!("the pass never latched: {}", app.status);
    }

    /// Record from stopped (the loop start), then a two-second stall right after the fourth wrap:
    /// 2.5 laps go by unseen and the playhead comes back ahead of where it was, so no frame sees
    /// it go back. The host still counts six laps and ends the recording at its next frame, the
    /// first check that sees the limit of five.
    #[test]
    fn a_two_second_stall_past_the_limit_ends_the_recording_at_the_next_frame() {
        let (mut app, _dir) = armed(5);
        app.start_recording();
        until_latched(&mut app);
        let latched = app.record_clock.unwrap();
        // To just past the fourth wrap, a frame after each block.
        run_until(&mut app, 400, |a| completed(a) >= 4);
        assert!(app.recording_active(), "{}", app.status);
        let before = app.player.as_ref().unwrap().position();
        stall(&mut app, 188);
        assert!(app.player.as_ref().unwrap().position() > before, "the playhead came back ahead: no backward step to see");
        frame(&mut app);
        // The pass is over at once (the player's own recording flag clears when its stop notice is
        // polled, a frame later).
        assert!(app.take_plan.is_none() && app.record_clock.is_none(), "the first frame after the stall ended the recording: {}", app.status);
        assert!(app.status.ends_with("Cycle recording ended at its limit of 5 laps; press Record to continue."), "{}", app.status);
        assert_eq!(latched.start, LOOP, "the pass latched at the loop start");
    }

    /// Three whole laps between two host frames: the playhead is where it was, yet the host
    /// counts the three laps.
    #[test]
    fn laps_between_frames_that_see_the_same_position_are_counted() {
        let (mut app, _dir) = armed(10);
        app.start_recording();
        until_latched(&mut app);
        run_until(&mut app, 200, |a| completed(a) >= 1);
        let (laps, at) = (completed(&app), app.player.as_ref().unwrap().position());
        stall(&mut app, 3 * BLOCKS_PER_LAP);
        assert_eq!(app.player.as_ref().unwrap().position(), at, "the same position three laps later");
        frame(&mut app);
        assert_eq!(completed(&app), laps + 3);
        assert!(app.recording_active(), "below the limit it records on");
    }

    /// Record pressed while the loop plays, halfway in (#567): the first lap runs from the latch
    /// to the loop end, so it is complete at the first wrap, and every later lap one loop on.
    #[test]
    fn a_pass_latched_mid_loop_completes_its_first_lap_at_the_first_wrap() {
        let (mut app, _dir) = armed(10);
        play_from(&mut app, LOOP);
        run_until(&mut app, 100, |a| a.player.as_ref().unwrap().position() >= LOOP + LOOP / 2);
        app.start_recording();
        until_latched(&mut app);
        let c = app.record_clock.unwrap();
        assert!(c.start > LOOP && c.start < 2 * LOOP, "latched inside the loop at {}", c.start);
        assert_eq!(app.take_plan.as_ref().unwrap().pass, None, "cut at each wrap");
        let first = 2 * LOOP - c.start;
        // Blocks until just before the first wrap, then across it, then one loop more.
        run_until(&mut app, 100, |a| a.record_clock.is_some_and(|r| r.reach - c.start >= first - 512));
        assert_eq!(completed(&app), 0);
        stall(&mut app, 2);
        frame(&mut app);
        assert_eq!(completed(&app), 1, "the first lap ends at the first wrap");
        stall(&mut app, BLOCKS_PER_LAP);
        frame(&mut app);
        assert_eq!(completed(&app), 2);
    }
}
