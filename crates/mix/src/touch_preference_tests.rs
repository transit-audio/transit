use super::*;

fn long_session() -> (Session, TrackId) {
    // Cover the held block, the longest return, and the trailing on-lane assertions.
    let (mut s, t) = session_with_dc(0.5, 96_000);
    let tr = s.track_mut(t).unwrap();
    tr.mixer.automation_mode = AutomationMode::Touch;
    tr.mixer.volume_db = -6.0;
    let mut lane = AutomationLane::new(AutoParam::Volume);
    lane.set_point(0, -20.0);
    tr.automation.push(lane);
    (s, t)
}

fn release(ms: u16) {
    let (mut s, t) = long_session();
    let mut rig = PlanRig::new(BLOCK);
    hold(&mut s, t, &[AutoParam::Volume]);
    rig.publish(&s);
    rig.render(0);
    // The preference changes with the release, through the next plan, not a shared value.
    s.touch_return_ms = ms;
    hold(&mut s, t, &[]);
    rig.publish(&s);
    let samples = usize::from(ms) * 48;
    let mut rendered = Vec::new();
    for b in 0..=(samples / BLOCK + 1) {
        rendered.extend_from_slice(&rig.render(((b + 1) * BLOCK) as i64)[0]);
    }
    let lane = level(-20.0);
    for b in 0..samples / BLOCK {
        for k in [b * BLOCK, b * BLOCK + BLOCK - 1] {
            let want = level(-6.0 - 14.0 * k as f32 / samples as f32);
            assert!(close(rendered[k], want), "{ms} ms, sample {k}: {} vs {want}", rendered[k]);
        }
    }
    if samples > 0 {
        assert!(!close(rendered[samples - 1], lane), "still returning one sample before the end");
    }
    assert!(rendered[samples..].iter().all(|x| close(*x, lane)), "{ms} ms: on lane from sample {samples}");
}

#[test]
fn fifty_ms_returns_in_2400_samples() {
    release(50);
}

#[test]
fn one_second_returns_in_48000_samples() {
    release(1000);
}

#[test]
fn zero_ms_snaps_on_the_release_sample() {
    release(0);
}

#[test]
fn changing_the_preference_during_a_return_keeps_that_returns_duration() {
    let (mut s, t) = long_session();
    s.touch_return_ms = 50;
    hold(&mut s, t, &[AutoParam::Volume]);
    let mut rig = PlanRig::new(BLOCK);
    rig.publish(&s);
    rig.render(0);
    hold(&mut s, t, &[]);
    rig.publish(&s);
    rig.render(BLOCK as i64);
    s.touch_return_ms = 1000;
    rig.publish(&s);
    for b in 1..=5 {
        let out = rig.render(((b + 1) * BLOCK) as i64);
        assert!(close(out[0][0], level(-6.0 - 14.0 * b as f32 / 5.0)));
    }
    // A later hold/release uses the newly published preference.
    hold(&mut s, t, &[AutoParam::Volume]);
    rig.publish(&s);
    rig.render(7 * BLOCK as i64);
    hold(&mut s, t, &[]);
    rig.publish(&s);
    rig.render(8 * BLOCK as i64);
    assert!(close(rig.render(9 * BLOCK as i64)[0][0], level(-6.0 - 14.0 / 100.0)));
}
