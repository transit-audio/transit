//! The state handle and the editor handle, driven on the plug-in's main thread.

use super::*;

impl transit_dsp::PluginStateReader for ClapState {
    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.link.save_state()
    }

    /// `clap.state` `load` on the main thread, which CLAP allows while the plug-in processes (the
    /// plug-in synchronizes with its audio thread). The instance is held for the call, so the
    /// audio side skips the blocks meanwhile. As `Plugin::load_state`, the load is the host's own
    /// change: its values become the host's view, nothing is reported as an edit, and parameter
    /// values still queued on the audio side from before it are dropped (#78).
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
        shared.loads.fetch_add(1, Ordering::AcqRel);
        let mut values = Vec::new();
        for ((p, id), view) in shared.info.params.iter().zip(&shared.ids).zip(shared.view.iter()) {
            if let Some(x) = core.inst.param_value(*id) {
                let v = p.clamp(x as f32);
                view.store(f64::from(v).to_bits(), Ordering::Relaxed);
                values.push((p.id.to_string(), v));
            }
        }
        drop(core);
        // Values the host set before the load are not news any more (as after `refresh_values`).
        shared.processed_gen.store(shared.host_gen.load(Ordering::Acquire), Ordering::Release);
        Some(values)
    }
}

/// The strip's model as the window shows it (issue #68).
fn header_view(h: &transit_dsp::EditorHeader) -> transit_rt::editor_window::HeaderView {
    transit_rt::editor_window::HeaderView {
        title: h.title.clone(),
        preset: h.preset.clone(),
        user: h.user.clone(),
        factory: h.factory.clone(),
        bypassed: h.bypassed,
        can_bypass: h.can_bypass,
        can_paste: h.can_paste,
        can_delete: h.can_delete,
        disabled: h.disabled.clone(),
        learning: h.learning,
    }
}

/// What a click in the strip asks of the host.
fn header_action(c: transit_rt::editor_window::HeaderClick) -> transit_dsp::HeaderAction {
    use transit_dsp::HeaderAction as A;
    use transit_rt::editor_window::HeaderClick as C;
    match c {
        C::User(i) => A::User(i),
        C::Factory(i) => A::Factory(i),
        C::Previous => A::Previous,
        C::Next => A::Next,
        C::Save => A::Save,
        C::SaveAs => A::SaveAs,
        C::Load => A::Load,
        C::Bypass => A::Bypass,
        C::Reset => A::Reset,
        C::Copy => A::Copy,
        C::Paste => A::Paste,
        C::Delete => A::Delete,
        C::ShowInChain => A::ShowInChain,
        C::LearnNext => A::LearnNext,
    }
}

impl PluginEditor for ClapEditor {
    fn open(&mut self) -> Result<(), String> {
        // Edits reported before the editor opened are stale.
        let _ = self.link.idle_headless();
        self.link.open(self.info.name)
    }

    fn close(&mut self) {
        self.link.close();
    }

    fn is_open(&self) -> bool {
        self.link.is_open()
    }

    fn idle(&mut self) -> Vec<(String, f32)> {
        // Issue #205: inside a host call into plug-in code on this thread nothing here may call
        // the plug-in (the service, `on_main_thread`); the edits stay queued for the next idle.
        if transit_dsp::host_call::active() {
            transit_dsp::host_call::defer();
            return Vec::new();
        }
        let shared = self.shared.upgrade();
        if let Some(s) = &shared {
            s.service(&mut Serviced::default());
        }
        let raw = self.link.idle();
        let _ = self.link.host().dirty.swap(false, Ordering::Relaxed);
        // Only the latest value per parameter: (index, the session's value, the plug-in's).
        let mut latest: Vec<(usize, f32, f64)> = Vec::new();
        for (cid, x) in raw {
            let Some(i) = self.ids.iter().position(|&c| c == cid) else { continue };
            let Some(p) = self.info.params.get(i) else { continue };
            let v = p.clamp(x as f32);
            match latest.iter_mut().find(|e| e.0 == i) {
                Some(e) => (e.1, e.2) = (v, x),
                None => latest.push((i, v, x)),
            }
        }
        let mut out: Vec<(String, f32)> = Vec::with_capacity(latest.len());
        for (i, v, x) in latest {
            // #651: the session sends `v` back; the audio side then recognizes it as `x`.
            if let Some(s) = &shared {
                s.note_report(i, v, x);
            }
            if let Some(p) = self.info.params.get(i) {
                out.push((p.id.to_string(), v));
            }
        }
        out
    }

    fn set_header(&mut self, header: &transit_dsp::EditorHeader) {
        self.link.header().set(&header_view(header));
    }

    fn take_header_actions(&mut self) -> Vec<transit_dsp::HeaderAction> {
        self.link.header().take_clicks().into_iter().map(header_action).collect()
    }
}
