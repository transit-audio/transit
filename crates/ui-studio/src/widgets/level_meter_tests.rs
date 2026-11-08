//! The level meter: the two ballistics presets at fixed frame steps (never a real clock), the
//! default, the bar on the fader's taper with `meter.hot` only above the reference, the VU
//! reference, the hold line and clip cell, and the gain-reduction bar.

use super::fader;
use super::level_meter::{self, Ballistics, Kind, Meter};
use super::test_support::{Rig, inside, mid, size};
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

/// The vertical channel meter (the mixer's stereo meter): one well per channel on the fader's
/// travel, the 35 percent peak fill on the taper, the RMS layer only when there is a reading, the
/// hold line, a 4 px clip cell above each well (`line`, `meter.hot` latched) and the grid.
#[test]
fn the_channel_meter_draws_a_well_per_channel_on_the_taper() {
    use super::level_meter::{Channel, channel_width, paint_channel};
    let mut rig = Rig::new();
    let travel = egui::Rect::from_min_max(egui::pos2(40.0, 40.0), egui::pos2(60.0, 240.0));
    let peaks = [
        Channel { peak_db: -6.0, rms_db: None, hold_db: Some(-3.0), clip: false },
        Channel { peak_db: -24.0, rms_db: None, hold_db: None, clip: false },
    ];
    let l = rig.idle(|ui| paint_channel(ui.painter(), 100.0, travel, 10.0, &crate::tokens::fader::SCALE_LABELS, &peaks));
    assert_eq!(l.wells.len(), 2);
    assert_eq!(l.wells[0], [100.0, 40.0, 110.0, 240.0], "the first well spans the travel");
    assert_eq!(l.wells[1][0], 113.0, "3 px between wells");
    assert_eq!(channel_width(2, 10.0), 23.0);
    let y = |db: f32| 40.0 + fader::taper(db) * 200.0;
    assert!((l.peaks[0].unwrap()[1] - y(-6.0)).abs() < 0.02 && (l.peaks[1].unwrap()[1] - y(-24.0)).abs() < 0.02, "peaks on the taper");
    assert_eq!(l.peak_color, DARK.meter_ok.gamma_multiply(0.35), "the peak over an RMS reading");
    assert_eq!(l.peak_inks, vec![DARK.meter_ok, DARK.meter_ok], "peak only: the peak is the level, solid");
    assert!(l.rms.iter().all(Option::is_none) && l.over.iter().all(Option::is_none), "peak only: no RMS layer");
    assert!((mid(l.holds[0].unwrap()).y - y(-3.0)).abs() < 0.02 && l.holds[1].is_none());
    assert_eq!(l.clip_cells[0], [100.0, 30.0, 110.0, 34.0], "4 px, 6 above the well (the Frame)");
    assert_eq!((l.clip, l.clip_colors.clone()), (false, vec![DARK.line, DARK.line]));
    assert_eq!(l.grid.len(), 8, "a line at each mark passed (all eight placed at travel 200)");
    // The well, its edge, grid and hold inks and the board's corner radius.
    assert_eq!((l.well_fill, l.well_edge, l.grid_color, l.hold_color), (DARK.surface_inset, DARK.line_inset, DARK.line_inset, DARK.text));
    assert_eq!((l.well_radius, l.fill_radius), (1.5, 0.0), "the well keeps 1.5, the fills are square");
    assert_eq!(l.zero_grid_color, DARK.line_strong, "the 0 dB grid line in `line.strong` (`fader.zeroGridLine`)");
    assert!((size(l.holds[0].unwrap()).1 - 2.0).abs() < 0.01, "a 2 px hold line");
    assert!((l.reference_y - y(0.0)).abs() < 0.02);
    // Negative control: an RMS reading draws the solid layer, hot only above 0 dBFS, and a latch
    // lights the cells.
    let loud = [
        Channel { peak_db: 3.0, rms_db: Some(2.0), hold_db: None, clip: true },
        Channel { peak_db: -12.0, rms_db: Some(-18.0), hold_db: None, clip: false },
    ];
    let l = rig.idle(|ui| paint_channel(ui.painter(), 100.0, travel, 10.0, &crate::tokens::fader::SCALE_LABELS, &loud));
    assert!((l.rms[1].unwrap()[1] - y(-18.0)).abs() < 0.02 && l.over[1].is_none());
    let over = l.over[0].expect("the part above 0 dBFS");
    assert!((over[3] - y(0.0)).abs() < 0.02 && (over[1] - y(2.0)).abs() < 0.02);
    assert!((l.rms[0].unwrap()[1] - y(0.0)).abs() < 0.02, "below the reference the RMS fill stays meter.ok");
    assert_eq!(
        (l.clip, l.clips.clone(), l.clip_colors.clone()),
        (true, vec![true, false], vec![DARK.meter_hot, DARK.line]),
        "each channel its own cell"
    );
}

/// The channel meter's peak fill turns `meter.hot` above the 0 dBFS line and stays `meter.ok`
/// below it, the rule the device meter already follows.
#[test]
fn the_channel_peak_fill_is_hot_only_above_zero_dbfs() {
    use super::level_meter::{Channel, paint_channel};
    let mut rig = Rig::new();
    let travel = egui::Rect::from_min_max(egui::pos2(40.0, 40.0), egui::pos2(60.0, 240.0));
    let y = |db: f32| 40.0 + fader::taper(db) * 200.0;
    let ch = |peak_db| Channel { peak_db, rms_db: None, hold_db: None, clip: false };
    let l = rig.idle(|ui| paint_channel(ui.painter(), 100.0, travel, 10.0, &crate::tokens::fader::SCALE_LABELS, &[ch(3.0), ch(-3.0), ch(0.0)]));
    let hot = l.peak_over[0].expect("hot between 0 and +3");
    assert!((hot[1] - y(3.0)).abs() < 0.02 && (hot[3] - y(0.0)).abs() < 0.02, "hot spans +3 to 0");
    assert!((l.peaks[0].unwrap()[1] - y(0.0)).abs() < 0.02, "the ok fill stops at the 0 dBFS line");
    assert_eq!(l.peak_over_color, DARK.meter_hot.gamma_multiply(0.35));
    // Negative control: a peak at -3 dB or exactly 0 draws no hot.
    assert!(l.peak_over[1].is_none() && l.peak_over[2].is_none(), "meter.hot is never used at or below 0 dBFS");
}

/// The layer a painted shape in the first well is, by its ink (`None`: not part of the well).
fn layer(shape: &egui::Shape, well: egui::Rect) -> Option<&'static str> {
    let egui::Shape::Rect(r) = shape else { return None };
    if r.rect.min.x < well.min.x - 0.01 || r.rect.max.x > well.max.x + 0.01 || r.rect.max.y <= well.min.y {
        return None;
    }
    let peak = DARK.meter_ok.gamma_multiply(crate::tokens::fader::METER_PEAK_ALPHA);
    let hot = DARK.meter_hot.gamma_multiply(crate::tokens::fader::METER_PEAK_ALPHA);
    if (r.stroke.color == DARK.line_inset && r.stroke.width > 0.0) || r.fill == DARK.surface_inset {
        Some("well")
    } else if (r.fill == DARK.line_inset || r.fill == DARK.line_strong) && r.rect.height() <= 1.01 {
        Some("grid")
    } else if r.fill == peak || r.fill == hot {
        Some("peak")
    } else if r.fill == DARK.meter_ok {
        Some("rms")
    } else if r.fill == DARK.text {
        Some("hold")
    } else {
        None
    }
}

/// Whether `layers` follow `fader.meterDrawOrder` (each layer after every earlier one).
fn in_draw_order(layers: &[&str]) -> bool {
    let rank = |l: &str| crate::tokens::fader::METER_DRAW_ORDER.iter().position(|x| *x == l).unwrap();
    layers.windows(2).all(|w| rank(w[0]) <= rank(w[1]))
}

/// The first well's rect and the layers painted in it, for one channel `ch`.
fn paint_one(
    rig: &mut Rig,
    ch: super::level_meter::Channel,
) -> (super::level_meter::ChannelMeterLayout, egui::Rect, Vec<egui::epaint::ClippedShape>) {
    let travel = egui::Rect::from_min_max(egui::pos2(40.0, 40.0), egui::pos2(60.0, 240.0));
    let (l, shapes) = rig.shapes(|ui| super::level_meter::paint_channel(ui.painter(), 100.0, travel, 10.0, &crate::tokens::fader::SCALE_LABELS, &[ch]));
    let well = egui::Rect::from_min_max(egui::pos2(l.wells[0][0], l.wells[0][1]), egui::pos2(l.wells[0][2], l.wells[0][3]));
    (l, well, shapes)
}

/// Fault 4 of the Fader construction board: two fills per bar, the peak at 35 percent
/// (`meter.hot` at 35 percent from 0 dBFS up to a peak above it) and the RMS solid `meter.ok` on
/// top from Y1 to its level, so a sustained signal reads as a bright bar with a paler cap.
#[test]
fn the_channel_meter_draws_the_rms_solid_over_the_35_percent_peak() {
    use super::level_meter::Channel;
    let mut rig = Rig::new();
    let (l, well, shapes) = paint_one(&mut rig, Channel { peak_db: 2.0, rms_db: Some(-12.0), hold_db: Some(2.0), clip: false });
    let rms = l.rms[0].expect("an RMS fill");
    let peak = l.peaks[0].expect("a peak fill");
    assert!(rms[1] > peak[1] && rms[3] == peak[3], "the RMS from Y1 to its level under the peak's: {rms:?} {peak:?}");
    assert!(l.peak_over[0].is_some(), "meter.hot at 35 percent from 0 dB up to the peak");
    let ink = |want: &str, y: f32| {
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Rect(r) if (r.rect.min.y - y).abs() < 0.01 && layer(&s.shape, well) == Some(want) => Some(r.fill),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the {want} rect painted"))
    };
    assert_eq!(ink("rms", rms[1]), DARK.meter_ok, "the RMS is solid `meter.ok`");
    assert_eq!(ink("peak", peak[1]).a(), DARK.meter_ok.gamma_multiply(0.35).a(), "the peak at 35 percent");
    // Negative control: a source with no RMS reading has no RMS layer; its peak is drawn solid
    // instead (Fills are square), never at the 35 percent of a zone above an RMS bar.
    let (old, _, _) = paint_one(&mut rig, Channel { peak_db: 2.0, rms_db: None, hold_db: Some(2.0), clip: false });
    assert!(old.rms[0].is_none());
    assert_eq!((old.peak_inks[0], old.peak_over_inks[0]), (DARK.meter_ok, DARK.meter_hot), "peak only: solid, `meter.hot` solid above 0");
}

/// Fault 5 of the Fader construction board: the draw order inside a well is the well and its
/// edge, the grid lines, the peak fill, the RMS fill, the hold line (`fader.meterDrawOrder`):
/// nothing is drawn over the level.
#[test]
fn the_channel_meter_paints_well_grid_peak_rms_hold_in_that_order() {
    use super::level_meter::Channel;
    let mut rig = Rig::new();
    let (_, well, shapes) = paint_one(&mut rig, Channel { peak_db: 2.0, rms_db: Some(-12.0), hold_db: Some(2.0), clip: false });
    let layers: Vec<&str> = shapes.iter().filter_map(|s| layer(&s.shape, well)).collect();
    for want in crate::tokens::fader::METER_DRAW_ORDER {
        assert!(layers.contains(&want), "{want} painted: {layers:?}");
    }
    assert!(in_draw_order(&layers), "well and edge, grid, peak, RMS, hold: {layers:?}");
    // Negative controls: the grid drawn last, and the edge stroked over the level (the old
    // order), both fail.
    let mut grid_last: Vec<&str> = layers.iter().copied().filter(|l| *l != "grid").collect();
    grid_last.push("grid");
    assert!(!in_draw_order(&grid_last), "the grid over the level is out of order");
    let mut edge_last = layers.clone();
    edge_last.push("well");
    assert!(!in_draw_order(&edge_last), "the well's edge over the level is out of order");
}

/// Meter grid (Fader > Meter grid): the meter draws a grid line only where the scale beside it
/// placed a label, on the same physical pixel row as that label's tick, at the docked strip's
/// travel (80, four lines) and taller ones, at 1.0 and 1.5; 0 dB in `line.strong`, the rest in
/// `line.inset`.
#[test]
fn the_meter_grid_rows_are_the_faders_tick_rows() {
    use super::level_meter::{Channel, paint_channel};
    for ppp in [1.0f32, 1.5] {
        let mut rig = Rig::new();
        rig.ctx.set_pixels_per_point(ppp);
        for travel in [80.0f32, 100.0, 200.0] {
            let (l, m, shapes) = {
                let ((l, m), shapes) = rig.shapes(|ui| {
                    let l = fader::show(ui, egui::Id::new("f"), fader::Variant::Channel, travel + 20.0, 0.0, false).layout;
                    let track = egui::Rect::from_min_max(egui::pos2(l.track[0], l.track[1]), egui::pos2(l.track[2], l.track[3]));
                    let ch = Channel { peak_db: f32::NEG_INFINITY, rms_db: None, hold_db: None, clip: false };
                    let m = paint_channel(ui.painter(), l.meter.unwrap()[0], track, crate::tokens::fader::METER_BAR, &l.placed, &[ch, ch]);
                    (l, m)
                });
                (l, m, shapes)
            };
            let what = format!("{ppp}x travel {travel}");
            let rows = |r: &[f32; 4]| [(r[1] * ppp).round() as i32, (r[3] * ppp).round() as i32];
            let grid: Vec<_> = m.grid.iter().map(rows).collect();
            let ticks: Vec<_> = l.ticks.iter().map(rows).collect();
            assert_eq!(grid, ticks, "{what}: one grid line per tick, on its row");
            assert_eq!(l.placed, fader::placed_marks(travel), "{what}");
            if travel == 80.0 {
                assert_eq!(grid.len(), 4, "{what}: 0, −12, −24, −∞ on the docked strip");
            }
            // The inks as painted: one `line.strong` line (0 dB), the rest `line.inset`.
            let well = egui::Rect::from_min_max(egui::pos2(m.wells[0][0], m.wells[0][1]), egui::pos2(m.wells[0][2], m.wells[0][3]));
            let inks: Vec<egui::Color32> = shapes
                .iter()
                .filter(|s| layer(&s.shape, well) == Some("grid"))
                .filter_map(|s| if let egui::Shape::Rect(r) = &s.shape { Some(r.fill) } else { None })
                .collect();
            assert_eq!(inks.len(), grid.len(), "{what}: {inks:?}");
            assert_eq!(inks.iter().filter(|c| **c == DARK.line_strong).count(), 1, "{what}: the 0 dB line");
            assert!(inks.iter().all(|c| *c == DARK.line_strong || *c == DARK.line_inset), "{what}: never `line`");
            // Negative control: the old grid (every mark) fails the tick equality where labels thin.
            if travel <= 100.0 {
                let old = rig.idle(|ui| {
                    let track = egui::Rect::from_min_max(egui::pos2(l.track[0], l.track[1]), egui::pos2(l.track[2], l.track[3]));
                    let ch = Channel { peak_db: f32::NEG_INFINITY, rms_db: None, hold_db: None, clip: false };
                    paint_channel(ui.painter(), l.meter.unwrap()[0], track, 10.0, &crate::tokens::fader::SCALE_LABELS, &[ch])
                });
                assert_ne!(old.grid.iter().map(rows).collect::<Vec<_>>(), ticks, "{what}: the every-mark grid is a ladder");
            }
        }
    }
}

/// Fills are square (Fader > Meter): the peak, RMS and hot fills and the hold line have no corner
/// radius and sit 1 px inside the well; only the well keeps its 1.5 radius. A peak-only source
/// paints its peak opaque (`meter.ok`, `meter.hot` above 0).
#[test]
fn the_channel_meter_fills_are_square_inside_the_well() {
    use super::level_meter::Channel;
    let mut rig = Rig::new();
    for ch in [
        Channel { peak_db: 4.0, rms_db: Some(2.0), hold_db: Some(4.0), clip: false },
        Channel { peak_db: -6.0, rms_db: Some(-12.0), hold_db: Some(6.0), clip: false },
        Channel { peak_db: 3.0, rms_db: None, hold_db: Some(-90.0 + 1.0), clip: false },
    ] {
        let (l, well, shapes) = paint_one(&mut rig, ch);
        let mut seen = 0;
        for s in &shapes {
            let egui::Shape::Rect(r) = &s.shape else { continue };
            let in_well = r.rect.min.x >= well.min.x - 0.01 && r.rect.max.x <= well.max.x + 0.01 && r.rect.min.y >= well.min.y - 0.01;
            let layer = layer(&s.shape, well);
            if layer == Some("well") {
                assert_ne!(r.corner_radius, egui::CornerRadius::ZERO, "{ch:?}: the well keeps its radius");
            } else if matches!(layer, Some("peak" | "rms" | "hold")) || (in_well && r.fill == DARK.meter_hot) {
                assert_eq!(r.corner_radius, egui::CornerRadius::ZERO, "{ch:?}: a square fill: {r:?}");
                assert!(r.rect.min.x >= well.min.x + 1.0 - 0.01 && r.rect.max.x <= well.max.x - 1.0 + 0.01, "{ch:?}: 1 px inside: {r:?}");
                assert!(r.rect.min.y >= well.min.y + 1.0 - 0.01 && r.rect.max.y <= well.max.y - 1.0 + 0.01, "{ch:?}: 1 px inside: {r:?}");
                seen += 1;
            }
        }
        assert!(seen >= 3, "{ch:?}: the fills and hold were checked ({seen})");
        for r in l.peaks.iter().chain(&l.rms).chain(&l.over).chain(&l.peak_over).chain(&l.holds).flatten() {
            assert!(r[0] >= l.wells[0][0] + 1.0 - 0.01 && r[2] <= l.wells[0][2] - 1.0 + 0.01 && r[3] <= l.wells[0][3] - 1.0 + 0.01, "{r:?}");
        }
    }
    // A peak-only source paints its peak opaque, hot opaque above 0.
    let (l, _, shapes) = paint_one(&mut rig, Channel { peak_db: 3.0, rms_db: None, hold_db: None, clip: false });
    let ink = |r: [f32; 4]| {
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Rect(x) if (x.rect.min.y - r[1]).abs() < 0.01 && (x.rect.max.y - r[3]).abs() < 0.01 && x.rect.width() < 10.0 => Some(x.fill),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(ink(l.peaks[0].unwrap()), DARK.meter_ok);
    assert_eq!(ink(l.peak_over[0].unwrap()).a(), 255, "opaque `meter.hot` above 0");
    assert_eq!(ink(l.peak_over[0].unwrap()), DARK.meter_hot);
}
