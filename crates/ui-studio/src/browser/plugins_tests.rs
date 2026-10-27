use super::*;

/// Built-in instrument and effect names from `engine.plugins`: an instrument the test's
/// instrument track does not already play.
fn builtins(t: &mut T) -> (String, String) {
    let b = t.app.builtin_plugins();
    let inst = t.track(TrackKind::Instrument);
    let current = t.app.engine.session().track(inst).and_then(|tr| tr.instrument.as_ref()).map(|i| i.plugin.clone());
    let i = b.iter().find(|p| p.instrument && Some(&p.id) != current.as_ref()).unwrap();
    (i.name.clone(), b.iter().find(|p| !p.instrument).unwrap().name.clone())
}

#[test]
fn an_instrument_dragged_onto_an_instrument_track_loads_it() {
    let mut t = T::new();
    let (inst_name, _) = builtins(&mut t);
    t.click(mid(t.app.browser.layout.tab_plugins));
    let inst = t.track(TrackKind::Instrument);
    let u0 = t.undo_len();
    let row = t.plugin(&inst_name);
    assert!(row.instrument && row.format == "Built-in");
    let lane = t.lane(inst);
    t.drag(mid(row.rect.unwrap()), (t.x_of(t.bar(2)), (lane[1] + lane[3]) / 2.0), json!({}));
    let got = t.app.engine.session().track(inst).unwrap().instrument.as_ref().map(|i| i.plugin.clone());
    assert_eq!(got, Some(row.id.clone()), "{}", t.app.status);
    assert_eq!(t.undo_len(), u0 + 1);
    assert!(t.app.status.starts_with("Loaded"), "{}", t.app.status);
    // An instrument onto an audio track is refused.
    let audio = t.track(TrackKind::Audio);
    let lane = t.lane(audio);
    t.drag(mid(t.plugin(&inst_name).rect.unwrap()), (t.x_of(t.bar(2)), (lane[1] + lane[3]) / 2.0), json!({}));
    assert!(t.app.status.contains("is an instrument"), "{}", t.app.status);
    assert_eq!(t.undo_len(), u0 + 1);
}

#[test]
fn an_instrument_dragged_below_the_tracks_makes_an_instrument_track_in_one_undo_step() {
    let mut t = T::new();
    let (inst_name, _) = builtins(&mut t);
    t.click(mid(t.app.browser.layout.tab_plugins));
    let n = t.app.engine.session().tracks.len();
    let u0 = t.undo_len();
    let last = t.app.arrange.layout.tracks.last().unwrap().lane;
    let row = t.plugin(&inst_name);
    t.drag(mid(row.rect.unwrap()), (t.x_of(t.bar(2)), last[3] + 30.0), json!({}));
    let s = t.app.engine.session();
    assert_eq!(s.tracks.len(), n + 1, "{}", t.app.status);
    let new = s.tracks.last().unwrap();
    assert_eq!(new.kind, TrackKind::Instrument);
    assert_eq!(new.instrument.as_ref().map(|i| i.plugin.clone()), Some(row.id));
    assert_eq!(t.undo_len(), u0 + 1);
}

#[test]
fn an_effect_dragged_onto_a_mixer_strip_or_a_lane_is_inserted() {
    let mut t = T::new();
    let (_, fx_name) = builtins(&mut t);
    t.app.ui.show_mixer = true;
    t.frames(3);
    t.click(mid(t.app.browser.layout.tab_plugins));
    let audio = t.track(TrackKind::Audio);
    let u0 = t.undo_len();
    let strip = t.app.mixer.layout.strips.iter().find(|s| s.track == audio.0).unwrap().rect;
    let row = t.plugin(&fx_name);
    assert!(!row.instrument);
    t.drag(mid(row.rect.unwrap()), mid(strip), json!({}));
    let ins = |t: &T| t.app.engine.session().track(audio).unwrap().mixer.inserts.iter().flatten().map(|i| i.plugin.clone()).collect::<Vec<_>>();
    assert_eq!(ins(&t), vec![row.id.clone()], "{}", t.app.status);
    assert_eq!(t.undo_len(), u0 + 1);
    // Onto the lane: the end of the chain.
    let lane = t.lane(audio);
    t.drag(mid(t.plugin(&fx_name).rect.unwrap()), (t.x_of(t.bar(2)), (lane[1] + lane[3]) / 2.0), json!({}));
    assert_eq!(ins(&t).len(), 2);
    assert_eq!(t.undo_len(), u0 + 2);
    // Below the tracks: refused.
    t.app.ui.show_mixer = false;
    t.frames(3);
    let last = t.app.arrange.layout.tracks.last().unwrap().lane;
    t.drag(mid(t.plugin(&fx_name).rect.unwrap()), (t.x_of(t.bar(2)), last[3] + 30.0), json!({}));
    assert!(t.app.status.contains("Drop"), "{}", t.app.status);
    assert_eq!(t.undo_len(), u0 + 2);
}

#[test]
fn a_double_click_on_a_plugin_acts_on_the_focused_track() {
    let mut t = T::new();
    let (_, fx_name) = builtins(&mut t);
    t.click(mid(t.app.browser.layout.tab_plugins));
    let audio = t.track(TrackKind::Audio);
    // Focus the audio track by clicking its header.
    let hdr = t.app.arrange.layout.tracks.iter().find(|r| r.id == audio.0).unwrap().header;
    t.click((hdr[0] + 30.0, (hdr[1] + hdr[3]) / 2.0));
    let u0 = t.undo_len();
    t.double_click(mid(t.plugin(&fx_name).rect.unwrap()));
    assert_eq!(t.app.engine.session().track(audio).unwrap().mixer.inserts.iter().flatten().count(), 1, "{}", t.app.status);
    assert_eq!(t.undo_len(), u0 + 1);
    assert!(t.plugin(&fx_name).selected);
}

#[test]
fn the_star_keeps_favorites_in_ui_state_and_the_filter_narrows_the_list() {
    let mut t = T::new();
    let (inst_name, fx_name) = builtins(&mut t);
    t.click(mid(t.app.browser.layout.tab_plugins));
    let u0 = t.undo_len();
    let dirty = t.app.engine.is_dirty();
    let row = t.plugin(&fx_name);
    t.click(mid(row.star.unwrap()));
    assert_eq!(t.app.ui.browser.favorites, vec![row.id.clone()]);
    // After the scan note: Favorites first.
    let l: Vec<_> = t.app.browser.layout.plugins.iter().filter(|p| p.row != "note").cloned().collect();
    assert_eq!((l[0].row, l[0].name.as_str()), ("heading", "Favorites"), "favorites first");
    assert_eq!(l[1].name, fx_name);
    assert!(l[1].favorite);
    // Unstar from the Favorites row.
    t.click(mid(l[1].star.unwrap()));
    assert!(t.app.ui.browser.favorites.is_empty());
    // Filter.
    t.click(mid(t.app.browser.layout.filter));
    t.text(&inst_name);
    let names: Vec<String> = t.app.browser.layout.plugins.iter().filter(|p| p.row == "plugin").map(|p| p.name.clone()).collect();
    assert!(names.contains(&inst_name) && !names.contains(&fx_name), "{names:?}");
    assert_eq!((t.undo_len(), t.app.engine.is_dirty()), (u0, dirty), "view state only");
    assert_eq!(serde_json::to_value(&t.app.ui).unwrap()["browser"]["favorites"], json!([]));
}
