//! The device lifetime boundary. Fakes replace only the device operations; reconnect and editor
//! retry logic use the same ordering as physical devices.

use std::{cell::RefCell, rc::Rc};
use transit_dsp::{EditorWindowRequest, EditorWindowState};
use transit_model::TrackId;
use transit_playback::{AudioConfig, Player, record::Recorder, telemetry::CallbackTelemetry};

pub(crate) trait Editors {
    fn close_editors(&self) -> Result<(), String>;
    fn editor_open(&self, track: TrackId, slot: usize) -> bool;
    fn open_editor(&self, track: TrackId, slot: usize) -> Result<(), String>;
    /// Issue #173: posts a grouping request to the editors' threads; never waits.
    fn editor_windows(&self, _request: EditorWindowRequest) {}
    /// Brings this slot's open editor window to the front, unactivated; never waits.
    fn raise_editor(&self, _track: TrackId, _slot: usize) {}
    /// This slot's editor window state as its thread last reported it.
    fn editor_window(&self, _track: TrackId, _slot: usize) -> Option<EditorWindowState> {
        None
    }
}

pub(crate) trait Connection: Editors {
    type Input;
    fn close_input(&self, input: &mut Option<Self::Input>) -> Result<(), String>;
    fn open_input(&self, cfg: &AudioConfig) -> Result<Self::Input, String>;
    fn monitor(&self, input: &Self::Input) -> Result<(), String>;
    fn monitored_input(&self, cfg: &AudioConfig) -> Result<Self::Input, String> {
        let r = self.open_input(cfg)?;
        self.monitor(&r)?;
        Ok(r)
    }
}

impl Editors for Player {
    fn close_editors(&self) -> Result<(), String> {
        self.close_editors().map_err(|e| e.to_string())
    }
    fn editor_open(&self, track: TrackId, slot: usize) -> bool {
        self.editor_open(track, slot)
    }
    fn open_editor(&self, track: TrackId, slot: usize) -> Result<(), String> {
        self.open_editor(track, slot)
    }
    fn editor_windows(&self, request: EditorWindowRequest) {
        let _posted = self.editor_windows(request);
    }
    fn raise_editor(&self, track: TrackId, slot: usize) {
        self.raise_editor(track, slot);
    }
    fn editor_window(&self, track: TrackId, slot: usize) -> Option<EditorWindowState> {
        self.editor_window(track, slot)
    }
}

impl Connection for Player {
    type Input = Recorder;
    fn close_input(&self, input: &mut Option<Recorder>) -> Result<(), String> {
        self.close_input(input).map_err(|e| e.to_string())
    }
    fn open_input(&self, cfg: &AudioConfig) -> Result<Recorder, String> {
        self.open_input(cfg)
    }
    fn monitor(&self, r: &Recorder) -> Result<(), String> {
        self.set_input(std::sync::Arc::clone(&r.monitor)).map(|_| ()).map_err(|e| e.to_string())
    }
}

pub(crate) fn replace<P: Connection>(player: &mut Option<P>, input: &mut Option<P::Input>, open: impl FnOnce() -> P) -> Result<(), String> {
    if let Some(p) = player.as_ref() {
        p.close_editors()?;
        p.close_input(input)?;
    }
    if input.is_some() {
        return Err("Cannot replace audio while an input is still owned by the caller".into());
    }
    // Assignment must finish (including Drop) before the loader runs.
    *player = None;
    *player = Some(open());
    Ok(())
}

pub(crate) type Trace = Rc<RefCell<Vec<&'static str>>>;

#[derive(Debug)]
pub(crate) struct FakeConnection {
    pub config: AudioConfig,
    pub telemetry: CallbackTelemetry,
    pub failed: bool,
    pub editors: RefCell<Vec<(TrackId, usize)>>,
    /// The fake host thread's editor windows (issue #173).
    pub windows: RefCell<FakeWindows>,
    trace: Trace,
}

impl FakeConnection {
    pub fn new(config: AudioConfig, trace: Trace) -> Self {
        trace.borrow_mut().push("open output");
        Self { config, telemetry: CallbackTelemetry::default(), failed: false, editors: RefCell::default(), windows: RefCell::default(), trace }
    }
}

/// What the fake host thread was asked of its editor windows (issue #173), applied as the real
/// host thread applies it (`transit_rt::editor_window::group`), or held in `queued` while the
/// fake thread is `held` inside a plug-in.
#[derive(Debug, Default)]
pub(crate) struct FakeWindows {
    pub follow: bool,
    /// Parked editors (hidden with the minimized main window); every other open editor is shown.
    pub parked: Vec<(TrackId, usize)>,
    /// Every request posted, in order, applied or not.
    pub posted: Vec<EditorWindowRequest>,
    /// Every raise asked for, applied or not.
    pub raised: Vec<(TrackId, usize)>,
    /// The fake host thread is stuck inside a plug-in: nothing posted is applied until it is not.
    pub held: bool,
    pub queued: Vec<EditorWindowRequest>,
}

impl FakeWindows {
    fn apply(&mut self, request: EditorWindowRequest, open: &[(TrackId, usize)]) {
        match request {
            EditorWindowRequest::Follow(on) => {
                self.follow = on;
                if !on {
                    self.parked.clear();
                }
            }
            EditorWindowRequest::Park if self.follow => self.parked = open.to_vec(),
            EditorWindowRequest::Unpark => self.parked.clear(),
            EditorWindowRequest::Park | EditorWindowRequest::Front => {}
        }
    }

    /// The fake thread returns from the plug-in: what was queued is applied, in order.
    #[cfg(test)]
    pub fn release(&mut self, open: &[(TrackId, usize)]) {
        self.held = false;
        for r in std::mem::take(&mut self.queued) {
            self.apply(r, open);
        }
    }
}

impl Drop for FakeConnection {
    fn drop(&mut self) {
        self.trace.borrow_mut().push("drop output");
    }
}

#[derive(Debug)]
pub(crate) struct FakeInput(Trace);

impl Drop for FakeInput {
    fn drop(&mut self) {
        self.0.borrow_mut().push("drop input");
    }
}

impl Editors for FakeConnection {
    fn close_editors(&self) -> Result<(), String> {
        self.trace.borrow_mut().push("close editors");
        self.editors.borrow_mut().clear();
        Ok(())
    }
    fn editor_open(&self, track: TrackId, slot: usize) -> bool {
        self.editors.borrow().contains(&(track, slot))
    }
    fn open_editor(&self, track: TrackId, slot: usize) -> Result<(), String> {
        self.trace.borrow_mut().push("open editor");
        self.editors.borrow_mut().push((track, slot));
        Ok(())
    }
    fn editor_windows(&self, request: EditorWindowRequest) {
        let mut w = self.windows.borrow_mut();
        w.posted.push(request);
        if w.held {
            w.queued.push(request);
        } else {
            w.apply(request, &self.editors.borrow());
        }
    }
    fn raise_editor(&self, track: TrackId, slot: usize) {
        self.windows.borrow_mut().raised.push((track, slot));
    }
    fn editor_window(&self, track: TrackId, slot: usize) -> Option<EditorWindowState> {
        let key = (track, slot);
        let w = self.windows.borrow();
        self.editors.borrow().contains(&key).then(|| if w.parked.contains(&key) { EditorWindowState::Parked } else { EditorWindowState::Shown })
    }
}

impl Connection for FakeConnection {
    type Input = FakeInput;
    fn close_input(&self, input: &mut Option<FakeInput>) -> Result<(), String> {
        *input = None;
        Ok(())
    }
    fn open_input(&self, _cfg: &AudioConfig) -> Result<FakeInput, String> {
        self.trace.borrow_mut().push("open input");
        Ok(FakeInput(Rc::clone(&self.trace)))
    }
    fn monitor(&self, _input: &FakeInput) -> Result<(), String> {
        self.trace.borrow_mut().push("connect monitor");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::cell::Cell;
    use transit_playback::PlayerError;

    struct RefusingConnection {
        inner: FakeConnection,
        refusal: Cell<Option<PlayerError>>,
    }

    impl Editors for RefusingConnection {
        fn close_editors(&self) -> Result<(), String> {
            self.inner.close_editors()
        }
        fn editor_open(&self, track: TrackId, slot: usize) -> bool {
            self.inner.editor_open(track, slot)
        }
        fn open_editor(&self, track: TrackId, slot: usize) -> Result<(), String> {
            self.inner.open_editor(track, slot)
        }
    }

    impl Connection for RefusingConnection {
        type Input = FakeInput;
        fn close_input(&self, input: &mut Option<FakeInput>) -> Result<(), String> {
            if let Some(error) = self.refusal.get() {
                return Err(error.to_string());
            }
            self.inner.close_input(input)
        }
        fn open_input(&self, cfg: &AudioConfig) -> Result<FakeInput, String> {
            self.inner.open_input(cfg)
        }
        fn monitor(&self, input: &FakeInput) -> Result<(), String> {
            self.inner.monitor(input)
        }
    }

    #[test]
    fn refused_input_return_keeps_both_owners_and_never_opens_a_replacement() {
        for refusal in [PlayerError::Busy, PlayerError::RecordingActive] {
            let trace = Trace::default();
            let mut player =
                Some(RefusingConnection { inner: FakeConnection::new(AudioConfig::default(), Rc::clone(&trace)), refusal: Cell::new(Some(refusal)) });
            let mut input = Some(player.as_ref().unwrap().open_input(&AudioConfig::default()).unwrap());
            trace.borrow_mut().clear();
            let open = || RefusingConnection { inner: FakeConnection::new(AudioConfig::default(), Rc::clone(&trace)), refusal: Cell::new(None) };
            assert_eq!(replace(&mut player, &mut input, open), Err(refusal.to_string()));
            assert!(player.is_some() && input.is_some());
            assert_eq!(*trace.borrow(), ["close editors"]);
            // Main becomes available, or the caller has finished and collected the take.
            player.as_ref().unwrap().refusal.set(None);
            trace.borrow_mut().clear();
            replace(&mut player, &mut input, open).unwrap();
            assert_eq!(*trace.borrow(), ["close editors", "drop input", "drop output", "open output"]);
            assert!(input.is_none());
        }
    }
}
