//! The channel strip in the design language (#326): the anatomy's fixed sections, the faders
//! lining up, the selected border, the insert slot's glyph, the short variant's buttons and
//! lists, the stereo meter's fills and clip cell, the main bus and the color bar. Each check has
//! a negative control: the same measurement on a case that must differ.

use crate::tests::*;
use crate::theme::DARK;
use crate::tokens::channel_strip as cs;
use crate::widgets::fader::taper;

fn color(c: [u8; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3])
}

fn h(r: [f32; 4]) -> f32 {
    r[3] - r[1]
}

/// Three audio tracks with 0, 1 and 3 inserts.
fn three(app: &mut StudioApp, first: TrackId) -> [TrackId; 3] {
    let b = new_track(app, json!({"kind": "audio", "format": "stereo", "name": "One"}));
    let c = new_track(app, json!({"kind": "audio", "format": "stereo", "name": "Three"}));
    app.add_device(b, "gain").unwrap();
    for _ in 0..3 {
        app.add_device(c, "gain").unwrap();
    }
    [first, b, c]
}

#[test]
fn sections_are_fixed_and_every_fader_lines_up_with_0_1_and_3_inserts() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let tracks = three(&mut app, a);
    full_mixer(&mut app, &ctx, &mut time);
    let strips: Vec<_> = tracks.iter().map(|t| strip(&app, *t)).collect();
    assert_eq!(strips.iter().map(|s| s.inserts.len()).collect::<Vec<_>>(), [0, 1, 3]);
    for s in &strips {
        assert_eq!(s.rect[2] - s.rect[0], cs::WIDTH, "132 wide");
        assert_eq!(h(s.inserts_section), cs::INSERTS_HEIGHT, "{}: inserts 112", s.name);
        assert_eq!(h(s.sends_section), cs::SENDS_HEIGHT, "{}: sends 86", s.name);
        assert_eq!(h(s.color_bar), cs::COLOR_BAR, "the 3 px color bar");
        for r in &s.inserts {
            assert_eq!(h(r.rect), cs::INSERT_SLOT_HEIGHT, "22 px slots");
        }
        assert!(s.inserts_section[3] <= s.sends_section[1] && s.sends_section[3] <= s.output[1], "inserts, sends, output in order");
    }
    for s in &strips[1..] {
        assert_eq!(
            (s.inserts_section[1], s.sends_section[1]),
            (strips[0].inserts_section[1], strips[0].sends_section[1]),
            "{}: the sections line up",
            s.name
        );
        assert_eq!((s.fader[1], s.fader[3]), (strips[0].fader[1], strips[0].fader[3]), "{}: the fader lines up", s.name);
        assert_eq!(
            (s.output[1], s.knob[1], s.mute[1]),
            (strips[0].output[1], strips[0].knob[1], strips[0].mute[1]),
            "{}: every section lines up",
            s.name
        );
    }
    // Negative control: the short variant's sections are shorter, so its fader starts higher; the
    // same measurement tells the two apart.
    let full_top = strips[0].fader[1];
    app.ui.mixer_full = false;
    frames(&mut app, &ctx, 4, &mut time);
    let short = strip(&app, a);
    assert!(short.short && short.inserts_section == [0.0; 4]);
    let shift = (short.fader[1] - short.rect[1]) - (full_top - strips[0].rect[1]);
    assert_eq!(
        shift,
        -(cs::INSERTS_HEIGHT + cs::SENDS_HEIGHT + 2.0 * (1.0 + cs::SECTION_PADDING + cs::SECTION_GAP) - cs::SHORT_ROW - cs::SECTION_GAP)
    );
}

#[test]
fn the_selected_strip_has_a_signal_border_and_the_others_a_line() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let b = new_track(&mut app, json!({"kind": "audio", "format": "stereo", "name": "B"}));
    for (focus, other) in [(a, b), (b, a)] {
        app.focus(focus);
        frames(&mut app, &ctx, 3, &mut time);
        let (s, o) = (strip(&app, focus), strip(&app, other));
        assert!(s.focused && !o.focused);
        assert_eq!(color(s.border_color), DARK.signal, "the selected strip");
        // Negative control: the strip beside it.
        assert_eq!(color(o.border_color), DARK.line, "an unselected strip");
    }
}

#[test]
fn the_insert_glyph_is_the_state_and_a_click_on_it_toggles_bypass() {
    let (mut app, ctx, mut time, a) = mixer_app();
    app.add_device(a, "gain").unwrap();
    app.add_device(a, "gain").unwrap();
    full_mixer(&mut app, &ctx, &mut time);
    let rows = strip(&app, a).inserts;
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert!(r.on);
        assert_eq!((color(r.glyph_color), color(r.name_color)), (DARK.signal, DARK.text), "on: signal glyph, text name");
        assert!(r.power[0] >= r.name_rect[2] && r.power[2] <= r.rect[2], "the glyph is right of the name");
    }
    let before = undo_len(&mut app);
    click(&mut app, mid(rows[0].power));
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.engine.session().track(a).unwrap().mixer.inserts[0].as_ref().unwrap().bypass, "the glyph bypassed it");
    assert_eq!(undo_len(&mut app), before + 1);
    let rows = strip(&app, a).inserts;
    assert!(!rows[0].on);
    assert_eq!((color(rows[0].glyph_color), color(rows[0].name_color)), (DARK.text_faint, DARK.text_muted), "bypassed: faint glyph, muted name");
    // Negative control: the other insert keeps its look.
    assert_eq!((color(rows[1].glyph_color), color(rows[1].name_color)), (DARK.signal, DARK.text));
    assert!(app.ui.plugin_windows.is_empty(), "a click on the glyph opens nothing");
    // A click on the name opens the device.
    click(&mut app, mid(rows[1].name_rect));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(app.ui.plugin_windows, vec![(a, rows[1].slot)]);
    assert!(!app.engine.session().track(a).unwrap().mixer.inserts[rows[1].slot].as_ref().unwrap().bypass, "and does not bypass it");
}

#[test]
fn the_short_variant_counts_inserts_and_sends_and_opens_their_lists() {
    let (mut app, ctx, mut time, a) = mixer_app();
    app.add_device(a, "gain").unwrap();
    app.add_device(a, "gain").unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let s = strip(&app, a);
    assert!(app.mixer.layout.short && s.short, "the docked pane is short");
    assert_eq!((s.fx_text.as_str(), s.snd_text.as_str()), ("FX 2", "Snd +"));
    assert_eq!((h(s.fx_button), h(s.snd_button)), (cs::SHORT_ROW, cs::SHORT_ROW));
    assert!(s.inserts.is_empty() && s.sends.is_empty() && s.add_insert == [0.0; 4] && s.list_rect == [0.0; 4], "the lists are closed");
    // FX opens every insert and + Insert under the button; a second click closes it.
    click(&mut app, mid(s.fx_button));
    frames(&mut app, &ctx, 4, &mut time);
    let open = strip(&app, a);
    assert_eq!(open.inserts.len(), 2);
    assert!(open.add_insert[3] > open.add_insert[1] && open.list_rect[1] >= s.fx_button[3] - 0.01, "the list sits under the button");
    click(&mut app, mid(open.inserts[0].power));
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.engine.session().track(a).unwrap().mixer.inserts[0].as_ref().unwrap().bypass, "the list's glyph works");
    assert_eq!(strip(&app, a).inserts.len(), 2, "and the list stays open");
    click(&mut app, mid(s.fx_button));
    frames(&mut app, &ctx, 4, &mut time);
    assert!(strip(&app, a).inserts.is_empty(), "a second click closes it");
    // Snd opens + Send, which adds a send through its menu.
    click(&mut app, mid(s.snd_button));
    frames(&mut app, &ctx, 4, &mut time);
    let add = strip(&app, a).add_send;
    assert!(add[3] > add[1], "+ Send in the list");
    click(&mut app, mid(add));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == crate::mixer::NEW_FX).cloned().expect("New FX Channel in + Send");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(a).unwrap().mixer.sends[0].is_some(), "the send was added from the list");
    assert_eq!(strip(&app, a).snd_text, "Snd 1");
    // Negative control: the full mixer window has no count buttons and lists the inserts inline.
    full_mixer(&mut app, &ctx, &mut time);
    let f = strip(&app, a);
    assert!(!f.short && f.fx_button == [0.0; 4] && f.snd_button == [0.0; 4] && f.inserts.len() == 2);
}

/// Feeds `peaks` to a track's meter for `n` frames of 1/60 s.
fn feed(app: &mut StudioApp, t: TrackId, peaks: [f32; 2], n: usize) {
    for _ in 0..n {
        crate::host::feed_meter(app.meters.entry(t).or_default(), peaks, 0.0, 1.0 / 60.0);
    }
}

#[test]
fn the_meter_fills_on_the_taper_and_its_clip_cell_latches_and_clears() {
    let (mut app, ctx, mut time, a) = mixer_app();
    // -6.02 dBFS on the left, -20 on the right: the kit's fall-off jumps to a rise at once.
    feed(&mut app, a, [0.5, 0.1], 3);
    let f = app.meters[&a].falloff;
    assert!((f[0] - 0.5).abs() < 1e-6 && (f[1] - 0.1).abs() < 1e-6, "peak fall-off: no attack: {f:?}");
    frames(&mut app, &ctx, 2, &mut time);
    // The frames fed silence (no player), so the reading has started to fall off; the fill shows
    // the reading the meter holds now.
    let reading = app.meters[&a].falloff.map(|lin| 20.0 * lin.log10());
    assert!(reading[0] < -6.03, "falling off: {reading:?}");
    let s = strip(&app, a);
    let m = s.meter_layout.clone().expect("the meter");
    assert_eq!(m.wells.len(), 2, "a stereo meter: one well per channel");
    let well = m.wells[0];
    assert_eq!(well[2] - well[0], crate::tokens::fader::METER_BAR, "10 px bars");
    assert_eq!((well[1], well[3]), (s.fader[1], s.fader[3]), "the wells span the fader's travel");
    let y = |db: f32| well[1] + taper(db) * (well[3] - well[1]);
    let left = m.peaks[0].expect("a peak fill");
    assert!((left[1] - y(reading[0])).abs() < 0.02, "the peak fill tops at the reading on the taper: {left:?}");
    assert!((mid(m.holds[0].unwrap()).1 - y(20.0 * 0.5f32.log10())).abs() < 0.02, "the hold line stays at the -6 dB peak");
    assert_eq!(m.peak_color, DARK.meter_ok.gamma_multiply(0.35), "meter.ok at 35 percent");
    assert!(m.rms.iter().all(Option::is_none), "the engine publishes peaks only: no RMS layer");
    let hold = m.holds[0].expect("a hold line");
    assert_eq!(hold[3] - hold[1], 2.0, "the 2 px hold line");
    // Negative control: the quieter channel's fill tops lower.
    let right = m.peaks[1].expect("a peak fill");
    assert!((right[1] - y(reading[1])).abs() < 0.02 && right[1] > left[1], "{right:?}");
    assert_eq!(m.grid.len(), 8, "a grid line at each taper mark");
    // The clip cell: `line` until a peak of 1.0 or more, then `meter.hot` until the readout click.
    assert!(!m.clip && m.clip_colors == [DARK.line, DARK.line]);
    let cell = m.clip_cells[0];
    assert!(cell[3] <= well[1] && cell[3] - cell[1] == crate::tokens::fader::METER_CLIP_CELL, "a 4 px cell above the well");
    feed(&mut app, a, [1.2, 0.1], 1);
    frames(&mut app, &ctx, 2, &mut time);
    let lit = strip(&app, a);
    let m = lit.meter_layout.clone().unwrap();
    // The engine publishes a peak per channel: only the channel that clipped latches.
    assert!(m.clip && m.clips == [true, false] && m.clip_colors == [DARK.meter_hot, DARK.line], "the left channel latched, the right did not");
    assert_eq!(color(lit.peak_color), DARK.meter_hot, "the readout turns hot");
    click(&mut app, mid(lit.peak_rect));
    frames(&mut app, &ctx, 4, &mut time);
    let m = strip(&app, a).meter_layout.unwrap();
    assert!(!m.clip && m.clip_colors == [DARK.line, DARK.line], "the peak readout click clears it");
}

#[test]
fn the_main_bus_is_160_wide_with_mute_only_and_wider_meters() {
    let (mut app, ctx, mut time, a) = mixer_app();
    app.run("track.new", json!({"kind": "master"})).unwrap();
    full_mixer(&mut app, &ctx, &mut time);
    let m = app.mixer.layout.master.clone().expect("the main bus");
    assert_eq!(m.rect[2] - m.rect[0], cs::WIDTH_MAIN);
    assert_eq!(m.number, "M");
    assert_eq!(m.toggles.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["M"], "Mute only: Mono, Dim and Ref wait for the engine");
    let well = m.meter_layout.as_ref().unwrap().wells[0];
    assert_eq!(well[2] - well[0], crate::tokens::fader::METER_BAR_MAIN, "14 px bars");
    // Negative control: a channel's strip and bars are narrower and it has all four toggles.
    let s = strip(&app, a);
    assert_eq!(s.rect[2] - s.rect[0], cs::WIDTH);
    assert_eq!(s.meter_layout.as_ref().unwrap().wells[0][2] - s.meter_layout.as_ref().unwrap().wells[0][0], crate::tokens::fader::METER_BAR);
    assert_eq!(s.toggles.len(), 4);
    // The mute works on the main bus.
    let master = app.engine.session().tracks.iter().find(|t| t.kind == transit_model::TrackKind::Master).unwrap().id;
    click(&mut app, mid(m.mute));
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.engine.session().track(master).unwrap().mixer.mute);
}

#[test]
fn the_color_bar_takes_the_palette_hue_nearest_the_tracks_rgb() {
    use crate::mixer::{hues, nearest};
    let table = hues();
    assert_eq!(table[0], 11.0, "hueStart from tokens.json");
    assert_eq!(table[1] - table[0], 22.5, "hueStep from tokens.json");
    // Pure red is OKLCH hue 29: Vermilion (33.5); pure green 142: Green (146); pure blue 264:
    // Blue (258.5); a grey has no chroma: Graphite.
    let samples: [([u8; 3], usize); 4] = [([255, 0, 0], 1), ([0, 255, 0], 6), ([0, 0, 255], 11), ([128, 128, 128], 16)];
    for (rgb, want) in samples {
        assert_eq!(nearest(rgb, &table), want, "{rgb:?} maps to {}", crate::tokens::palette::NAMES[want]);
    }
    // Negative control: a hue table rotated half a step maps the same samples elsewhere.
    let wrong: [f32; 16] = std::array::from_fn(|i| table[i] + 11.25);
    assert!(samples[..3].iter().any(|(rgb, want)| nearest(*rgb, &wrong) != *want), "a wrong hue table fails");
    // On a strip: the track's RGB gives the bar its hue's `bar` value.
    let (mut app, ctx, mut time, a) = mixer_app();
    app.run("track.color", json!({"tracks": [a.0], "color": [0, 0, 255]})).unwrap();
    let grey = new_track(&mut app, json!({"kind": "audio", "format": "stereo", "name": "Grey"}));
    app.run("track.color", json!({"tracks": [grey.0], "color": [128, 128, 128]})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(color(strip(&app, a).color_bar_color), crate::tokens::palette::DARK_BAR[11], "Blue's bar");
    assert_eq!(color(strip(&app, grey).color_bar_color), crate::tokens::palette::DARK_BAR[16], "Graphite's bar");
}

#[test]
fn the_gain_field_stops_at_plus_6_and_a_higher_gain_reads_truthfully_at_the_top() {
    let (mut app, ctx, mut time, a) = mixer_app();
    // A gain above the taper's top from elsewhere (an old session, a lane, MIDI Learn).
    app.run("mix.volume", json!({"tracks": [a.0], "db": 9.0})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let s = strip(&app, a);
    assert_eq!(s.volume_text, "+9.0", "the readout tells the truth");
    let cap = s.fader_layout.as_ref().unwrap().cap;
    assert!(((cap[1] + cap[3]) / 2.0 - s.fader[1]).abs() < 0.02, "the cap is pinned at the top of the travel");
    // Typing past +6 stops at +6, like the drag.
    let ro = mid(s.readout_rect);
    click(&mut app, ro);
    frames(&mut app, &ctx, 6, &mut time);
    typed(&mut app, "12");
    frames(&mut app, &ctx, 3, &mut time);
    key(&mut app, "Enter");
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(app.engine.session().track(a).unwrap().mixer.volume_db, 6.0);
    // Negative control: at 0 dB the cap sits at 14 percent of the travel, not the top.
    app.run("mix.volume", json!({"tracks": [a.0], "db": 0.0})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let s = strip(&app, a);
    let cap = s.fader_layout.as_ref().unwrap().cap;
    assert!((cap[1] + cap[3]) / 2.0 > s.fader[1] + 10.0);
}

#[test]
fn at_the_docked_panes_minimum_the_short_strip_keeps_its_readouts_inside() {
    use crate::mixer::{min_pane_height, strip_fits};
    let (mut app, ctx, mut time, a) = mixer_app();
    app.ui.mixer_height = 0.0;
    frames(&mut app, &ctx, 4, &mut time);
    let l = &app.mixer.layout;
    // The panel's own 1 px edge comes off the area egui gives the strips.
    assert!((l.area[3] - l.area[1] - min_pane_height()).abs() <= 1.0, "the pane clamps to its minimum: {:?}", l.area);
    let s = strip(&app, a);
    assert!(s.short);
    for r in [s.readout_rect, s.peak_rect] {
        assert!(r[1] >= l.area[1] && r[3] <= l.area[3], "{r:?} inside {:?}", l.area);
        assert!(r[1] >= s.toggles[3].rect[3], "below the toggles");
    }
    assert!(s.fader[3] - s.fader[1] >= 80.0 - 0.01, "the fader keeps its travel");
    assert!(strip_fits(min_pane_height() - 9.0, true));
    // Negative control: the old minimum, 260, does not fit the short strip.
    assert!(!strip_fits(260.0 - 8.0, true));
}

/// A short strip's FX and Snd lists stay inside the pane when the button is at the bottom edge:
/// the list opens over the button, and a list taller than either side is pushed back inside.
#[test]
fn a_list_at_the_panes_bottom_opens_upward_inside_the_pane() {
    use crate::mixer::list_origin;
    let pane = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 400.0));
    let button = egui::Rect::from_min_size(egui::pos2(10.0, 360.0), egui::vec2(60.0, 20.0));
    let h = 120.0;
    let (pos, pivot) = list_origin(button, h, pane);
    let list = egui::Rect::from_min_size(egui::pos2(pos.x - pivot.x().to_factor() * 150.0, pos.y - pivot.y().to_factor() * h), egui::vec2(150.0, h));
    assert!(pane.contains_rect(list), "{list:?} inside {pane:?}");
    // Negative control: the unclamped position, under the button, leaves the pane.
    let under = egui::Rect::from_min_size(button.left_bottom(), egui::vec2(150.0, h));
    assert!(!pane.contains_rect(under));
    // Room below keeps the list under the button.
    let top = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(60.0, 20.0));
    assert_eq!(list_origin(top, h, pane), (top.left_bottom(), egui::Align2::LEFT_TOP));
}

/// The open FX list of a short strip lies inside the docked pane.
#[test]
fn the_open_fx_list_lies_inside_the_docked_pane() {
    let (mut app, ctx, mut time, a) = mixer_app();
    for _ in 0..6 {
        app.add_device(a, "gain").unwrap();
    }
    app.ui.mixer_height = 0.0;
    frames(&mut app, &ctx, 4, &mut time);
    let s = strip(&app, a);
    click(&mut app, mid(s.fx_button));
    frames(&mut app, &ctx, 4, &mut time);
    let (l, area) = (strip(&app, a).list_rect, app.mixer.layout.area);
    assert!(l[3] > l[1], "the list is open");
    assert!(l[1] >= area[1] - 0.01 && l[3] <= area[3] + 0.01, "{l:?} inside {area:?}");
}

#[test]
fn every_strip_draws_the_scale_ticks_beside_its_cap_column_in_both_variants() {
    let (mut app, ctx, mut time, a) = mixer_app();
    for full in [false, true] {
        if full {
            full_mixer(&mut app, &ctx, &mut time);
        }
        let s = strip(&app, a);
        let f = s.fader_layout.clone().unwrap();
        let well = s.meter_layout.as_ref().unwrap().wells[0];
        assert_eq!(f.ticks.len(), 8, "full {full}: a tick per scale mark");
        for (t, (_, label)) in f.ticks.iter().zip(&f.marks) {
            assert!(t[0] >= f.cap[2] && t[2] < label[0] && t[2] < well[0], "full {full}: between the cap column and the label: {t:?}");
        }
        assert_eq!(f.tick_inks[1], DARK.text_muted, "the 0 dB tick");
        // Negative control: every other tick is `line.strong`, never the 0 dB ink.
        assert!(f.tick_inks.iter().enumerate().all(|(i, c)| (i == 1) == (*c == DARK.text_muted)));
    }
}

/// Whether `q` lies inside the strip `r`'s rounded rect less its 1 px border (radius 7).
fn inside_corners(r: [f32; 4], q: [f32; 2]) -> bool {
    let (rad, c) = (crate::tokens::radius::PANEL, crate::tokens::radius::PANEL - 1.0);
    let cx = if q[0] < r[0] + rad {
        r[0] + rad
    } else if q[0] > r[2] - rad {
        r[2] - rad
    } else {
        q[0]
    };
    let cy = if q[1] < r[1] + rad {
        r[1] + rad
    } else if q[1] > r[3] - rad {
        r[3] - rad
    } else {
        q[1]
    };
    ((q[0] - cx).powi(2) + (q[1] - cy).powi(2)).sqrt() <= c + 1e-3
}

#[test]
fn the_color_bar_stays_inside_the_strips_rounded_corners_in_both_views() {
    let (mut app, ctx, mut time, a) = mixer_app();
    app.run("track.new", json!({"kind": "master"})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    for full in [false, true] {
        if full {
            full_mixer(&mut app, &ctx, &mut time);
        }
        let strips: Vec<_> = app.mixer.layout.strips.iter().chain(app.mixer.layout.master.iter()).cloned().collect();
        assert!(strips.len() >= 2, "a channel and the main bus");
        for s in strips {
            let bar = s.color_bar;
            assert!(s.color_bar_outline.len() >= 4, "{}: an outline", s.name);
            for q in &s.color_bar_outline {
                assert!(inside_corners(s.rect, *q), "full {full}, {}: {q:?} lies outside the corner radius", s.name);
                assert!(q[1] >= bar[1] - 1e-3 && q[1] <= bar[3] + 1e-3, "{}: within the bar's rows", s.name);
            }
            let top = s.color_bar_outline.iter().filter(|q| (q[1] - bar[1]).abs() < 1e-3).map(|q| q[0]);
            let (l, r) = top.fold((f32::MAX, f32::MIN), |(l, r), x| (l.min(x), r.max(x)));
            assert!(
                (l - (s.rect[0] + 8.0)).abs() < 1e-3 && (r - (s.rect[2] - 8.0)).abs() < 1e-3,
                "{}: the top row starts where the curve does",
                s.name
            );
            // Negative control: the bar's plain rect (what a 3 px rect with a clamped radius paints)
            // has its corners outside the radius.
            assert!(!inside_corners(s.rect, [bar[0], bar[1]]) && !inside_corners(s.rect, [bar[2], bar[1]]), "{}", s.name);
        }
        let _ = a;
    }
}
