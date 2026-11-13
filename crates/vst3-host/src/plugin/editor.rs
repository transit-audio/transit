//! The controller service and editor handle, driven on the plug-in main thread.

use super::*;

impl PluginEditor for Vst3Editor {
    fn has_gui(&self) -> bool {
        self.conv.link.has_view()
    }

    fn open(&mut self) -> Result<(), String> {
        // Edits reported before the editor opened (e.g. during setup) are stale.
        let _ = self.host.take_edits();
        self.conv.link.open(self.conv.info.name)
    }

    fn close(&mut self) {
        self.conv.link.close();
    }

    fn is_open(&self) -> bool {
        self.conv.link.is_open()
    }

    /// Also the UI-thread hook for the lazy curve sampling (at most `CURVE_BUDGET_TABLES` curves or
    /// `CURVE_BUDGET_TIME` per call) and for `restartComponent` (see the module docs): the host
    /// polls every editor handle each UI frame, open or not.
    fn idle(&mut self) -> Vec<(String, f32)> {
        // Issue #205: inside a host call into plug-in code on this thread nothing here may call
        // the plug-in (the service, the view's housekeeping, the curve sampling); the edits stay
        // queued for the next idle.
        if transit_dsp::host_call::active() {
            transit_dsp::host_call::defer();
            return Vec::new();
        }
        let shared = self.shared.upgrade();
        if let Some(s) = &shared {
            s.service(&mut Serviced::default());
        }
        self.conv.link.idle();
        self.curves.step(CURVE_BUDGET_TABLES, CURVE_BUDGET_TIME, |id, n| self.conv.link.to_plain(id, n));
        let mut out = Vec::new();
        for (vid, n) in self.host.take_edits() {
            let Some(i) = self.conv.ids.iter().position(|&x| x == vid) else { continue };
            if let (Some(p), Some(v)) = (self.conv.info.params.get(i), self.conv.plain(i, n)) {
                // #651: the session sends `v` back; the audio side then recognizes it as this value.
                if let Some(s) = &shared {
                    s.note_report(i, v, n);
                }
                out.push((p.id.to_string(), v));
            }
        }
        out
    }

    fn set_param(&mut self, id: &str, value: f32) {
        let Some(i) = self.conv.param_index.get(id).copied() else { return };
        if let (Some(&vid), Some(n)) = (self.conv.ids.get(i), self.conv.normalized(i, value)) {
            self.conv.link.set_normalized(vid, n);
        }
    }

    fn set_header(&mut self, header: &transit_dsp::EditorHeader) {
        self.conv.link.header().set(&header_view(header));
    }

    fn take_header_actions(&mut self) -> Vec<transit_dsp::HeaderAction> {
        self.conv.link.header().take_clicks().into_iter().map(header_action).collect()
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
