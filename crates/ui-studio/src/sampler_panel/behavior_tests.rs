//! The Pads view's gestures through the C2 commands, each one undo step with a negative control:
//! a dropped file, a refused file (the panel's note), press to select and audition, the choke
//! picker, M and S (solo dims the others to 40 %), a drag that swaps two pads, rename, and the
//! zone's knobs, segments and toggles.

use super::test_rig::{T, mid, wav};
use serde_json::json;

#[test]
fn a_file_dropped_on_an_empty_pad_becomes_its_sample_in_one_undo_step() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let before = t.undo_len();
    // Negative control: dropped beside the grid (on the strip), nothing happens.
    let strip = t.panel(tr).strip.panel;
    t.drop_file(&snare, mid(strip));
    assert!(t.patch(tr).zones.is_empty());
    assert_eq!(t.undo_len(), before);
    t.drop_file(&snare, t.pad_at(tr, 3));
    let patch = t.patch(tr);
    assert_eq!(patch.pad(3).zone, Some(0), "A4 is bound to the lowest free zone");
    assert_eq!(t.undo_len(), before + 1, "zone source and binding are one step");
    let p = t.panel(tr);
    let pad = &p.pad(3).unwrap().pad;
    assert!(!pad.empty && pad.name == "Snare", "{pad:?}");
    assert_eq!(p.selected, 3, "the pad a sample lands on is selected");
    // A second file on the same pad replaces its zone's source, keeping the zone.
    let clap = t.sample("Clap.wav", 1200);
    t.drop_file(&clap, t.pad_at(tr, 3));
    assert_eq!(t.patch(tr).pad(3).zone, Some(0));
    assert_eq!(t.panel(tr).pad(3).unwrap().pad.name, "Clap");
    t.app.run("edit.undo", json!({})).unwrap();
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(3);
    assert!(t.patch(tr).zones.is_empty() && t.patch(tr).pad(3).zone.is_none());
    assert!(t.panel(tr).pad(3).unwrap().pad.empty);
}

#[test]
fn a_refused_file_leaves_the_kit_and_shows_the_panels_note() {
    let (mut t, tr) = T::open();
    let long = t.dir.join("Long.wav");
    wav(&long, 48_000 * 31);
    let before = t.undo_len();
    let window = t.panel(tr).window;
    t.drop_file(&long, t.pad_at(tr, 0));
    let p = t.panel(tr);
    let (text, rect) = p.note.clone().expect("the refusal is shown, never silent");
    assert!(text.contains("30 s"), "{text}");
    assert!(rect[1] >= p.panel("pads").unwrap()[3], "under the hero row");
    assert!(t.patch(tr).zones.is_empty() && t.patch(tr).pad(0).zone.is_none());
    assert_eq!(t.undo_len(), before, "no step");
    assert!(p.window[3] > window[3], "the window grew to hold the note");
    // The next edit that lands clears it.
    let ok = t.sample("Kick.wav", 480);
    t.drop_file(&ok, t.pad_at(tr, 0));
    assert!(t.panel(tr).note.is_none());
    assert_eq!(t.panel(tr).window, window, "and the window fits again");
}

#[test]
fn a_press_selects_a_pad_and_auditions_it_at_the_audition_velocity() {
    let (mut t, tr) = T::kit();
    t.app.ui.keys.velocity = 90;
    let uid = t.window(tr).uid;
    let view = |t: &T| t.app.builtin_editor.devices.sampler.views.get(&uid).cloned().unwrap();
    let before = t.undo_len();
    t.click(t.pad_at(tr, 2), 1);
    assert_eq!(t.panel(tr).selected, 2);
    assert_eq!((view(&t).auditions, view(&t).last_audition), (1, Some((62, 90))), "A3's trigger note");
    assert_eq!(t.undo_len(), before, "selecting is not an edit");
    // Negative control: an empty pad is selected without an audition.
    t.click(t.pad_at(tr, 9), 1);
    assert_eq!(t.panel(tr).selected, 9);
    assert_eq!(view(&t).auditions, 1);
}

#[test]
fn the_choke_picker_m_and_s_each_write_one_step() {
    let (mut t, tr) = T::kit();
    t.click(t.pad_at(tr, 1), 1);
    let before = t.undo_len();
    let seg = t.panel(tr).strip.segments[4].rect;
    t.click(mid(seg), 1);
    assert_eq!(t.patch(tr).pad(1).choke, 4);
    assert_eq!(t.undo_len(), before + 1);
    assert_eq!(t.panel(tr).pad(1).unwrap().pad.badge_text, "4");
    // Negative control: clicking the group it is in writes nothing.
    t.click(mid(seg), 1);
    assert_eq!(t.undo_len(), before + 1);
    // Off.
    t.click(mid(t.panel(tr).strip.segments[0].rect), 1);
    assert_eq!(t.patch(tr).pad(1).choke, 0);
    assert!(t.panel(tr).pad(1).unwrap().pad.badge.is_none());
    // M.
    t.click(mid(t.panel(tr).strip.mute.unwrap().rect), 1);
    assert!(t.patch(tr).pad(1).mute);
    let p = t.panel(tr);
    assert_eq!((p.pad(1).unwrap().pad.opacity, p.pad(1).unwrap().pad.chip_letter), (0.4, Some('M')));
    assert_eq!(p.pad(0).unwrap().pad.opacity, 1.0, "the others are untouched");
    t.click(mid(t.panel(tr).strip.mute.unwrap().rect), 1);
    assert!(!t.patch(tr).pad(1).mute);
    assert_eq!(t.undo_len(), before + 4);
}

#[test]
fn a_solo_pad_dims_every_other_pad_to_forty_percent() {
    let (mut t, tr) = T::kit();
    // Negative control: before the solo every pad is at full strength.
    assert!(t.panel(tr).pads.iter().all(|r| r.pad.opacity == 1.0));
    t.click(t.pad_at(tr, 2), 1);
    t.click(mid(t.panel(tr).strip.solo.unwrap().rect), 1);
    assert!(t.patch(tr).pad(2).solo);
    let p = t.panel(tr);
    assert_eq!((p.pad(2).unwrap().pad.opacity, p.pad(2).unwrap().pad.chip_letter), (1.0, Some('S')));
    for i in [0u8, 1] {
        assert_eq!(p.pad(i).unwrap().pad.opacity, 0.4, "pad {i} is excluded");
    }
    assert!(p.pads.iter().filter(|r| r.pad.empty).all(|r| r.pad.opacity == 1.0), "empty pads never dim");
}

#[test]
fn a_drag_between_pads_swaps_them_in_one_undo_step() {
    let (mut t, tr) = T::kit();
    t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": 1, "name": "Boom", "choke": 3}));
    let before = t.undo_len();
    let (a, c) = (t.patch(tr).pad(0), t.patch(tr).pad(2));
    t.drag(t.pad_at(tr, 0), t.pad_at(tr, 2), false);
    let patch = t.patch(tr);
    assert_eq!((patch.pad(0).zone, patch.pad(2).zone), (c.zone, a.zone), "the bindings swapped");
    assert_eq!((patch.pad(0).name.as_deref(), patch.pad(2).name.as_deref()), (None, Some("Boom")), "the name moved with its sound");
    assert_eq!((patch.pad(0).choke, patch.pad(2).choke, patch.pad(0).note), (3, 0, None), "group and note stay with the pad");
    assert_eq!(t.undo_len(), before + 1, "one step");
    let p = t.panel(tr);
    assert_eq!((p.pad(0).unwrap().pad.name.as_str(), p.pad(2).unwrap().pad.name.as_str()), ("OH Hat", "Boom"));
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(3);
    assert_eq!((t.patch(tr).pad(0), t.patch(tr).pad(2)), (a, c), "one undo puts both back");
    // Onto an empty pad: the sound moves.
    t.drag(t.pad_at(tr, 1), t.pad_at(tr, 6), false);
    let patch = t.patch(tr);
    assert!(patch.pad(1).zone.is_none() && patch.pad(6).zone == Some(1));
    // Negative control: a drag that ends on the pad it started on is not a swap.
    let n = t.undo_len();
    let at = t.pad_at(tr, 0);
    t.drag(at, (at.0 + 10.0, at.1 + 10.0), false);
    assert_eq!(t.undo_len(), n);
}

#[test]
fn a_double_click_on_the_strips_name_renames_the_pad() {
    let (mut t, tr) = T::kit();
    t.click(t.pad_at(tr, 0), 1);
    let before = t.undo_len();
    t.click(mid(t.panel(tr).strip.name_rect), 2);
    let r = t.app.finish.rename.as_ref().expect("the field opens");
    assert!(matches!(r.target, crate::finish::RenameTarget::Pad(x, 0) if x == tr));
    assert_eq!(r.text, "Kick");
    t.send("ui.text", json!({"text": "Big Kick"}));
    t.key("Enter");
    assert_eq!(t.patch(tr).pad(0).name.as_deref(), Some("Big Kick"));
    assert_eq!(t.undo_len(), before + 1);
    assert_eq!(t.panel(tr).pad(0).unwrap().pad.name, "Big Kick");
    // Negative control: an empty pad's name does not open the field.
    t.click(t.pad_at(tr, 9), 1);
    t.click(mid(t.panel(tr).strip.name_rect), 2);
    assert!(t.app.finish.rename.is_none());
}

#[test]
fn a_knob_drag_edits_the_zone_in_one_undo_step() {
    let (mut t, tr) = T::kit();
    t.click(t.pad_at(tr, 0), 1);
    let before = t.undo_len();
    let k = t.panel(tr).knob("attack_ms").unwrap().knob.dial;
    t.drag(mid(k), (mid(k).0, mid(k).1 - 60.0), false);
    let a = t.patch(tr).zone(0).unwrap().attack_ms;
    assert!(a > 1.0, "dragged up: {a}");
    assert_eq!(t.undo_len(), before + 1, "one step for the drag");
    assert_eq!(t.panel(tr).knob("attack_ms").unwrap().value, super::zone::ATTACK.format(a));
    // Level is the zone slot's control.
    let k = t.panel(tr).knob("level").unwrap().knob.dial;
    t.drag(mid(k), (mid(k).0, mid(k).1 + 40.0), false);
    let level = t.patch(tr).controls.get("zone001_level").copied().expect("the slot control is stored");
    assert!(level < 0.0, "{level}");
    assert_eq!(t.undo_len(), before + 2);
    // A double click resets Sustain to its default exactly (100 %).
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "playback": {"sustain": 0.5}}));
    let k = t.panel(tr).knob("sustain").unwrap().knob.dial;
    t.click(mid(k), 2);
    assert_eq!(t.patch(tr).zone(0).unwrap().sustain, 1.0);
    // Negative control: another pad's zone is untouched.
    assert_eq!(t.patch(tr).zone(1).unwrap().attack_ms, 1.0);
}

#[test]
fn the_trigger_mode_reverse_and_key_track_write_the_zone() {
    let (mut t, tr) = T::kit();
    t.click(t.pad_at(tr, 0), 1);
    let z = t.patch(tr).zone(0).unwrap().clone();
    assert_eq!(z.mode, transit_model::sampler_patch::PlayMode::OneShot, "a new pad's default");
    let p = t.panel(tr);
    assert_eq!(p.mode.clone().unwrap().selected, 0);
    let before = t.undo_len();
    t.click(mid(p.mode.unwrap().segments[1]), 1);
    assert_eq!(t.patch(tr).zone(0).unwrap().mode, transit_model::sampler_patch::PlayMode::Gate);
    t.click(mid(t.panel(tr).reverse.unwrap().rect), 1);
    assert!(t.patch(tr).zone(0).unwrap().reverse);
    t.click(mid(t.panel(tr).key_track.unwrap().rect), 1);
    assert!(t.patch(tr).zone(0).unwrap().key_tracking);
    assert_eq!(t.undo_len(), before + 3);
    // Negative control: with an empty pad selected the row takes no input.
    t.click(t.pad_at(tr, 9), 1);
    let p = t.panel(tr);
    assert!(!p.reverse.clone().unwrap().available);
    let n = t.undo_len();
    t.click(mid(p.reverse.unwrap().rect), 1);
    let k = t.panel(tr).knob("attack_ms").unwrap().knob.dial;
    t.drag(mid(k), (mid(k).0, mid(k).1 - 60.0), false);
    assert_eq!(t.undo_len(), n);
}
