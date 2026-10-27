use super::*;
use crate::edit_window::test_support::*;
use egui_kittest::Harness;

#[test]
fn shift_and_cmd_click_on_track_names_extend_the_selection() {
    let mut h = harness(5);
    let ids = row_ids(&h);
    let (none, shift, cmd) = (egui::Modifiers::NONE, egui::Modifiers::SHIFT, egui::Modifiers::COMMAND);
    for (i, mods, want) in [(1, none, vec![1]), (3, shift, vec![1, 2, 3]), (2, cmd, vec![1, 3]), (0, cmd, vec![0, 1, 3]), (4, none, vec![4])] {
        let at = row(&h, i).min + vec2(60.0, 10.0);
        click_with(&mut h, at, mods);
        let want: Vec<TrackId> = want.into_iter().map(|k| ids[k]).collect();
        assert_eq!(h.state().engine.session().edit.selected_tracks, want, "row {i} clicked with {mods:?}");
    }
}

#[test]
fn dragging_a_name_plate_down_drops_it_where_released() {
    let mut h = harness(4);
    let order = |h: &Harness<SoundApp>| -> Vec<TrackId> { h.state().engine.session().tracks.iter().map(|t| t.id).collect() };
    let before = order(&h);
    let grab = row(&h, 0).min + vec2(40.0, 12.0);
    let release = pos2(grab.x, row(&h, 2).min.y);
    drag(&mut h, grab, release);
    let want: Vec<TrackId> = [1, 0, 2, 3].iter().map(|&i| before[i]).collect();
    assert_eq!(order(&h), want);
}
