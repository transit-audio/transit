use super::*;

/// A plugin instance a session needs: which slot, which plugin, how many channels, and the state
/// to restore.
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceSpec {
    pub track: TrackId,
    /// Insert slot index, [`INSTRUMENT_SLOT`], or a clip-effect slot ([`clip_fx_slot`]).
    pub slot: usize,
    pub id: String,
    /// Channel count the instance is prepared for.
    pub ch: usize,
    /// Decoded [`Insert::state`](transit_model::Insert::state).
    pub state: Option<Vec<u8>>,
    /// The insert's [`InsertUid`](transit_model::InsertUid): the instance follows it when the
    /// insert moves to another slot (0 for an instrument or a clip effect).
    pub uid: u64,
}

/// Every third-party plugin slot (inserts and instruments) of a session, with the channel count
/// a plan prepares it for. Inactive and master tracks are included (they have strips).
pub fn instance_specs(s: &Session) -> Vec<InstanceSpec> {
    let mut out = realtime_instance_specs(s);
    out.retain(|x| is_third_party(&x.id));
    out
}

/// Every plugin instance a realtime engine takes from its host: each insert and instrument, built-in or third-party, and each clip-effect module
/// (an EQ or compressor per clip, [`clip_fx_slot`]) of an audio track, with the channel count
/// a plan prepares it for. Only third-party plugins carry a state to restore.
/// Inactive and master tracks are included (they have strips); frozen tracks are not (they play
/// their render, [`Session::with_frozen_audio`]).
pub fn realtime_instance_specs(s: &Session) -> Vec<InstanceSpec> {
    let main_ch = main_channels(s);
    let state = |ins: &transit_model::Insert| if is_third_party(&ins.plugin) { ins.state_bytes() } else { None };
    let mut out = Vec::new();
    for t in s.tracks.iter().filter(|t| s.frozen_audio(t).is_none()) {
        let ch = strip_channels(t, main_ch);
        let mut seen: Vec<u64> = Vec::new();
        for (i, ins) in t.mixer.inserts.iter().enumerate() {
            // A hardware insert has no plug-in instance (the plan runs it, `hardware.rs`).
            if let Some(ins) = ins.as_ref().filter(|x| x.hardware.is_none()) {
                // A copied insert on the same strip keeps no identity (0): its instance stays by slot.
                let uid = ins.uid.get();
                let uid = if seen.contains(&uid) { 0 } else { uid };
                seen.push(uid);
                out.push(InstanceSpec { track: t.id, slot: i, id: ins.plugin.clone(), ch, state: state(ins), uid });
            }
        }
        // An External Instrument has no plug-in instance (the plan reads its return, `external.rs`).
        if let Some(ins) = t.instrument.as_ref().filter(|i| i.plugin != transit_model::EXTERNAL_INSTRUMENT_PLUGIN) {
            out.push(InstanceSpec { track: t.id, slot: INSTRUMENT_SLOT, id: ins.plugin.clone(), ch: ch.max(2), state: state(ins), uid: 0 });
        }
        if t.kind == TrackKind::Audio {
            for clip in t.clips().iter().filter(|c| c.is_audio()) {
                let (eq, comp) = clip_fx_modules(s, clip.id.0);
                for (want, is_comp, id) in [(eq, false, CLIP_EQ), (comp, true, CLIP_COMP)] {
                    if let Some(slot) = clip_fx_slot(clip.id.0, is_comp).filter(|_| want) {
                        out.push(InstanceSpec { track: t.id, slot, id: id.to_string(), ch, state: None, uid: 0 });
                    }
                }
            }
        }
    }
    out
}

/// Instances being made by [`create_instance`] right now (any thread).
static CREATING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct CreatingGuard;

impl CreatingGuard {
    fn enter() -> Self {
        CREATING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        CreatingGuard
    }
}

impl Drop for CreatingGuard {
    fn drop(&mut self) {
        CREATING.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Whether a plugin instance is being created now ([`create_instance`] is running). One relaxed
/// atomic load: the audio callback reads it to tell a callback that overlapped a creation (module
/// load, `initialize`, state restore, all on the control thread) from one that did not.
#[must_use]
pub fn creating_instance() -> bool {
    CREATING.load(std::sync::atomic::Ordering::Relaxed) != 0
}

/// A plugin made by [`create_instance`].
pub struct NewInstance {
    pub plugin: Box<dyn Plugin>,
    /// The plugin refused the stored state and starts from its defaults.
    pub refused_state: bool,
}

/// Creates a plugin (built-in or hosted), restores `state` (if any) and prepares it. Expensive
/// for third-party plugins: in realtime use, call it on the control thread.
pub fn create_instance(id: &str, state: Option<&[u8]>, sample_rate: f32, max_block: usize, ch: usize) -> Option<NewInstance> {
    create_instance_with(id, state, sample_rate, max_block, ch, || {})
}

/// [`create_instance`] that calls `restoring` once the plugin exists and its stored state is
/// about to be restored and it prepared (the second phase of a load, issue #67).
pub fn create_instance_with(
    id: &str,
    state: Option<&[u8]>,
    sample_rate: f32,
    max_block: usize,
    ch: usize,
    restoring: impl FnOnce(),
) -> Option<NewInstance> {
    let _creating = CreatingGuard::enter();
    let started = std::time::Instant::now();
    let Some(mut plugin) = create_plugin(id) else {
        log::warn!("{}", creation_line(id, started.elapsed(), false, state.is_some()));
        return None;
    };
    restoring();
    let refused_state = state.is_some_and(|st| !plugin.load_state(st));
    if refused_state {
        log::warn!("{id}: the plugin did not accept its stored state");
    }
    plugin.prepare(sample_rate, max_block, ch);
    log::info!("{}", creation_line(id, started.elapsed(), true, state.is_some()));
    Some(NewInstance { plugin, refused_state })
}

/// The INFO line for one plug-in instance: `plug-in instance created: id=<id> in 412 ms (stored
/// state restored)`; a failed creation says `failed` (issue #67 reads these for load times).
pub fn creation_line(id: &str, took: std::time::Duration, ok: bool, had_state: bool) -> String {
    let verb = if ok { "created" } else { "failed" };
    let state = if had_state { "with stored state" } else { "no stored state" };
    format!("plug-in instance {verb}: id={id} in {} ms ({state})", took.as_millis())
}

/// A built-in plugin, else a hosted CLAP plugin (`clap:<id>`).
fn create_plugin(id: &str) -> Option<Box<dyn Plugin>> {
    // Tests never load real plug-ins; adopt a fake instead. Scanning the machine's plug-in folders
    // loads every installed plug-in in-process, and some never unload, so a test that reached the
    // real hosts would hang on a developer machine (CI has none installed).
    #[cfg(test)]
    if is_third_party(id) {
        return None;
    }
    transit_dsp::create(id)
        .or_else(|| transit_clap_host::create(id))
        .or_else(|| transit_vst3_host::create(id))
        .or_else(|| transit_au_host::create(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::session_with_dc;
    use transit_model::Insert;

    #[test]
    fn the_creation_line_names_the_plugin_and_its_time_for_issue_67() {
        let took = std::time::Duration::from_millis(412);
        assert_eq!(creation_line("vst3:abc", took, true, true), "plug-in instance created: id=vst3:abc in 412 ms (with stored state)");
        assert_eq!(creation_line("clap:x", took, false, false), "plug-in instance failed: id=clap:x in 412 ms (no stored state)");
    }
    #[test]
    fn realtime_specs_list_builtins_and_clip_effects_and_instance_specs_only_hosted_ones() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("room_reverb"));
        s.track_mut(t).unwrap().mixer.inserts[1] = Some(Insert::new("clap:test.x"));
        let clip = s.track(t).unwrap().clips()[0].id.0;
        s.edit.values.insert(format!("clip_fx.{clip}.comp.threshold"), -20.0);
        let specs = realtime_instance_specs(&s);
        let slots: Vec<(usize, &str)> = specs.iter().map(|x| (x.slot, x.id.as_str())).collect();
        assert_eq!(slots, [(0, "room_reverb"), (1, "clap:test.x"), (clip_fx_slot(clip, true).unwrap(), CLIP_COMP)]);
        assert!(specs.iter().all(|x| x.state.is_none()));
        assert_eq!(instance_specs(&s).iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["clap:test.x"]);
    }
}
