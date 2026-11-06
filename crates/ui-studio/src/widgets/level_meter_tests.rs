//! The level meter: the two ballistics presets at fixed frame steps (never a real clock), the
//! default, the bar on the fader's taper with `meter.hot` only above the reference, the VU
//! reference, the hold line and clip cell, and the gain-reduction bar.

use super::fader;
use super::level_meter::{self, Ballistics, Kind, Meter};
use super::test_support::{Rig, inside, mid};
use crate::theme::DARK;

/// Steps a reading from `from` toward `to` in frames of `dt` up to the first frame at or past
/// `ms` of accumulated time (what the screen shows at that moment).
fn run(from: f32, to: f32, dt: f32, ms: f32, b: Ballistics) -> f32 {
    let mut v = from;
    let mut elapsed = 0.0;
    while elapsed < ms {
        v = level_meter::meter_step(v, to, dt, b);
        elapsed += dt * 1000.0;
    }
    v
}

fn reached(from: f32, to: f32, v: f32) -> f32 {
    (v - from) / (to - from)
}

#[test]
fn the_default_jumps_up_at_once_and_falls_off_over_300_ms() {
    assert_eq!(Ballistics::default(), Ballistics::PEAK_FALLOFF);
    assert_eq!(Meter::level(-6.0).ballistics, Ballistics::PEAK_FALLOFF, "every meter defaults to the fall-off bar");
    for dt in [1.0 / 60.0, 1.0 / 144.0, 0.005] {
        let b = Ballistics::PEAK_FALLOFF;
        assert_eq!(level_meter::meter_step(-40.0, -6.0, dt, b), -6.0, "a rise lands within the frame (dt {dt})");
        let fall = run(-6.0, -40.0, dt, 300.0, b);
        assert!(reached(-6.0, -40.0, fall) >= 0.99, "dt {dt}: {fall}");
        let early = run(-6.0, -40.0, dt, 100.0, b);
        assert!(reached(-6.0, -40.0, early) < 0.9, "the fall is smoothed, not instant: {early}");
    }
}

#[test]
fn the_vu_preset_rises_and_falls_over_300_ms() {
    for dt in [1.0 / 60.0, 1.0 / 144.0, 0.005] {
        let b = Ballistics::VU;
        let first = level_meter::meter_step(-40.0, -18.0, dt, b);
        assert!(first < -30.0, "no jump on a rise (dt {dt}): {first}");
        let up = run(-40.0, -18.0, dt, 300.0, b);
        assert!(reached(-40.0, -18.0, up) >= 0.99, "dt {dt}: {up}");
        let down = run(-18.0, -40.0, dt, 300.0, b);
        assert!(reached(-18.0, -40.0, down) >= 0.99, "dt {dt}: {down}");
        // 99 percent at 300 ms, not long before: the time constant is the standard's.
        let at_200 = run(-40.0, -18.0, dt, 200.0, b);
        assert!(reached(-40.0, -18.0, at_200) < 0.98, "dt {dt}: {at_200}");
    }
    assert_eq!(level_meter::meter_step(f32::NEG_INFINITY, -12.0, 0.01, Ballistics::PEAK_FALLOFF), -12.0, "silence is the taper's floor");
}

#[test]
fn the_bar_sits_on_the_fader_taper_with_meter_hot_only_above_the_reference() {
    let mut rig = Rig::new();
    let l = rig.idle(|ui| level_meter::show(ui, 10.0, 220.0, &Meter { hold_db: Some(-3.0), peak_db: Some(-4.0), ..Meter::level(-12.0) }));
    let inner_top = l.well[1] + 1.0;
    let inner_h = l.well[3] - l.well[1] - 2.0;
    let want = inner_top + fader::taper(-12.0) * inner_h;
    assert!((l.fill[1] - want).abs() < 0.02, "the bar top at -12 dB on the taper");
    assert_eq!(l.over, None, "no meter.hot below 0 dBFS");
    assert_eq!(l.fill_color, DARK.meter_ok);
    let hold = l.hold.unwrap();
    assert!((mid(hold).y - (inner_top + fader::taper(-3.0) * inner_h)).abs() < 0.02 && (hold[3] - hold[1] - 2.0).abs() < 0.02, "2 px hold line");
    assert!(inside(l.peak.unwrap(), l.well));
    let cell = l.clip_cell.unwrap();
    assert!(cell[3] <= l.well[1] && (cell[3] - cell[1] - 4.0).abs() < 0.02, "4 px clip cell above the well");
    assert_eq!(l.clip_color, DARK.surface_inset, "unlit");
    assert_eq!(l.tag, None);
    let hot = rig.idle(|ui| level_meter::show(ui, 10.0, 220.0, &Meter { clip: true, ..Meter::level(3.0) }));
    let over = hot.over.expect("above 0 dBFS reads meter.hot");
    assert!((over[3] - hot.reference_y).abs() < 0.02 && hot.fill[1] >= hot.reference_y - 0.02, "meter.hot only above the reference");
    assert_eq!((hot.over_color, hot.clip_color), (DARK.meter_hot, DARK.meter_hot));
}

#[test]
fn zero_vu_lands_on_the_reference_mark_and_the_reference_is_settable() {
    let mut rig = Rig::new();
    let at_ref = rig.idle(|ui| level_meter::show(ui, 10.0, 220.0, &Meter::vu(level_meter::VU_REFERENCE_DBFS)));
    assert_eq!(at_ref.reference_dbfs, -18.0, "0 VU defaults to -18 dBFS");
    assert!((at_ref.fill[1] - at_ref.reference_y).abs() < 0.02, "0 VU fills to the 0 mark");
    assert_eq!(at_ref.over, None);
    assert_eq!(at_ref.tag.map(|t| t.0), Some("VU"));
    let minus_20 = rig.idle(|ui| level_meter::show(ui, 10.0, 220.0, &Meter { reference_dbfs: -20.0, ..Meter::vu(-18.0) }));
    assert!(minus_20.over.is_some(), "-18 dBFS is +2 VU against a -20 reference");
}

#[test]
fn a_gain_reduction_bar_fills_down_from_the_top_in_mod() {
    let mut rig = Rig::new();
    let none = rig.idle(|ui| level_meter::show(ui, 10.0, 200.0, &Meter::gain_reduction(0.0)));
    let six = rig.idle(|ui| level_meter::show(ui, 10.0, 200.0, &Meter::gain_reduction(6.0)));
    assert_eq!(Meter::gain_reduction(6.0).kind, Kind::GainReduction);
    assert!((none.fill[3] - none.fill[1]).abs() < 0.02, "no reduction, no bar");
    assert!((six.fill[1] - (six.well[1] + 1.0)).abs() < 0.02 && six.fill[3] > six.fill[1], "fills from the top");
    assert_eq!((six.fill_color, six.over, six.clip_cell), (DARK.modulation, None, None), "mod, no over zone, no clip cell");
    assert_eq!(six.tag.map(|t| t.0), Some("GR"));
}

#[test]
fn the_presets_match_tokens_json() {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../../../docs/design/language/tokens.json")).unwrap();
    let b = &v["meter"]["ballistics"];
    let f = |x: &serde_json::Value| x.as_f64().map(|x| x as f32);
    assert_eq!(
        (f(&b["peak_falloff"]["attack_ms"]), f(&b["peak_falloff"]["release_ms"])),
        (Some(Ballistics::PEAK_FALLOFF.attack_ms), Some(Ballistics::PEAK_FALLOFF.release_ms))
    );
    assert_eq!((f(&b["vu"]["attack_ms"]), f(&b["vu"]["release_ms"])), (Some(Ballistics::VU.attack_ms), Some(Ballistics::VU.release_ms)));
    assert_eq!(f(&b["vu"]["reference_dbfs"]), Some(level_meter::VU_REFERENCE_DBFS));
}
