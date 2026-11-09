use super::*;

#[test]
fn an_empty_grid_click_preserves_where_play_starts_and_clears_note_selection() {
    let mut t = T::open();
    t.add(60, 0, 480);
    t.click(mid(t.note_rect(60, 0)));
    assert_eq!(t.selected(), 1);
    t.app.set_cursor(12_000, Some(t.track));
    t.app.engine.transport.position = 6_000;
    assert!(!t.app.is_playing());
    let undo = t.undo_len();

    t.click(t.at(960.0, 64));

    assert_eq!(t.app.cursor(), 12_000, "empty grid must not change where Play starts");
    assert_eq!(t.app.engine.transport.position, 6_000);
    assert_eq!(t.app.position(), 12_000, "the stopped transport readout stays at the cursor");
    assert_eq!(t.app.keys.click_tick, Some(960));
    assert_eq!(t.app.cursor_track(), Some(t.track));
    assert!(t.app.keys.selection.is_empty());
    assert_eq!(t.selected(), 0);
    assert_eq!(t.undo_len(), undo);
}

#[test]
fn a_piano_roll_ruler_click_moves_the_edit_cursor_and_stopped_playhead() {
    let mut t = T::open();
    t.add(60, 0, 480);
    t.click(mid(t.note_rect(60, 0)));
    t.app.set_cursor(12_000, Some(t.track));
    t.app.engine.transport.position = 6_000;
    assert!(!t.app.is_playing());
    let ruler = t.app.keys.layout.ruler;
    let x = t.at(960.0, 64).0;

    t.click((x, mid(ruler).1));

    let s = t.app.engine.session();
    let wanted = s.tempo.tick_to_samples(960, s.sample_rate);
    assert_eq!(t.app.cursor(), wanted);
    assert_eq!(t.app.engine.transport.position, wanted);
    assert_eq!(t.selected(), 1, "ruler clicks preserve the note selection");
}

#[test]
fn step_entry_after_an_empty_grid_click_starts_at_the_clicked_tick() {
    let mut t = T::open();
    t.app.set_cursor(12_000, Some(t.track));
    t.app.engine.transport.position = 6_000;
    t.click(t.at(960.0, 64));
    assert_eq!(t.app.cursor(), 12_000);
    assert_eq!(t.app.engine.transport.position, 6_000);
    t.step_on();

    assert_eq!(t.app.keys.layout.step_cursor, 960);
    t.app.step_midi_at(transit_playback::PlayedNote { pitch: 64, velocity: 90, on: true }, 0);
    t.app.step_midi_at(transit_playback::PlayedNote { pitch: 64, velocity: 0, on: false }, 100);

    let notes = t.notes();
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].start, notes[0].pitch), (960, 64));
    assert_eq!(t.app.engine.transport.position, 6_000);
    assert_eq!(t.app.cursor(), 12_000);
}

#[test]
fn step_entry_without_a_grid_click_starts_at_the_playhead_inside_the_clip() {
    let mut t = T::open();
    t.app.set_cursor(12_000, Some(t.track));
    assert_eq!(t.app.keys.click_tick, None);
    assert_eq!(t.app.engine.session().edit.selection.start, 0);

    t.step_on();

    assert_eq!(t.app.keys.layout.step_cursor, 480);
}

#[test]
fn switching_the_open_clip_clears_the_grid_click() {
    let mut t = T::open();
    let other = t.app.run("midi.new_clip", json!({"tracks": [t.track.0], "start": 96_000, "end": 192_000})).unwrap();
    let other = ClipId(other["clip"].as_u64().unwrap());
    assert_ne!(other, t.clip);
    t.click(t.at(960.0, 64));
    assert_eq!(t.app.keys.click_tick, Some(960));
    assert!(t.app.open_keys(t.clip));
    assert_eq!(t.app.keys.click_tick, Some(960), "reopening the same clip keeps its marker");

    assert!(t.app.open_keys(other));

    assert_eq!(t.app.keys.clip, Some(other));
    assert_eq!(t.app.keys.click_tick, None);
}

#[test]
fn turning_step_off_forgets_the_grid_click() {
    let mut t = T::open();
    t.app.set_cursor(12_000, Some(t.track));
    t.click(t.at(960.0, 64));
    t.step_on();
    assert_eq!(t.app.keys.layout.step_cursor, 960);

    t.key("S", json!({}));

    assert!(!t.app.keys.step.on);
    assert_eq!(t.app.keys.click_tick, None);
    t.step_on();
    assert_eq!(t.app.keys.layout.step_cursor, 480, "the cleared click no longer overrides the playhead");
}

#[test]
fn the_arrangement_selection_takes_precedence_over_the_grid_click_for_step_entry() {
    let mut t = T::open();
    t.app.run("edit.select", json!({"tracks": [t.track.0], "start": 36_000, "end": 48_000})).unwrap();
    t.click(t.at(960.0, 64));

    t.step_on();

    assert_eq!(t.app.keys.layout.step_cursor, 1440);
}

#[test]
fn a_grid_click_before_a_later_clip_starts_step_entry_at_the_clip_start() {
    let mut t = T::open();
    let other = t.app.run("midi.new_clip", json!({"tracks": [t.track.0], "start": 96_000, "end": 192_000})).unwrap();
    t.clip = ClipId(other["clip"].as_u64().unwrap());
    assert!(t.app.open_keys(t.clip));
    t.frames(2);
    t.app.keys.scroll = -960.0;
    t.frames(2);
    t.app.set_cursor(120_000, Some(t.track));
    t.app.engine.transport.position = 108_000;

    t.click(t.at(-480.0, 64));

    assert_eq!(t.app.keys.click_tick, Some(-480));
    assert_eq!(t.app.cursor(), 120_000);
    assert_eq!(t.app.engine.transport.position, 108_000);
    t.step_on();
    assert_eq!(t.app.keys.layout.step_cursor, 0);
    t.app.step_midi_at(transit_playback::PlayedNote { pitch: 64, velocity: 90, on: true }, 0);
    t.app.step_midi_at(transit_playback::PlayedNote { pitch: 64, velocity: 0, on: false }, 100);
    assert_eq!(t.notes().len(), 1);
    assert_eq!(t.notes()[0].start, 0);
}

#[test]
fn a_shift_grid_click_keeps_the_note_selection_and_marks_step_entry() {
    let mut t = T::open();
    t.add(60, 0, 480);
    t.click(mid(t.note_rect(60, 0)));
    t.app.set_cursor(12_000, Some(t.track));
    t.app.engine.transport.position = 6_000;
    let undo = t.undo_len();

    t.click_n(t.at(960.0, 64), 1, json!({"shift": true}));

    assert_eq!(t.selected(), 1);
    assert_eq!(t.app.keys.click_tick, Some(960));
    assert_eq!(t.app.cursor(), 12_000);
    assert_eq!(t.app.engine.transport.position, 6_000);
    assert_eq!(t.undo_len(), undo);
}

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
    assert!(t.app.ui.show_devices && t.app.ui.show_mixer, "Devices joins the Mixer");
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
    // Devices tab: the piano roll closes and the device chain shows; the Mixer pane is
    // independent and stays as it was (shown), so both panes come back.
    t.click(mid(l.tab_devices));
    assert!(!t.app.keys.layout.visible && t.app.ui.show_devices && t.app.ui.show_mixer);
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

fn have(r: [f32; 4]) -> bool {
    r[2] > r[0] && r[3] > r[1]
}

#[test]
fn a_click_on_the_active_mixer_tab_hides_the_pane_and_the_strip_stays() {
    let mut t = T::new();
    assert!(t.app.ui.show_mixer && !t.app.ui.show_devices);
    t.click(mid(t.app.keys.switcher[1]));
    assert!(!t.app.ui.show_mixer && !t.app.ui.show_devices, "the active tab toggles its pane off");
    assert!(!t.app.mixer.layout.visible);
    assert!(t.app.keys.switcher.iter().all(|r| have(*r)), "the strip stays with nothing shown: {:?}", t.app.keys.switcher);
    // One click brings it back.
    t.click(mid(t.app.keys.switcher[1]));
    assert!(t.app.ui.show_mixer && t.app.mixer.layout.visible);
}

#[test]
fn the_mixer_and_devices_tabs_toggle_independently_and_both_panes_show() {
    let mut t = T::new();
    assert!(t.app.ui.show_mixer && !t.app.ui.show_devices);
    // Devices while the Mixer is shown: both on, both drawn, both tabs lit.
    t.click(mid(t.app.keys.switcher[2]));
    assert!(t.app.ui.show_mixer && t.app.ui.show_devices, "the Mixer stays when Devices shows");
    assert!(t.app.mixer.layout.visible && t.app.mixer.devices.visible, "both panes drawn");
    assert!(have(t.app.mixer.layout.area) && have(t.app.mixer.devices.area));
    // The active Devices tab hides only its own pane.
    t.click(mid(t.app.keys.switcher[2]));
    assert!(t.app.ui.show_mixer && !t.app.ui.show_devices);
    // From nothing shown, two clicks show both; the strip stayed in between.
    t.click(mid(t.app.keys.switcher[1]));
    assert!(!t.app.ui.show_mixer && !t.app.ui.show_devices);
    assert!(t.app.keys.switcher.iter().all(|r| have(*r)), "the strip stays with nothing shown");
    t.click(mid(t.app.keys.switcher[2]));
    t.click(mid(t.app.keys.switcher[1]));
    assert!(t.app.ui.show_mixer && t.app.ui.show_devices, "both tabs on after two clicks");
    assert!(t.app.mixer.layout.visible && t.app.mixer.devices.visible);
}

/// The View menu entry `label`, from the menu bar (open the View menu first).
fn view_item(t: &T, label: &str) -> [f32; 4] {
    t.app.arrange.bar_menu.iter().find(|m| m.label == label).unwrap_or_else(|| panic!("no {label}: {:?}", t.app.arrange.bar_menu)).rect
}

#[test]
fn the_view_menu_and_shortcuts_show_both_panes_and_hide_only_their_own() {
    let mut t = T::new();
    assert!(t.app.ui.show_mixer && !t.app.ui.show_devices);
    // View > Devices while the Mixer is shown, by clicks: both shown.
    t.click(mid(view_item(&t, "View")));
    t.click(mid(view_item(&t, "Devices")));
    assert!(t.app.ui.show_mixer && t.app.ui.show_devices, "View > Devices keeps the Mixer");
    // The menu offers both entries (checked from the same two flags) and View > Mixer hides only it.
    t.click(mid(view_item(&t, "View")));
    t.click(mid(view_item(&t, "Mixer")));
    assert!(!t.app.ui.show_mixer && t.app.ui.show_devices, "View > Mixer hides only the Mixer");
    // The shortcuts: Ctrl+Alt+M shows the Mixer beside Devices; Ctrl+Alt+L hides only Devices.
    t.key("M", json!({"cmd": true, "alt": true}));
    assert!(t.app.ui.show_mixer && t.app.ui.show_devices);
    t.key("L", json!({"cmd": true, "alt": true}));
    assert!(t.app.ui.show_mixer && !t.app.ui.show_devices);
    t.key("L", json!({"cmd": true, "alt": true}));
    assert!(t.app.ui.show_mixer && t.app.ui.show_devices);
}

#[test]
fn a_ui_json_with_both_panes_on_loads_both() {
    let ui: crate::UiState = serde_json::from_value(json!({"show_mixer": true, "show_devices": true})).unwrap();
    assert!(ui.show_mixer && ui.show_devices, "both stay on");
    let saved = serde_json::to_value(&ui).unwrap();
    assert!(saved["show_mixer"] == true && saved["show_devices"] == true);
    let ui: crate::UiState = serde_json::from_value(json!({"show_mixer": false, "show_devices": true})).unwrap();
    assert!(!ui.show_mixer && ui.show_devices);
}

#[test]
fn opening_a_device_card_from_the_mixer_shows_devices_and_leaves_the_mixer() {
    let mut t = T::new();
    assert!(t.app.ui.show_mixer && !t.app.ui.show_devices);
    crate::mixer::inserts::open_card(&mut t.app, t.track);
    assert!(t.app.ui.show_devices && t.app.ui.show_mixer, "devices shown, mixer untouched");
    t.app.ui.show_mixer = false;
    crate::mixer::inserts::open_card(&mut t.app, t.track);
    assert!(t.app.ui.show_devices && !t.app.ui.show_mixer, "a hidden mixer stays hidden");
}

/// Every right-hand header control, the Length field, and the two labels, as `(name, rect)`.
fn header_rects(l: &crate::keys::KeysLayout) -> Vec<(&'static str, [f32; 4])> {
    vec![
        ("step", l.step),
        ("step_velocity", l.step_velocity),
        ("audition", l.audition),
        ("quantize", l.quantize),
        ("pitch", l.pitch),
        ("snap", l.snap),
        ("use_selected_velocity", l.use_selected_velocity),
        ("new_note_velocity", l.new_note_velocity),
        ("note_length", l.note_length),
        ("new_note_velocity_label", l.new_note_velocity_label),
        ("note_length_label", l.note_length_label),
    ]
}

/// Every header control is the same height on one center line, within a physical pixel.
fn aligned(l: &crate::keys::KeysLayout, ppp: f32, narrow: bool) -> Result<(), String> {
    let px = 1.0 / ppp;
    let all = header_rects(l);
    let controls: Vec<_> = all.iter().filter(|(name, _)| !name.ends_with("_label")).filter(|(name, _)| !narrow || *name == "note_length").collect();
    let want_h = crate::tokens::layout::DENSE_CONTROL_HEIGHT;
    let want_cy = controls[0].1;
    let want_cy = (want_cy[1] + want_cy[3]) / 2.0;
    for (name, r) in
        controls.iter().copied().copied().chain(all.iter().copied().filter(|(n, _)| n.ends_with("_label") && (!narrow || *n == "note_length_label")))
    {
        let (h, cy) = (r[3] - r[1], (r[1] + r[3]) / 2.0);
        if !name.ends_with("_label") && (h - want_h).abs() > px {
            return Err(format!("{name}: height {h}, want {want_h}"));
        }
        if (cy - want_cy).abs() > px {
            return Err(format!("{name}: center {cy}, want {want_cy}"));
        }
    }
    Ok(())
}

/// Every rectangle painted in the header row (the frames and fills of the controls), flattened in
/// paint order: its rect, fill and stroke. The row's own background and the tabs are left out.
fn header_rects_painted(t: &T) -> Vec<(egui::Rect, egui::Color32, egui::Stroke, egui::CornerRadius)> {
    fn flat<'a>(s: &'a egui::Shape, out: &mut Vec<&'a egui::Shape>) {
        if let egui::Shape::Vec(v) = s {
            v.iter().for_each(|s| flat(s, out));
        } else {
            out.push(s);
        }
    }
    let l = &t.app.keys.layout;
    let cy = (l.note_length[1] + l.note_length[3]) / 2.0;
    let mut all = Vec::new();
    t.shapes.iter().for_each(|s| flat(&s.shape, &mut all));
    all.into_iter()
        .filter_map(|s| match s {
            egui::Shape::Rect(r) if r.rect.height() < 40.0 && r.rect.width() < 300.0 && (r.rect.center().y - cy).abs() < 16.0 => {
                Some((r.rect, r.fill, r.stroke, r.corner_radius))
            }
            _ => None,
        })
        .collect()
}

/// The header's PAINTED band, read from the frame's shapes: the pane's top line and the line
/// under the header, the two full-width horizontal lines inside the band's height. Returns the
/// band's top and bottom y (#412, E78: controls are measured against this, not the row the
/// layout hands the header).
fn painted_band(t: &T) -> Option<(f32, f32)> {
    fn flat<'a>(s: &'a egui::Shape, out: &mut Vec<&'a egui::Shape>) {
        if let egui::Shape::Vec(v) = s {
            v.iter().for_each(|s| flat(s, out));
        } else {
            out.push(s);
        }
    }
    let pane = t.app.keys.layout.pane;
    let mut all = Vec::new();
    t.shapes.iter().for_each(|s| flat(&s.shape, &mut all));
    let ys: Vec<f32> = all
        .into_iter()
        .filter_map(|s| match s {
            egui::Shape::LineSegment { points, stroke }
                if stroke.color == crate::theme::TOKENS.line
                    && (points[0].y - points[1].y).abs() < 0.01
                    && (points[1].x - points[0].x) >= pane[2] - pane[0] - 1.0
                    && points[0].y >= pane[1]
                    && points[0].y <= pane[1] + crate::keys::HEADER_BAND_H =>
            {
                Some(points[0].y)
            }
            _ => None,
        })
        .collect();
    let top = ys.iter().copied().fold(f32::INFINITY, f32::min);
    let bottom = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    (ys.len() >= 2).then_some((top, bottom + 0.5))
}

/// Every wide-header control is centered on the painted band, within a physical pixel.
fn centered_on_band(t: &T, ppp: f32) -> Result<(), String> {
    let (top, bottom) = painted_band(t).ok_or("no painted band (top and bottom lines) found")?;
    let cy = (top + bottom) / 2.0;
    let px = 1.0 / ppp;
    for (name, r) in header_rects(&t.app.keys.layout).into_iter().filter(|(n, _)| *n != "note_length" || t.width >= 1200.0) {
        let c = (r[1] + r[3]) / 2.0;
        if (c - cy).abs() > px {
            return Err(format!("{name}: center {c}, painted band {top}..{bottom} center {cy}"));
        }
    }
    Ok(())
}

#[test]
fn piano_roll_header_controls_share_one_height_and_center_line() {
    for ppp in [1.0f32, 1.5, 2.0] {
        for narrow in [false, true] {
            let mut t = T::open();
            t.ctx.set_pixels_per_point(ppp);
            if narrow {
                t.width = 1024.0;
            }
            t.add(60, 480, 480);
            t.click(mid(t.note_rect(60, 480)));
            t.frames(3);
            let l = &t.app.keys.layout;
            aligned(l, ppp, narrow).unwrap_or_else(|e| panic!("ppp {ppp} narrow {narrow}: {e}"));
            if !narrow {
                centered_on_band(&t, ppp).unwrap_or_else(|e| panic!("ppp {ppp}: {e}"));
            }
            painted_aligned(&header_rects_painted(&t), l, ppp, narrow).unwrap_or_else(|e| panic!("painted, ppp {ppp} narrow {narrow}: {e}"));
        }
    }
}

/// The PAINTED frames of the header's two fields, not their allocations (#412, E78): every
/// visible rectangle over a field is [`HEADER_CONTROL_H`](crate::tokens::layout::DENSE_CONTROL_HEIGHT)
/// high on the field's center line, and each field has the kit's inset frame with its 1 px line.
fn painted_aligned(
    painted: &[(egui::Rect, egui::Color32, egui::Stroke, egui::CornerRadius)],
    l: &crate::keys::KeysLayout,
    ppp: f32,
    narrow: bool,
) -> Result<(), String> {
    let px = 1.0 / ppp;
    let want_h = crate::tokens::layout::DENSE_CONTROL_HEIGHT;
    let fields = [("note_length", l.note_length), ("new_note_velocity", l.new_note_velocity)];
    for (name, f) in fields.into_iter().filter(|(n, _)| !narrow || *n == "note_length") {
        let cy = (f[1] + f[3]) / 2.0;
        let over: Vec<_> = painted
            .iter()
            .filter(|(r, fill, stroke, _)| (fill.a() > 0 || stroke.width > 0.0) && r.min.x >= f[0] - 2.0 && r.max.x <= f[2] + 2.0 && r.width() > 20.0)
            .collect();
        for (r, fill, _, radius) in &over {
            if fill.a() > 0 && *radius != egui::CornerRadius::same(crate::tokens::chrome::RADIUS_CONTROL as u8) {
                return Err(format!("{name}: corner radius {radius:?}, want the kit's control radius"));
            }
            if (r.height() - want_h).abs() > px || (r.center().y - cy).abs() > px {
                return Err(format!("{name}: painted frame h {} center {}, want {want_h} at {cy}", r.height(), r.center().y));
            }
        }
        if !over.iter().any(|(_, fill, stroke, _)| fill.a() > 0 && stroke.width >= 1.0) {
            return Err(format!("{name}: no kit field frame painted"));
        }
    }
    Ok(())
}

#[test]
fn the_painted_check_fails_on_a_frame_shorter_than_its_allocation() {
    // The reported symptom: the allocation is 24 high, the painted frame about three quarters.
    let l =
        crate::keys::KeysLayout { note_length: [560.0, 446.0, 654.0, 470.0], new_note_velocity: [768.0, 446.0, 812.0, 470.0], ..Default::default() };
    let frame = |r: egui::Rect| {
        (
            r,
            crate::theme::TOKENS.surface_inset,
            egui::Stroke::new(1.0, crate::theme::TOKENS.line),
            egui::CornerRadius::same(crate::tokens::chrome::RADIUS_CONTROL as u8),
        )
    };
    let rect = |x0: f32, x1: f32, h: f32| egui::Rect::from_center_size(egui::pos2((x0 + x1) / 2.0, 458.0), egui::vec2(x1 - x0, h));
    let good = [frame(rect(560.0, 654.0, 24.0)), frame(rect(768.0, 812.0, 24.0))];
    assert!(aligned_alloc_ok(&l) && painted_aligned(&good, &l, 1.0, false).is_ok());
    let short = [frame(rect(560.0, 654.0, 18.0)), frame(rect(768.0, 812.0, 24.0))];
    assert!(aligned_alloc_ok(&l), "the allocation check passes on it, which is the escape");
    assert!(painted_aligned(&short, &l, 1.0, false).is_err());
}

fn aligned_alloc_ok(l: &crate::keys::KeysLayout) -> bool {
    [l.note_length, l.new_note_velocity].iter().all(|r| (r[3] - r[1] - crate::tokens::layout::DENSE_CONTROL_HEIGHT).abs() < 0.01)
}

#[test]
fn the_check_fails_on_the_pre_fix_sizing() {
    // Measured before the fix (build a007ee27): buttons and Length 20 high at cy 458, the
    // velocity box 22 high at cy 459, the labels 13 high.
    let mut l = crate::keys::KeysLayout::default();
    for r in [
        &mut l.step,
        &mut l.step_velocity,
        &mut l.audition,
        &mut l.quantize,
        &mut l.pitch,
        &mut l.snap,
        &mut l.use_selected_velocity,
        &mut l.note_length,
    ] {
        *r = [0.0, 448.0, 50.0, 468.0];
    }
    l.new_note_velocity = [0.0, 448.0, 44.0, 470.0];
    l.new_note_velocity_label = [0.0, 451.5, 80.0, 464.5];
    l.note_length_label = [0.0, 451.5, 40.0, 464.5];
    assert!(aligned(&l, 1.0, false).is_err());
}

#[test]
fn every_header_control_still_dispatches_its_command() {
    let mut t = T::open();
    t.add(60, 480, 480);
    t.click(mid(t.note_rect(60, 480)));
    let vel = t.notes()[0].velocity;
    t.app.ui.keys.velocity = if vel == 50 { 51 } else { 50 };
    t.click(mid(t.app.keys.layout.use_selected_velocity));
    assert_eq!(t.app.ui.keys.velocity, vel, "the eyedropper takes the selected note's velocity");
    let fixed = t.app.ui.keys.step_fixed;
    t.click(mid(t.app.keys.layout.step_velocity));
    assert_eq!(t.app.ui.keys.step_fixed, !fixed, "the step velocity button toggles Played and Fixed");
    let audition = t.app.ui.keys.audition;
    t.click(mid(t.app.keys.layout.audition));
    assert_eq!(t.app.ui.keys.audition, !audition);
    t.click(mid(t.app.keys.layout.step));
    assert!(t.app.keys.layout.step_on, "Step turns step entry on");
    t.click(mid(t.app.keys.layout.quantize));
    assert!(t.app.keys.quantize_open, "Quantize opens its panel");
    t.app.keys.quantize_open = false;
    t.click(mid(t.app.keys.layout.pitch));
    t.frames(2);
    assert!(!t.app.keys.layout.pitch_menu.is_empty(), "Pitch opens its menu");
    t.key("Escape", json!({}));
    t.click(mid(t.app.keys.layout.snap));
    t.frames(2);
    assert!(!t.app.keys.layout.snap_menu.is_empty(), "Grid opens its list");
}

#[test]
fn the_length_field_frame_is_the_button_height_in_every_state() {
    let want = crate::tokens::layout::DENSE_CONTROL_HEIGHT;
    for ppp in [1.0f32, 1.5, 2.0] {
        for narrow in [false, true] {
            let mut t = T::open();
            t.ctx.set_pixels_per_point(ppp);
            if narrow {
                t.width = 1024.0;
            }
            t.add(60, 480, 480);
            t.frames(3);
            for state in ["no selection", "with a value", "focused"] {
                match state {
                    "with a value" => {
                        t.click(mid(t.note_rect(60, 480)));
                        t.frames(3);
                    }
                    "focused" => t.click(mid(t.app.keys.layout.note_length)),
                    _ => {}
                }
                let l = t.app.keys.layout.clone();
                let px = 1.0 / ppp;
                let f = l.note_length;
                assert!((f[3] - f[1] - want).abs() <= px, "ppp {ppp} narrow {narrow} {state}: allocation {}", f[3] - f[1]);
                painted_aligned(&header_rects_painted(&t), &l, ppp, narrow).unwrap_or_else(|e| panic!("ppp {ppp} narrow {narrow} {state}: {e}"));
                if !narrow {
                    let b = l.step;
                    assert!(((f[3] - f[1]) - (b[3] - b[1])).abs() <= px, "ppp {ppp} {state}: Length vs button height");
                    centered_on_band(&t, ppp).unwrap_or_else(|e| panic!("ppp {ppp} {state}: {e}"));
                }
            }
        }
    }
}
