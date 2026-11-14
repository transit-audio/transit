//! Undoable CC point operations. Non-CC channel events are never addressed here.
use super::*;
use transit_midi::{CtrlEvent, CtrlKind};

fn number(p: &Value, key: &str, max: i64, default: Option<i64>, cmd: &str) -> Result<i64> {
    let value = match p.get(key) {
        Some(v) => v.as_i64(),
        None => default,
    };
    value.filter(|v| (0..=max).contains(v)).ok_or_else(|| bad(cmd, format!("{key} must be 0..{max}")))
}

/// Whether point `index` of the clip is hidden by its left edge ([`transit_mix::left_hidden`], ADR
/// 0052 decision 17): like a non-CC event, it is never addressed here.
fn hidden(e: &Engine, id: ClipId, index: usize) -> bool {
    e.session().find_clip(id).is_some_and(|(_, c)| match &c.content {
        ClipContent::Midi { sequence } => sequence.ctrls.get(index).is_some_and(|p| transit_mix::left_hidden(c, p.tick)),
        ClipContent::Audio { .. } => false,
    })
}

pub(super) fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    let cmd = "midi.cc_add";
    let id = clip_id(p, cmd)?;
    let point = CtrlEvent {
        tick: number(p, "tick", i64::MAX, None, cmd)?,
        channel: number(p, "channel", 15, Some(0), cmd)? as u8,
        kind: CtrlKind::Cc(number(p, "controller", 127, None, cmd)? as u8),
        value: number(p, "value", 127, None, cmd)? as i32,
    };
    let seq = seq_mut(e, id, cmd)?;
    seq.ctrls.push(point);
    seq.sort();
    Ok(json!({"point": point}))
}

pub(super) fn edit(e: &mut Engine, p: &Value) -> Result<Value> {
    let cmd = "midi.cc_edit";
    let id = clip_id(p, cmd)?;
    let index = usize::try_from(number(p, "index", i64::MAX, None, cmd)?).map_err(|_| bad(cmd, "index out of range"))?;
    let old = *seq_mut(e, id, cmd)?.ctrls.get(index).ok_or_else(|| bad(cmd, "index out of range"))?;
    if !matches!(old.kind, CtrlKind::Cc(_)) {
        return Err(bad(cmd, "not a CC point"));
    }
    if hidden(e, id, index) {
        return Err(bad(cmd, "the point is hidden by the clip's left edge"));
    }
    let point = CtrlEvent {
        tick: number(p, "tick", i64::MAX, Some(old.tick), cmd)?,
        value: number(p, "value", 127, Some(i64::from(old.value)), cmd)? as i32,
        ..old
    };
    let seq = seq_mut(e, id, cmd)?;
    seq.ctrls[index] = point;
    seq.sort();
    Ok(json!({"point": point}))
}

pub(super) fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let cmd = "midi.cc_delete";
    let id = clip_id(p, cmd)?;
    let index = usize::try_from(number(p, "index", i64::MAX, None, cmd)?).map_err(|_| bad(cmd, "index out of range"))?;
    let seq = seq_mut(e, id, cmd)?;
    if !seq.ctrls.get(index).is_some_and(|c| matches!(c.kind, CtrlKind::Cc(_))) {
        return Err(bad(cmd, "index is not a CC point"));
    }
    if hidden(e, id, index) {
        return Err(bad(cmd, "the point is hidden by the clip's left edge"));
    }
    let seq = seq_mut(e, id, cmd)?;
    seq.ctrls.remove(index);
    Ok(json!({"deleted": 1}))
}
