//! The state handle: saves, and loads into the live instance on its main thread.

use super::*;

impl transit_dsp::PluginStateReader for Vst3State {
    fn save_state(&mut self) -> Option<Vec<u8>> {
        let to_controller = self.shared.upgrade().map(|s| Arc::clone(&s.to_controller));
        // A parameter only the controller stores (no processor state) would otherwise be saved
        // with the controller's stale value. The host's plain value converts through the
        // controller itself here (this thread may call it), so it is exact even for a curve
        // nobody sampled yet.
        for (i, u) in self.unsynced.iter().enumerate() {
            let bits = u.swap(SYNCED, Ordering::Relaxed);
            if bits != SYNCED
                && let (Some(&id), Some(n)) = (self.conv.ids.get(i), self.conv.normalized(i, f64::from_bits(bits) as f32))
            {
                self.conv.link.sync_param(id, n);
                // Handed over: the service need not send it again (review N1).
                if let Some(f) = to_controller.as_ref().and_then(|t| t.get(i)) {
                    f.store(false, Ordering::Release);
                }
            }
        }
        self.conv.link.save_state()
    }

    /// Loads into the live instance on its main thread while it processes (VST3 has the host
    /// call `setState` on the UI thread). The instance is held for the load, so the audio side
    /// plays the blocks meanwhile as silence with fades, as during a reactivation. As
    /// `Plugin::load_state`, the load is the host's own change: nothing is reported as an edit,
    /// and the values the host set before it are dropped, including any still queued on the audio
    /// side (#78; the caller stores the returned ones).
    fn load_state(&mut self, data: &[u8]) -> Option<transit_dsp::LoadedValues> {
        let shared = self.shared.upgrade()?;
        if std::thread::current().id() != shared.main {
            log::warn!("{}: a state load off the instance's main thread was refused", shared.info.id);
            return None;
        }
        let core = lock(&shared.core);
        if !core.inst.load_state(data) {
            return None;
        }
        shared.loads.bump();
        if shared.host.midi_cc_changed.swap(false, Ordering::Acquire) {
            shared.map.publish(&core.inst.midi_mapping());
        }
        shared.host.values_changed.store(false, Ordering::Relaxed);
        shared.remember_values(&core.inst);
        for u in self.unsynced.iter() {
            u.store(SYNCED, Ordering::Relaxed);
        }
        for f in shared.to_controller.iter() {
            f.store(false, Ordering::Release);
        }
        let values = (0..self.conv.ids.len())
            .filter_map(|i| {
                let (pi, &id) = (self.conv.info.params.get(i)?, self.conv.ids.get(i)?);
                let n = core.inst.normalized_value(id)?;
                let v = match self.conv.maps.get(i)? {
                    Mapping::Stepped(steps) => (n * steps).round(),
                    Mapping::Plain => core.inst.to_plain(id, n).unwrap_or(f64::from(pi.min) + n * f64::from(pi.max - pi.min)),
                    Mapping::Linear => f64::from(pi.min) + n * f64::from(pi.max - pi.min),
                };
                v.is_finite().then(|| (pi.id.to_string(), pi.clamp(v as f32)))
            })
            .collect();
        Some(values)
    }
}
