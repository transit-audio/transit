//! The Sampler patch's validation ([`PatchV1::validate`]): every count, id, length, finite value
//! and range, checked before anything is prepared. An edit that fails is refused; a loaded patch
//! that fails is kept opaque ([`crate::sampler_patch`]'s module doc).

use crate::sampler_patch::{
    CHOKE_GROUPS, LoopMode, MAX_CONTROL_ID, MAX_CONTROLS, MAX_LABEL_BYTES, MAX_SLICES, MAX_SOURCES, PADS, PatchV1, SCHEMA, SliceLoop, SliceSet,
    ZONES, ZoneEntry, is_control_id,
};

/// Why a patch is refused (an edit) or kept opaque (a load).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn bad(what: impl Into<String>) -> Invalid {
    Invalid(what.into())
}

fn in_range(x: f32, lo: f32, hi: f32) -> bool {
    x.is_finite() && (lo..=hi).contains(&x)
}

fn label_ok(l: Option<&String>) -> bool {
    l.is_none_or(|l| l.len() <= MAX_LABEL_BYTES)
}

impl PatchV1 {
    /// Checks every count, id, length, finite value and range (before anything is prepared). An
    /// edit that fails is refused; a loaded patch that fails is kept opaque.
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.schema != SCHEMA {
            return Err(bad(format!("schema {} is not {SCHEMA}", self.schema)));
        }
        if self.sources.len() > MAX_SOURCES {
            return Err(bad(format!("{} sources (at most {MAX_SOURCES})", self.sources.len())));
        }
        if self.zones.len() > ZONES {
            return Err(bad(format!("{} zones (at most {ZONES})", self.zones.len())));
        }
        let mut seen = [false; ZONES];
        for z in &self.zones {
            let n = z.id + 1;
            match seen.get_mut(usize::from(z.id)) {
                Some(s) if !*s => *s = true,
                Some(_) => return Err(bad(format!("zone {n} appears twice"))),
                None => return Err(bad(format!("zone id {} is past {ZONES}", z.id))),
            }
            self.check_zone(z).map_err(|why| bad(format!("zone {n}: {why}")))?;
        }
        // The key-map groups' shape (the core refuses a badly shaped one, so it loads opaque).
        self.check_groups().map_err(|r| bad(r.to_string()))?;
        self.check_pads()?;
        if self.groups.len() > ZONES {
            return Err(bad("too many groups"));
        }
        for (i, g) in self.groups.iter().enumerate() {
            if self.groups.iter().skip(i + 1).any(|h| h.id == g.id) {
                return Err(bad(format!("group {} appears twice", g.id)));
            }
        }
        if let Some(l) = &self.slicing {
            self.check_loop(l)?;
        }
        if self.controls.len() > MAX_CONTROLS {
            return Err(bad("too many controls"));
        }
        for (id, v) in &self.controls {
            if !is_control_id(id) {
                return Err(bad(format!("`{}` is not a zone control id", id.chars().take(MAX_CONTROL_ID).collect::<String>())));
            }
            if !v.is_finite() {
                return Err(bad(format!("control `{id}` is not a finite number")));
            }
        }
        Ok(())
    }

    fn check_zone(&self, z: &ZoneEntry) -> Result<(), &'static str> {
        if usize::from(z.source) >= self.sources.len() {
            return Err("its source is not in the patch");
        }
        // A slice-bound zone plays the region and loop its slice derives (checked with the slice
        // below); its stored fields may disagree or be a one-frame loop, and are rewritten on the
        // next edit that writes them through (ADR 0041 amendment 2026-11-08, "Validator changes").
        if z.slice.is_none() && z.start >= z.end {
            return Err("its region is empty");
        }
        if z.slice.is_none()
            && z.loop_mode != LoopMode::Off
            && !(z.start <= z.loop_start && z.loop_start < z.loop_end && z.loop_end <= z.end && z.loop_end - z.loop_start >= 2)
        {
            return Err("its loop is not inside its region (or shorter than two frames)");
        }
        let [klo, khi] = z.keys;
        let [vlo, vhi] = z.velocities;
        if klo > khi || khi > 127 || vlo < 1 || vlo > vhi || vhi > 127 || z.root > 127 {
            return Err("a key, velocity or root is out of range");
        }
        let ranges = in_range(z.transpose, -48.0, 48.0)
            && in_range(z.cents, -100.0, 100.0)
            && in_range(z.attack_ms, 0.0, 5000.0)
            && in_range(z.decay_ms, 0.0, 10_000.0)
            && in_range(z.sustain, 0.0, 1.0)
            && in_range(z.release_ms, 0.0, 10_000.0)
            && in_range(z.declick_ms, 0.0, 10.0)
            && in_range(z.velocity_sensitivity, 0.0, 1.0);
        if !ranges {
            return Err("a playback setting is out of range or not finite");
        }
        // Widths too wide for the ranges, or set on a pad-bound zone, are kept and clamped at use.
        if z.fades.widths().iter().any(|&w| w > crate::sampler_fades::MAX_WIDTH) {
            return Err("a fade width is past 127");
        }
        if !label_ok(z.label.as_ref()) {
            return Err("its label is too long");
        }
        if let Some(b) = &z.slice {
            let set = self.slicing.as_ref().and_then(|l| match b.set {
                SliceSet::Auto => l.auto.as_ref(),
                SliceSet::Manual => l.manual.as_ref(),
            });
            if !set.is_some_and(|s| s.markers.iter().any(|m| m.id == b.id)) {
                return Err("its slice is not in the loop's marker set");
            }
        }
        Ok(())
    }

    fn check_pads(&self) -> Result<(), Invalid> {
        if self.pad_start_note > crate::pad_notes::MAX_PAD_START {
            return Err(bad(format!("the pad start note {} is past {}", self.pad_start_note, crate::pad_notes::MAX_PAD_START)));
        }
        if self.pads.len() > PADS {
            return Err(bad(format!("{} pads (at most {PADS})", self.pads.len())));
        }
        let mut stored = [false; PADS];
        for p in &self.pads {
            let n = usize::from(p.index) + 1;
            match stored.get_mut(usize::from(p.index)) {
                Some(s) if !*s => *s = true,
                Some(_) => return Err(bad(format!("pad {n} appears twice"))),
                None => return Err(bad(format!("pad index {} is past {PADS}", p.index))),
            }
            if p.note.is_some_and(|x| x > 127) {
                return Err(bad(format!("pad {n}: its note is past 127")));
            }
            if p.choke > CHOKE_GROUPS {
                return Err(bad(format!("pad {n}: choke group {} is past {CHOKE_GROUPS}", p.choke)));
            }
            if !label_ok(p.name.as_ref()) {
                return Err(bad(format!("pad {n}: its name is too long")));
            }
            if let Some(z) = p.zone {
                let Some(zone) = self.zone(z) else { return Err(bad(format!("pad {n}: zone {} is not populated", z + 1))) };
                if zone.membership.is_some() {
                    return Err(bad(format!("pad {n}: a pad's zone cannot be a layer or round robin")));
                }
                if self.pads.iter().filter(|q| q.zone == Some(z)).count() > 1 {
                    return Err(bad(format!("zone {} is bound to two pads", z + 1)));
                }
            }
        }
        // Unique among the pads' effective notes (stored, or the start note plus the index).
        let mut notes = [false; 128];
        for i in 0..PADS as u8 {
            let note = self.pad_note(i);
            match notes.get_mut(usize::from(note)) {
                Some(s) if !*s => *s = true,
                _ => return Err(bad(format!("pad {}: note {note} is another pad's trigger note", i + 1))),
            }
        }
        Ok(())
    }

    fn check_loop(&self, l: &SliceLoop) -> Result<(), Invalid> {
        if usize::from(l.source) >= self.sources.len() {
            return Err(bad("the loop's source is not in the patch"));
        }
        if l.start >= l.end {
            return Err(bad("the loop's region is empty"));
        }
        if !in_range(l.detector.sensitivity, 0.0, 1.0) {
            return Err(bad("the detector sensitivity is out of range"));
        }
        if l.bars.is_some_and(|b| !(1..=16).contains(&b))
            || l.root.is_some_and(|r| r > 127)
            || l.beat_length.is_some_and(|b| !(b.is_finite() && b > 0.0))
        {
            return Err(bad("the loop's bars, root or beat length is out of range"));
        }
        if l.grid_bpm.is_some_and(|b| !(b.is_finite() && (5.0..=1000.0).contains(&b))) {
            return Err(bad("the loop's grid tempo is not from 5 to 1000 BPM"));
        }
        for (name, set) in [("Auto", l.auto.as_ref()), ("Manual", l.manual.as_ref())] {
            let Some(set) = set else { continue };
            if set.markers.first().is_some_and(|m| m.frame != l.start) {
                return Err(bad(format!("{name}'s first marker is not at the loop start")));
            }
            if set.markers.len() > MAX_SLICES {
                return Err(bad(format!("{name} has {} slices (at most {MAX_SLICES} per loop)", set.markers.len())));
            }
            let mut last: Option<u64> = None;
            for (i, m) in set.markers.iter().enumerate() {
                if m.frame < l.start || m.frame >= l.end || last.is_some_and(|f| m.frame <= f) {
                    return Err(bad(format!("{name}'s markers are not increasing inside the loop")));
                }
                last = Some(m.frame);
                if set.markers.iter().skip(i + 1).any(|o| o.id == m.id) {
                    return Err(bad(format!("{name}'s slice id {} appears twice", m.id)));
                }
                if !label_ok(m.label.as_ref()) {
                    return Err(bad(format!("{name}'s label is too long")));
                }
            }
        }
        Ok(())
    }
}
