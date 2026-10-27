use super::*;

#[test]
fn the_view_switcher_shows_in_the_piano_roll_the_mixer_and_the_devices() {
    let mut t = T::new();
    let have = |r: [f32; 4]| r[2] > r[0] && r[3] > r[1];
    // Mixer view.
    assert!(t.app.ui.show_mixer && !t.app.ui.show_devices);
    let sw = t.app.keys.switcher;
    assert!(sw.iter().all(|r| have(*r)), "mixer view: {sw:?}");
    // The piano roll view carries the same three tabs.
    let r = t.app.arrange.layout.clips.iter().find(|c| c.id == t.clip.0).unwrap().rect;
    t.click_n(mid(r), 2, json!({}));
    let l = t.app.keys.layout.clone();
    assert!(l.visible && [l.tab_keys, l.tab_mixer, l.tab_devices].iter().all(|r| have(*r)), "{l:?}");
    // Back to the mixer through its tab, then Devices through the strip.
    t.click(mid(l.tab_mixer));
    assert!(t.app.mixer.layout.visible);
    t.click(mid(t.app.keys.switcher[2]));
    assert!(t.app.ui.show_devices && !t.app.ui.show_mixer);
    assert!(t.app.keys.switcher.iter().all(|r| have(*r)), "devices view");
    // Piano Roll from the strip reopens the clip.
    t.click(mid(t.app.keys.switcher[0]));
    assert!(t.app.keys.layout.visible && t.app.keys.layout.clip == Some(t.clip.0));
}

#[test]
fn a_double_click_on_a_midi_clip_opens_the_piano_roll_and_tabs_shortcut_and_escape_switch_back() {
    let mut t = T::new();
    assert!(t.app.mixer.layout.visible, "the mixer pane to start with");
    let undo = t.undo_len();
    let r = t.app.arrange.layout.clips.iter().find(|c| c.id == t.clip.0).unwrap().rect;
    t.click_n(mid(r), 2, json!({}));
    let l = t.app.keys.layout.clone();
    assert!(l.visible && l.clip == Some(t.clip.0) && l.track == Some(t.track.0), "{l:?}");
    assert!(!t.app.mixer.layout.visible, "the piano roll replaces the mixer pane");
    assert!(l.pane[1] > t.app.arrange.layout.timeline[1], "in the bottom pane");
    // Devices tab: the device chain comes back in its place.
    t.click(mid(l.tab_devices));
    assert!(!t.app.keys.layout.visible && t.app.ui.show_devices && !t.app.ui.show_mixer);
    // Ctrl+Alt+K brings the same clip back.
    t.key("K", json!({"cmd": true, "alt": true}));
    assert!(t.app.keys.layout.visible && t.app.keys.layout.clip == Some(t.clip.0));
    t.click(mid(t.app.keys.layout.tab_mixer));
    assert!(!t.app.keys.layout.visible && t.app.mixer.layout.visible, "Mixer tab");
    t.key("K", json!({"cmd": true, "alt": true}));
    assert!(t.app.keys.layout.visible);
    // Escape in the piano roll returns to the mixer that was shown before.
    t.key("Escape", json!({}));
    assert!(!t.app.keys.layout.visible && t.app.mixer.layout.visible, "Escape");
    assert_eq!(t.undo_len(), undo, "opening and closing is view state");
}

#[test]
fn the_audition_switch_and_the_keyboard_column() {
    let mut t = T::open();
    assert!(t.app.ui.keys.audition, "on by default");
    t.click(mid(t.app.keys.layout.audition));
    assert!(!t.app.ui.keys.audition);
    let before = t.app.keys.layout.auditions;
    t.click_n(t.at(10.0, 60), 2, json!({}));
    assert_eq!(t.notes().len(), 1);
    assert_eq!(t.app.keys.layout.auditions, before, "off: placing a note is silent");
    t.click(mid(t.app.keys.layout.audition));
    assert!(t.app.ui.keys.audition, "and back on");
    t.click(mid(t.note_rect(60, 0)));
    assert_eq!(t.app.keys.layout.auditions, before + 1, "clicking a note plays it");
    // A key in the keyboard column plays its pitch.
    let kb = t.app.keys.layout.keyboard;
    let (_, y) = t.at(0.0, 65);
    t.click(((kb[0] + kb[2]) / 2.0, y));
    assert_eq!(t.app.keys.layout.last_audition.map(|a| a.0), Some(65));
    assert_eq!(t.app.keys.layout.sounding, Vec::<u8>::new(), "released after the click");
}

#[test]
fn view_gestures_change_the_ui_state_and_never_the_document() {
    let mut t = T::open();
    let doc = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    // Grid menu.
    t.click(mid(t.app.keys.layout.snap));
    let item = t.app.keys.layout.snap_menu.iter().find(|m| m.label == "1/8").cloned().expect("1/8 listed");
    assert_eq!(t.app.keys.layout.snap_menu.len(), Snap::ALL.len());
    t.click(mid(item.rect));
    assert_eq!(Snap::of(&t.app.ui.keys.grid), Some(Snap::Eighth));
    // Pane height: drag the top edge up.
    let h0 = t.app.ui.keys.height;
    let hd = mid(t.app.keys.layout.handle);
    t.drag(hd, (hd.0, hd.1 - 60.0), json!({}));
    assert!((t.app.ui.keys.height - (h0 + 60.0)).abs() < 1.0, "{}", t.app.ui.keys.height);
    assert!((t.app.keys.layout.pane[3] - t.app.keys.layout.pane[1] - t.app.ui.keys.height).abs() < 1.5);
    // Ctrl+wheel zooms time; Ctrl+Shift+wheel and Alt+wheel zoom the keys.
    let g = mid(t.app.keys.layout.grid);
    let z0 = t.app.ui.keys.px_per_quarter;
    t.input("ui.scroll", json!({"x": g.0, "y": g.1, "dy": 120, "cmd": true}));
    assert!(t.app.ui.keys.px_per_quarter > z0, "Ctrl+wheel zoomed in: {}", t.app.ui.keys.px_per_quarter);
    let k0 = t.app.ui.keys.key_h;
    t.input("ui.scroll", json!({"x": g.0, "y": g.1, "dy": 120, "cmd": true, "shift": true}));
    let k1 = t.app.ui.keys.key_h;
    assert!(k1 > k0, "Ctrl+Shift+wheel: {k0} -> {k1}");
    t.input("ui.scroll", json!({"x": g.0, "y": g.1, "dy": -120, "alt": true}));
    assert!(t.app.ui.keys.key_h < k1, "Alt+wheel: {k1} -> {}", t.app.ui.keys.key_h);
    // Plain wheel scrolls the keys.
    let top = t.app.keys.layout.top_pitch;
    t.input("ui.scroll", json!({"x": g.0, "y": g.1, "dy": 80}));
    assert!(t.app.keys.layout.top_pitch > top, "wheel scrolled up");
    // Ruler drag down zooms time in; keyboard drag sideways zooms the keys.
    let z1 = t.app.ui.keys.px_per_quarter;
    let r = mid(t.app.keys.layout.ruler);
    t.drag(r, (r.0, r.1 + 40.0), json!({}));
    assert!(t.app.ui.keys.px_per_quarter > z1 * 1.3, "ruler drag: {z1} -> {}", t.app.ui.keys.px_per_quarter);
    let k2 = t.app.ui.keys.key_h;
    let kb = mid(t.app.keys.layout.keyboard);
    t.drag(kb, (kb.0 + 30.0, kb.1), json!({}));
    assert!(t.app.ui.keys.key_h > k2, "keyboard drag: {k2} -> {}", t.app.ui.keys.key_h);
    // A ruler click moves the cursor (view state too).
    let r = t.app.keys.layout.ruler;
    t.click((r[0] + 5.0, (r[1] + r[3]) / 2.0));
    assert_eq!(t.undo_len(), undo, "no undo entries");
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), doc, "the document is untouched");
    // The preferences go out with ui.inspect and come back through ui.set.
    let v = t.app.inspect(&t.ctx);
    assert_eq!(v["ui"]["keys"]["grid"]["mode"], json!({"Fixed": 480}));
    t.app.run("ui.set", json!({"keys": {"grid": {"mode": {"Fixed": 120}}, "height": 300.0, "audition": false}})).unwrap();
    assert_eq!((Snap::of(&t.app.ui.keys.grid), t.app.ui.keys.audition), (Some(Snap::ThirtySecond), false));
}

#[test]
fn the_playhead_follows_and_the_editor_stays_on_its_clip() {
    let mut t = T::open();
    t.app.keys.scroll = 20_000.0;
    t.frames(2);
    t.app.run("transport.play", json!({"from": 0})).unwrap();
    t.frames(10);
    assert!(t.app.is_playing());
    let l = &t.app.keys.layout;
    assert!(l.scroll_ticks < 20_000.0, "the view paged to the playhead: {}", l.scroll_ticks);
    assert_eq!(l.clip, Some(t.clip.0));
    t.key("Space", json!({}));
    assert!(!t.app.is_playing(), "Space still toggles the transport");
    // Undoing the clip away closes the editor.
    while t.app.engine.session().find_clip(t.clip).is_some() {
        t.app.run("edit.undo", json!({})).unwrap();
    }
    t.frames(2);
    assert!(!t.app.keys.layout.visible && t.app.keys.clip.is_none());
}

#[test]
fn shift_wheel_and_a_horizontal_wheel_pan_the_arrangement_a_tenth_of_the_view_per_notch_at_any_zoom() {
    for spp in [100.0, 1500.0] {
        pan_cases(|t, extra| {
            t.app.run("view.zoom_set", json!({"samples_per_px": spp})).unwrap();
            t.app.run("view.scroll", json!({"to": 4_000_000})).unwrap();
            t.frames(3);
            let (s0, visible) = arrange_view(t);
            let l = &t.app.arrange.layout;
            let at = ((l.timeline[0] + l.timeline[2]) / 2.0, (l.timeline[1] + l.timeline[3]) / 2.0);
            t.notch(at, extra);
            (arrange_view(t).0 - s0, visible)
        });
    }
}

#[test]
fn shift_wheel_and_a_horizontal_wheel_pan_the_piano_roll_a_tenth_of_the_view_per_notch_at_any_zoom() {
    for ppq in [60.0f32, 240.0] {
        pan_cases(|t, extra| {
            t.app.ui.keys.px_per_quarter = ppq;
            t.app.keys.scroll = 5000.0;
            t.frames(3);
            let g = t.app.keys.layout.grid;
            let visible = f64::from(g[2] - g[0]) / t.app.keys.layout.px_per_tick;
            let s0 = t.app.keys.scroll;
            t.notch(mid(g), extra);
            (t.app.keys.scroll - s0, visible)
        });
    }
}

#[test]
fn the_plain_wheel_still_scrolls_rows_and_pitches_and_never_time() {
    let mut t = T::open();
    let g = mid(t.app.keys.layout.grid);
    let (s0, top) = (t.app.keys.scroll, t.app.keys.layout.top_pitch);
    t.notch(g, json!({"dy": 1}));
    assert_eq!(t.app.keys.scroll, s0, "plain wheel does not pan time");
    assert!(t.app.keys.layout.top_pitch != top, "plain wheel scrolls pitches");
    let sc = t.app.arrange.layout.scroll;
    let l = &t.app.arrange.layout;
    let at = ((l.timeline[0] + l.timeline[2]) / 2.0, l.timeline[1] + 5.0);
    t.notch(at, json!({"dy": -1}));
    assert_eq!(t.app.arrange.layout.scroll, sc, "plain wheel does not pan the arrangement");
}

#[test]
fn a_one_bar_clip_looped_to_four_edits_as_one_bar_with_ghost_repeats() {
    let mut t = T::open();
    let bar = t.clip_ticks();
    t.add(60, 0, bar / 4);
    t.add(64, bar / 2, bar / 4);
    t.loop_to_bars(0, 4);
    let l = &t.app.keys.layout;
    assert_eq!(l.clip_ticks, bar, "the span is the cycle");
    assert_eq!((l.looped_to_ticks, l.loop_cycle, l.lead_in_ticks), (Some(4 * bar), Some((0, bar)), 0));
    // Three repeats of the two notes, each at the cycle's position.
    assert_eq!(l.loop_ghosts.len(), 6);
    for k in 1..4 {
        assert!(l.loop_ghosts.iter().any(|g| (g.pitch, g.start) == (60, k * bar)), "repeat {k}");
        assert!(l.loop_ghosts.iter().any(|g| (g.pitch, g.start) == (64, k * bar + bar / 2)), "repeat {k}");
    }
    assert!(l.loop_ghosts.iter().all(|g| g.start >= bar && g.start + g.length <= 4 * bar));
    // A note placed at bar 3 is past the loop end: outside, not part of the cycle.
    t.add(67, 2 * bar, bar / 4);
    let n = &t.app.keys.layout.notes;
    assert!(n.iter().find(|n| n.pitch == 67).unwrap().outside);
    assert!(n.iter().filter(|n| n.pitch != 67).all(|n| !n.outside));
    // The clip's content is one bar plus that note; the ghosts did not become notes.
    assert_eq!(t.notes().len(), 3);
}

#[test]
fn an_unlooped_four_bar_clip_is_unchanged_with_no_ghosts() {
    let mut t = T::open();
    let bar = t.clip_ticks();
    t.add(60, 0, bar / 4);
    let to = {
        let s = t.app.engine.session();
        s.tempo.tick_to_samples(4 * bar, s.sample_rate)
    };
    t.app.trim_clip(t.clip, false, to).unwrap();
    t.frames(5);
    let l = &t.app.keys.layout;
    assert_eq!(l.clip_ticks, 4 * bar, "the span is the arrangement length");
    assert_eq!((l.looped_to_ticks, l.loop_cycle, l.lead_in_ticks), (None, None, 0));
    assert!(l.loop_ghosts.is_empty());
}

#[test]
fn a_loop_with_a_lead_in_spans_zero_to_its_end_and_reports_the_lead_in() {
    let mut t = T::open();
    let bar = t.clip_ticks();
    t.add(60, 0, bar / 8);
    t.add(62, bar / 2, bar / 8);
    let q = bar / 4;
    t.loop_to_bars(q, 3);
    let l = &t.app.keys.layout;
    assert_eq!((l.clip_ticks, l.loop_cycle, l.lead_in_ticks, l.looped_to_ticks), (bar, Some((q, bar)), q, Some(3 * bar)));
    // The note before the cycle start is the intro: no ghost of it; the cycle's note repeats every 3/4 bar.
    let cycle = bar - q;
    assert!(l.loop_ghosts.iter().all(|g| g.pitch == 62));
    assert!(l.loop_ghosts.iter().any(|g| g.start == bar / 2 + cycle));
    assert!(l.loop_ghosts.iter().all(|g| g.start >= bar && g.start < 3 * bar));
}

#[test]
fn the_clip_length_readout_says_looped_to() {
    let bar = transit_time::TICKS_PER_QUARTER * 4;
    assert_eq!(crate::keys::clip_length_text(bar, Some(4 * bar)), "Clip length 1.00 bars, looped to 4.00");
    assert_eq!(crate::keys::clip_length_text(4 * bar, None), "Clip length 4.00 bars");
    assert_eq!(crate::keys::clip_length_text(bar, Some(bar)), "Clip length 1.00 bars");
}
