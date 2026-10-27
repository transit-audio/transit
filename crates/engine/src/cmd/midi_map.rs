//! MIDI Learn commands (ADR 0028): arm a learn, map and unmap controls, list mappings.
//!
//! `midi.map` and `midi.unmap` change the session's mappings (one undo step) or, with
//! `scope: "profile"`, this machine's controller profile (no undo step, like the I/O map).

use super::*;
use crate::cmd;
use crate::{Engine, LearnArm};
use serde_json::json;
use transit_model::{AutoParam, Control, ControlTarget, MapMode, Mapping, MappingSet, MidiSource, Scope, SourceKind, Strip};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "midi.learn",
            "MIDI Learn",
            [],
            None,
            "{target: \"track:<id>:volume|pan|mute|solo|arm|send:<slot>|plugin:<slot a..j|instrument>:<param id>\", scope?: session|profile} — the next controller message maps it",
            always,
            learn
        ),
        cmd!(query "midi.learn_cancel", "Cancel MIDI Learn", [], None, "{}", always, |e, _| {
            let was = e.learn.take().is_some();
            e.midi_profile_revision = e.midi_profile_revision.wrapping_add(1);
            Ok(json!({"cancelled": was}))
        }),
        cmd!(
            "midi.map",
            "Map MIDI Control",
            [],
            None,
            "{source: {port, channel?: 1..16|any, kind: cc|note, number: 0..127}, target, mode?: pickup|jump|scaled, range?: [min, max] (0..1), toggle?: bool, pass_through?: bool (also reach the track's routes; default false), scope?: session|profile}",
            always,
            map
        ),
        cmd!(
            "midi.unmap",
            "Forget MIDI Mapping",
            [],
            None,
            "{target, scope?: session|profile} — without scope the session's mapping goes first, then the profile's",
            always,
            unmap
        ),
        cmd!(query "midi.mappings", "MIDI Mappings", [], None, "{} — session and profile mappings, the one armed to learn, and which are overridden", always, mappings),
    ]
}

fn scope_of(p: &Value, id: &str) -> Result<Option<Scope>> {
    match p.get("scope") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_str() {
            Some("session") => Ok(Some(Scope::Session)),
            Some("profile") => Ok(Some(Scope::Profile)),
            _ => Err(bad(id, "`scope` must be session or profile")),
        },
    }
}

/// The target as the scope stores it: a session mapping names a track by id, the profile by position.
fn normalize(e: &Engine, id: &str, target: ControlTarget, scope: Scope) -> Result<ControlTarget> {
    let tracks = &e.session().tracks;
    let strip = match (scope, target.strip) {
        (Scope::Session, Strip::Position(n)) => {
            let t = tracks.get(usize::from(n).wrapping_sub(1)).ok_or_else(|| bad(id, format!("no track at position {n}")))?;
            Strip::Track(t.id)
        }
        (Scope::Session, Strip::Track(t)) => {
            if e.session().track(t).is_none() {
                return Err(bad(id, format!("no track with id {}", t.0)));
            }
            Strip::Track(t)
        }
        (Scope::Profile, Strip::Track(t)) => {
            let at = tracks.iter().position(|x| x.id == t).ok_or_else(|| bad(id, format!("no track with id {}", t.0)))?;
            Strip::Position(u16::try_from(at + 1).map_err(|_| bad(id, "too many tracks"))?)
        }
        (Scope::Profile, Strip::Position(n)) => Strip::Position(n),
    };
    Ok(ControlTarget { strip, control: target.control })
}

fn target_param(p: &Value, id: &str) -> Result<ControlTarget> {
    str_param(p, "target").ok_or_else(|| bad(id, "`target` required"))?.parse().map_err(|m: String| bad(id, m))
}

/// A plug-in target must name a device the track has, and a built-in device's parameter must
/// exist. (A hosted plug-in's table lives in the host registries, so the UI checks that a
/// hosted parameter is one the picker offers: not hidden, not read-only.)
fn check_device(e: &Engine, id: &str, target: &ControlTarget) -> Result<()> {
    let Control::Plugin { slot, param } = &target.control else { return Ok(()) };
    let Strip::Track(t) = target.strip else { return Ok(()) };
    let track = e.session().track(t).ok_or_else(|| bad(id, format!("no track with id {}", t.0)))?;
    let device = if *slot == AutoParam::INSTRUMENT_SLOT {
        track.instrument.as_ref()
    } else {
        track.mixer.inserts.get(usize::from(*slot)).and_then(Option::as_ref)
    };
    let device = device.ok_or_else(|| bad(id, "that slot has no plug-in"))?;
    if let Some(info) = transit_dsp::plugin_info(&device.plugin)
        && info.param(param).is_none()
    {
        return Err(bad(id, format!("{} has no parameter `{param}`", info.name)));
    }
    Ok(())
}

fn learn(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "midi.learn";
    let scope = scope_of(p, id)?.unwrap_or(Scope::Session);
    let target = normalize(e, id, target_param(p, id)?, scope)?;
    check_device(e, id, &target)?;
    e.learn = Some(LearnArm { target: target.clone(), scope });
    e.midi_profile_revision = e.midi_profile_revision.wrapping_add(1);
    Ok(json!({"armed": target.to_string(), "scope": scope}))
}

fn source_param(p: &Value, id: &str) -> Result<MidiSource> {
    let s = p.get("source").filter(|v| v.is_object()).ok_or_else(|| bad(id, "`source` required"))?;
    let port = str_param(s, "port").filter(|x| !x.is_empty() && x.len() <= 1024).ok_or_else(|| bad(id, "source `port` required"))?.to_string();
    let channel = match s.get("channel") {
        None | Some(Value::Null) => None,
        Some(v) if v.as_str() == Some("any") => None,
        Some(v) => Some(v.as_u64().filter(|c| (1..=16).contains(c)).ok_or_else(|| bad(id, "source `channel` must be any or 1..16"))? as u8 - 1),
    };
    let kind = match str_param(s, "kind") {
        Some("cc") => SourceKind::Cc,
        Some("note") => SourceKind::Note,
        _ => return Err(bad(id, "source `kind` must be cc or note")),
    };
    let number = s.get("number").and_then(Value::as_u64).filter(|n| *n < 128).ok_or_else(|| bad(id, "source `number` must be 0..127"))? as u8;
    Ok(MidiSource { port, channel, kind, number })
}

fn map(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "midi.map";
    let scope = scope_of(p, id)?.unwrap_or(Scope::Session);
    let target = normalize(e, id, target_param(p, id)?, scope)?;
    check_device(e, id, &target)?;
    let mut m = Mapping::new(source_param(p, id)?, target.clone());
    m.mode = match str_param(p, "mode") {
        None | Some("pickup") => MapMode::Pickup,
        Some("jump") => MapMode::Jump,
        Some("scaled") => MapMode::Scaled,
        Some(_) => return Err(bad(id, "`mode` must be pickup, jump or scaled")),
    };
    if let Some(r) = p.get("range") {
        let r = r.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(id, "`range` must be [min, max]"))?;
        let n = |v: &Value| v.as_f64().filter(|x| x.is_finite()).map(|x| x as f32).ok_or_else(|| bad(id, "`range` must be numbers"));
        m.min = n(&r[0])?;
        m.max = n(&r[1])?;
    }
    if let Some(t) = p.get("toggle") {
        m.toggle = t.as_bool().ok_or_else(|| bad(id, "`toggle` must be a boolean"))?;
    }
    if let Some(t) = p.get("pass_through") {
        m.pass_through = t.as_bool().ok_or_else(|| bad(id, "`pass_through` must be a boolean"))?;
    }
    m.validate().map_err(|why| bad(id, why))?;
    let replaced = match scope {
        Scope::Session => store(&mut e.session_mut().midi_mappings, m.clone()),
        Scope::Profile => {
            let was = store(&mut e.midi_profile, m.clone());
            e.midi_profile_revision = e.midi_profile_revision.wrapping_add(1);
            was
        }
    };
    if e.learn.as_ref().is_some_and(|l| l.target == target && l.scope == scope) {
        e.learn = None;
        e.midi_profile_revision = e.midi_profile_revision.wrapping_add(1);
    }
    Ok(json!({"target": target.to_string(), "scope": scope, "replaced": replaced}))
}

/// Store `m`; how many mappings it displaced.
fn store(set: &mut MappingSet, m: Mapping) -> usize {
    let before = set.len();
    // `m` is validated by the caller.
    let _ = set.set(m);
    before + 1 - set.len()
}

fn unmap(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "midi.unmap";
    let target = target_param(p, id)?;
    let scopes: Vec<Scope> = match scope_of(p, id)? {
        Some(s) => vec![s],
        None => vec![Scope::Session, Scope::Profile],
    };
    for scope in scopes {
        let t = normalize(e, id, target.clone(), scope)?;
        let removed = match scope {
            Scope::Session => {
                if e.session().midi_mappings.for_target(&t).is_none() {
                    false
                } else {
                    e.session_mut().midi_mappings.remove(&t)
                }
            }
            Scope::Profile => {
                let removed = e.midi_profile.remove(&t);
                if removed {
                    e.midi_profile_revision = e.midi_profile_revision.wrapping_add(1);
                }
                removed
            }
        };
        if removed {
            return Ok(json!({"target": t.to_string(), "scope": scope}));
        }
    }
    Err(bad(id, "that control has no mapping"))
}

fn describe(m: &Mapping, scope: Scope, overridden: bool) -> Value {
    let channel = m.source.channel.map(|c| c + 1);
    json!({
        "scope": scope,
        "target": m.target.to_string(),
        "source": {"port": m.source.port, "channel": channel, "kind": m.source.kind, "number": m.source.number},
        "text": m.source.describe(),
        "mode": m.mode,
        "range": [m.min, m.max],
        "toggle": m.toggle,
        "pass_through": m.pass_through,
        "overridden": overridden,
    })
}

fn mappings(e: &mut Engine, _: &Value) -> Result<Value> {
    let ids: Vec<_> = e.session().tracks.iter().map(|t| t.id).collect();
    let resolved = transit_model::resolve(&ids, &e.session().midi_mappings, &e.midi_profile);
    let session: Vec<Value> = e.session().midi_mappings.iter().map(|m| describe(m, Scope::Session, false)).collect();
    let profile: Vec<Value> = e
        .midi_profile
        .iter()
        .map(|m| {
            let over = resolved.iter().any(|r| r.scope == Scope::Profile && r.mapping == *m && r.overridden);
            describe(m, Scope::Profile, over)
        })
        .collect();
    let learn = e.learn.as_ref().map(|l| json!({"target": l.target.to_string(), "scope": l.scope}));
    Ok(json!({"session": session, "profile": profile, "learn": learn, "active": resolved.iter().filter(|r| !r.overridden).count()}))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::demo::demo_engine;

    fn first_two(e: &Engine) -> (u64, u64) {
        let t = &e.session().tracks;
        (t[0].id.0, t[1].id.0)
    }
    fn src(cc: u8) -> Value {
        json!({"port": "KeyStep", "channel": 1, "kind": "cc", "number": cc})
    }

    #[test]
    fn map_and_unmap_a_session_control_are_one_undo_step_each() {
        let mut e = demo_engine();
        let (a, _) = first_two(&e);
        let target = format!("track:{a}:volume");
        e.execute("midi.map", &json!({"source": src(74), "target": target})).unwrap();
        assert_eq!(e.session().midi_mappings.len(), 1);
        assert!(e.is_dirty());
        e.execute("midi.unmap", &json!({"target": target})).unwrap();
        assert!(e.session().midi_mappings.is_empty());
        assert!(e.undo(), "forget undoes");
        assert_eq!(e.session().midi_mappings.len(), 1);
        assert!(e.undo(), "learn undoes");
        assert!(e.session().midi_mappings.is_empty());
        assert!(e.execute("midi.unmap", &json!({"target": target})).is_err(), "nothing to forget");
    }

    #[test]
    fn profile_mappings_are_per_machine_and_make_no_undo_step() {
        let mut e = demo_engine();
        let (a, _) = first_two(&e);
        let pushes = e.undo_pushes();
        let r = e.execute("midi.map", &json!({"source": src(21), "target": format!("track:{a}:pan"), "scope": "profile"})).unwrap();
        assert_eq!(r["target"], "strip:1:pan", "the profile names strips by position");
        assert_eq!(e.undo_pushes(), pushes);
        assert!(!e.is_dirty());
        assert_eq!(e.midi_profile().len(), 1);
        assert!(e.session().midi_mappings.is_empty());
        e.execute("midi.unmap", &json!({"target": "strip:1:pan"})).unwrap();
        assert!(e.midi_profile().is_empty());
    }

    #[test]
    fn learn_arms_and_the_mapping_disarms_it() {
        let mut e = demo_engine();
        let (a, b) = first_two(&e);
        e.execute("midi.learn", &json!({"target": format!("track:{a}:mute")})).unwrap();
        assert_eq!(e.learn().unwrap().target.to_string(), format!("track:{a}:mute"));
        e.execute("midi.learn", &json!({"target": format!("track:{b}:volume")})).unwrap();
        assert_eq!(e.learn().unwrap().target.to_string(), format!("track:{b}:volume"), "arming again replaces");
        assert!(!e.is_dirty(), "arming is not an edit");
        e.execute("midi.map", &json!({"source": src(7), "target": format!("track:{b}:volume")})).unwrap();
        assert!(e.learn().is_none());
        e.execute("midi.learn", &json!({"target": format!("track:{a}:solo")})).unwrap();
        e.execute("midi.learn_cancel", &json!({})).unwrap();
        assert!(e.learn().is_none());
    }

    #[test]
    fn map_validates_and_mappings_lists_everything() {
        let mut e = demo_engine();
        let (a, b) = first_two(&e);
        let t = format!("track:{a}:volume");
        for bad in [
            json!({"source": src(74), "target": "track:9999:volume"}),
            json!({"source": src(74), "target": t, "mode": "sideways"}),
            json!({"source": src(74), "target": t, "range": [0.5, 0.5]}),
            json!({"source": {"port": "K", "kind": "cc", "number": 200}, "target": t}),
            json!({"source": {"port": "K", "kind": "cc", "number": 1, "channel": 17}, "target": t}),
            json!({"target": t}),
            json!({"source": src(74), "target": t, "scope": "everywhere"}),
        ] {
            assert!(e.execute("midi.map", &bad).is_err(), "{bad}");
        }
        assert!(e.session().midi_mappings.is_empty() && !e.is_dirty());
        e.execute("midi.map", &json!({"source": src(74), "target": t, "mode": "jump", "range": [0.2, 0.8]})).unwrap();
        e.execute("midi.map", &json!({"source": src(74), "target": "strip:2:pan", "scope": "profile"})).unwrap();
        e.execute("midi.map", &json!({"source": src(20), "target": format!("track:{b}:send:1"), "scope": "profile"})).unwrap();
        let r = e.execute("midi.mappings", &json!({})).unwrap();
        assert_eq!(r["session"][0]["mode"], "jump");
        assert_eq!(r["session"][0]["range"], json!([0.2f32, 0.8f32]));
        assert_eq!(r["profile"].as_array().unwrap().len(), 2);
        assert_eq!(r["profile"][0]["overridden"], true, "the session owns CC 74 on that port and channel");
        assert_eq!(r["profile"][1]["overridden"], false);
        assert_eq!(r["active"], 2);
    }

    #[test]
    fn plug_in_parameters_map_by_stable_id_and_are_checked_against_the_device() {
        let mut e = demo_engine();
        let (a, _) = first_two(&e);
        e.execute("mix.insert", &json!({"track": a, "slot": 0, "plugin": "eq_7band"})).unwrap();
        let ok = format!("track:{a}:plugin:a:mid_gain");
        e.execute("midi.map", &json!({"source": src(74), "target": ok})).unwrap();
        let m = e.session().midi_mappings.iter().next().unwrap().clone();
        assert_eq!(m.target.to_string(), ok);
        // Serde: the session file carries the text form and loads back.
        let text = serde_json::to_string(&m).unwrap();
        assert!(text.contains(&format!("\"track:{a}:plugin:a:mid_gain\"")), "{text}");
        assert_eq!(serde_json::from_str::<Mapping>(&text).unwrap(), m);
        for bad in [
            format!("track:{a}:plugin:a:no_such_param"),
            format!("track:{a}:plugin:b:mid_gain"),
            format!("track:{a}:plugin:instrument:mid_gain"),
            format!("track:{a}:plugin:a"),
        ] {
            assert!(e.execute("midi.map", &json!({"source": src(75), "target": bad})).is_err(), "{bad}");
            assert!(e.execute("midi.learn", &json!({"target": bad})).is_err(), "{bad}");
        }
        assert_eq!(e.session().midi_mappings.len(), 1);
        // The profile names the strip by position and keeps the parameter id.
        let r = e.execute("midi.map", &json!({"source": src(76), "target": ok, "scope": "profile"})).unwrap();
        assert_eq!(r["target"], "strip:1:plugin:a:mid_gain");
        e.execute("midi.learn", &json!({"target": ok})).unwrap();
        assert_eq!(e.learn().unwrap().target.to_string(), ok);
        e.execute("midi.unmap", &json!({"target": ok})).unwrap();
        assert!(e.session().midi_mappings.is_empty());
    }

    #[test]
    fn a_dangling_session_mapping_is_listed_but_does_not_resolve() {
        let mut e = demo_engine();
        let (a, _) = first_two(&e);
        e.execute("midi.map", &json!({"source": src(1), "target": format!("track:{a}:mute")})).unwrap();
        e.execute("track.delete", &json!({"tracks": [a]})).unwrap();
        let r = e.execute("midi.mappings", &json!({})).unwrap();
        assert_eq!(r["session"].as_array().unwrap().len(), 1);
        assert_eq!(r["active"], 0);
    }
}

#[cfg(test)]
mod replace_tests {
    #![allow(clippy::unwrap_used)]
    use crate::demo::demo_engine;
    use serde_json::json;

    #[test]
    fn an_armed_learn_does_not_survive_a_new_session() {
        let mut e = demo_engine();
        let t = e.session().tracks[0].id.0;
        e.execute("midi.learn", &json!({"target": format!("track:{t}:volume")})).unwrap();
        let rev = e.midi_profile_revision();
        e.replace_session(transit_model::Session::default());
        assert!(e.learn().is_none(), "the armed control belongs to the old session");
        assert_ne!(e.midi_profile_revision(), rev, "the host is told to disarm the worker");
    }

    #[test]
    fn pass_through_is_a_mapping_option_that_defaults_off() {
        let mut e = demo_engine();
        let t = e.session().tracks[0].id.0;
        let src = json!({"port": "K", "channel": 1, "kind": "cc", "number": 1});
        e.execute("midi.map", &json!({"source": src, "target": format!("track:{t}:volume")})).unwrap();
        assert!(!e.session().midi_mappings.iter().next().unwrap().pass_through);
        e.execute("midi.map", &json!({"source": src, "target": format!("track:{t}:volume"), "pass_through": true})).unwrap();
        assert!(e.session().midi_mappings.iter().next().unwrap().pass_through);
        assert!(e.execute("midi.map", &json!({"source": src, "target": format!("track:{t}:volume"), "pass_through": 3})).is_err());
    }
}
