//! A macro strip pages edit is a slot-record change that keeps the device instance (ADR 0047
//! decision 1.5, slice 1): the same uid, the same live instance, no new instance and no
//! re-prepare. The edit is applied through `Track::set_device_pages`, the function behind
//! `mix.device_pages`; the control routes an edit through a path that rebuilds the slot and
//! shows the check catches it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan_tests::*;
use super::*;
use std::sync::atomic::Ordering;
use transit_model::{DeviceRef, Insert, UserPage};

/// An instrument track with a Macro Oscillator and an EQ on insert a.
fn session() -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("macro_oscillator"));
    tr.mixer.inserts[0] = Some(Insert::new("eq_7band"));
    (s, t)
}

fn page() -> UserPage {
    let mut p = UserPage::named("User 1");
    p.slots[0] = Some("level".into());
    p
}

/// The live instances of the first strip (instrument, then insert a) as addresses.
fn instances(rig: &Rig) -> (usize, usize) {
    let strip = &rig.eng.plan.as_deref().unwrap().tables.strips[0];
    let addr = |s: &Option<PluginSlot>| s.as_ref().map_or(0, |s| &*s.plugin as *const dyn Plugin as *const () as usize);
    (addr(&strip.instrument), addr(strip.plugins.first().unwrap_or(&None)))
}

fn uids(s: &Session, t: TrackId) -> (u64, u64) {
    let tr = s.track(t).unwrap();
    (tr.instrument.as_ref().unwrap().uid.get(), tr.mixer.inserts[0].as_ref().unwrap().uid.get())
}

/// Publishes the session, applies `edit`, publishes again and lists every sign of a rebuilt
/// slot the edit left (empty: the instances were kept).
fn check(edit: impl Fn(&mut Session, TrackId)) -> Vec<String> {
    let (mut s, t) = session();
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    rig.render(2);
    let (made, prepared, live, ids) = (rig.log.made.load(Ordering::Relaxed), rig.log.prepared.load(Ordering::Relaxed), instances(&rig), uids(&s, t));
    assert!(live.0 != 0 && live.1 != 0, "both devices have an instance");
    edit(&mut s, t);
    let tr = s.track(t).unwrap();
    assert_eq!(tr.device_pages(DeviceRef::Instrument).unwrap(), &[page()], "the edit landed");
    assert_eq!(tr.device_pages(DeviceRef::Insert(0)).unwrap(), &[page()], "the edit landed");
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    rig.render(2);
    let mut signs = Vec::new();
    if uids(&s, t) != ids {
        signs.push(format!("uid changed: {ids:?} -> {:?}", uids(&s, t)));
    }
    let now = rig.log.made.load(Ordering::Relaxed);
    if now != made {
        signs.push(format!("made {} new instance(s)", now - made));
    }
    if instances(&rig) != live {
        signs.push("the live instances changed".into());
    }
    let re = rig.log.prepared.load(Ordering::Relaxed);
    if re != prepared {
        signs.push(format!("re-prepared {} time(s)", re - prepared));
    }
    signs
}

#[test]
fn a_pages_edit_keeps_the_device_instance() {
    let signs = check(|s, t| {
        let tr = s.track_mut(t).unwrap();
        assert_eq!(tr.set_device_pages(DeviceRef::Instrument, &[page()]), Ok(true));
        assert_eq!(tr.set_device_pages(DeviceRef::Insert(0), &[page()]), Ok(true));
    });
    assert!(signs.is_empty(), "{signs:?}");
}

/// Control: the same pages written by rebuilding the slot records (a fresh `Insert` with the old
/// settings, as a replace-the-device path would) fail the check, at the uid and at the
/// compiler's instances.
#[test]
fn control_an_edit_that_rebuilds_the_slot_fails_the_check() {
    let rebuild = |old: &Insert| Insert { pages: vec![page()], params: old.params.clone(), ..Insert::new(old.plugin.as_str()) };
    let signs = check(|s, t| {
        let tr = s.track_mut(t).unwrap();
        tr.instrument = tr.instrument.as_ref().map(rebuild);
        tr.mixer.inserts[0] = tr.mixer.inserts[0].as_ref().map(rebuild);
    });
    assert!(signs.iter().any(|x| x.starts_with("uid changed")), "{signs:?}");
    assert!(signs.iter().any(|x| x == "made 2 new instance(s)"), "{signs:?}");
    assert!(signs.iter().any(|x| x == "the live instances changed"), "{signs:?}");
}
