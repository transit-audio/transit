//! Editor/state-reader ownership follows confirmed SlotBinding, never a slot name.
//!
//! Hosted (third-party) instances are made on the plug-in host thread (ADR 0018,
//! `transit-plugin-host`): a request is announced for a frame ("Loading…"), submitted, and polled
//! once per service call; the compile takes the instance when it is ready. The editor and state
//! handles captured here are the host's proxies, so opening an editor, idling it and reading a
//! state go to the host thread too, and a retired instance is destroyed there.
//!
//! **Late instances are adopted, never discarded** (issue #82, escape E48). A request waits for a
//! new instance at most [`DEFAULT_SLOW_AFTER`]; past that it is compiled with the slot vacant,
//! the slot reads "Loading… (slow)", and when the instance is made it is taken into the next plan
//! (the vacant slot is made again, `PlanCompiler::retry_vacant`). That wait only decides what the
//! slot shows and how long the request's other edits wait. Only the plug-in host's wedge
//! detection (its creation budget, ADR 0018) makes the slot read "Not responding", and even then
//! a creation that returns is adopted. A "Not responding" slot can be retried
//! ([`Handles::request_retry`]): a new creation on the host's current thread, restoring the state
//! the session stores for the slot.
use super::*;
use crate::{EditorEdit, PluginStates};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use transit_dsp::{EditorHeader, HeaderAction, JobPoll, LoadedValues, PluginEditor, PluginStateReader, StateBusy, StateJob};
use transit_mix::InstanceSpec;
use transit_model::TrackId;
use transit_plugin_host::{Ctx, Host, LoadPhase, Pending, PhaseLog, Status};

/// How long one save waits, in all, for plug-ins whose thread is held by a modal dialog (issue
/// #56). Such a plug-in that has not answered by then keeps its stored state and the status line
/// names it; a wedged thread's plug-ins answer at once. A plug-in that is answering is waited for
/// however long it takes (up to the host's request timeout).
pub const DEFAULT_SAVE_DEADLINE: Duration = Duration::from_millis(500);

/// How long a session request waits for a new instance before it is compiled without it (the
/// slot vacant until the instance is ready), and after which the slot reads "Loading… (slow)"
/// (issue #82). It never abandons the creation.
pub const DEFAULT_SLOW_AFTER: Duration = Duration::from_secs(10);

/// Makes one instance for a spec, on whichever thread runs it (the plug-in host thread for hosted
/// plug-ins).
pub type Factory = Arc<dyn Fn(&InstanceSpec) -> Option<NewInstance> + Send + Sync>;

/// The time the loading display is measured on (a test sets a manual one).
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// A plug-in that was made on the host thread and taken into a plan (issue #67): how long it took
/// in all and in each phase, for the status line, the remembered load time and the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedIn {
    /// The session's plug-in id (the key of the remembered load time).
    pub id: String,
    /// The plug-in's name.
    pub name: &'static str,
    /// From the request to the plan taking it.
    pub total: Duration,
    /// Each phase it passed through, in order.
    pub phases: Vec<(LoadPhase, Duration)>,
}

/// An editor window that opened (issue #67), for the status line when it took long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorOpened {
    pub track: TrackId,
    pub slot: usize,
    pub name: &'static str,
    pub report: transit_dsp::EditorOpenReport,
}

/// What a slot whose plug-in is not in a plan yet shows (issue #82; #67 adds the time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotLoad {
    /// Being made, for `elapsed` so far ("Loading…").
    Loading { elapsed: Duration },
    /// Being made for longer than [`DEFAULT_SLOW_AFTER`] ("Loading… (slow)"): the session plays
    /// on without it, and it joins the next plan when it is ready.
    Slow { elapsed: Duration },
    /// Its creation ran past the plug-in host's creation budget and its thread is wedged (ADR
    /// 0018): "Not responding", with Retry. If the creation still returns, it is adopted.
    NotResponding { elapsed: Duration },
}

impl SlotLoad {
    /// How long the instance has been on its way.
    #[must_use]
    pub fn elapsed(self) -> Duration {
        match self {
            SlotLoad::Loading { elapsed } | SlotLoad::Slow { elapsed } | SlotLoad::NotResponding { elapsed } => elapsed,
        }
    }
}

/// Which ids go to the plug-in host thread (the rest are made in the compile).
pub type Deferred = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Test seams for the hosted-instance path of a real Player (the UI tests drive the slot text and
/// Retry through them): the host, the factory, which ids go to the host, the display clock and
/// budget.
#[cfg(any(test, feature = "test-hooks"))]
pub struct PluginTestHooks {
    pub host: Host,
    pub factory: Factory,
    pub deferred: Deferred,
    pub clock: Clock,
    pub slow_after: Duration,
}

/// Whether two specs are the same instance in the compiler's eyes: the slot and the plug-in (id,
/// uid and channels), whatever state they restore.
fn same_instance(a: &InstanceSpec, b: &InstanceSpec) -> bool {
    a.track == b.track && a.slot == b.slot && a.id == b.id && a.uid == b.uid && a.ch == b.ch
}

/// What a creation job on the host thread returns: `Err` when the factory unwound.
type Made = Result<Option<NewInstance>, ()>;

/// The job that makes `spec` on the host thread and adopts the instance there. It marks the
/// load's phases on `phases` at `clock`'s time (issue #67): creating from its start, restoring
/// when the factory reports it, binding once the instance is made.
fn creation(spec: InstanceSpec, factory: Factory, phases: Arc<PhaseLog>, clock: Clock) -> impl FnOnce(&mut Ctx) -> Made + Send + 'static {
    move |ctx| {
        // A creation that never returns is logged with its plug-in (issue #205).
        ctx.name_job(&spec.id);
        phases.enter(LoadPhase::Creating, clock());
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transit_plugin_host::phase_scope(&phases, &clock, || factory(&spec)).map(|mut made| {
                made.plugin = ctx.adopt(made.plugin);
                phases.enter(LoadPhase::Binding, clock());
                made
            })
        }))
        .map_err(|payload| {
            if let Err(secondary) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload))) {
                std::mem::forget(secondary);
            }
        })
    }
}

struct Entry {
    editor: Option<Box<dyn PluginEditor>>,
    state: Option<Box<dyn PluginStateReader>>,
    refused: bool,
    /// The plug-in has a sidechain input (read once, at creation).
    key_input: bool,
    /// The plug-in's name, for the status line.
    name: &'static str,
    /// The session holds a state for it: restored at creation, or captured since. Until then a
    /// save waits for its read even while its thread is held by a modal dialog (issue #56).
    stored: bool,
}

/// Where a third-party instance the next plan needs is on its way to being made.
enum Stage {
    /// Seen in the latest request; not yet announced to the UI.
    Queued,
    /// Announced for a frame (the UI shows it as loading); submitted on the next service.
    Announced,
    /// Being made on the plug-in host thread.
    Requested(Pending<Made>),
    /// Made (or refused or failed: `None`), waiting for the compile to take it.
    Ready(Option<NewInstance>),
}

struct Staged {
    spec: InstanceSpec,
    stage: Stage,
    /// When it was first asked for (or retried), on the handles' clock.
    since: Instant,
    /// The phases its creation has passed through (issue #67).
    phases: Arc<PhaseLog>,
    /// Its creation ran past the host's creation budget: the slot reads "Not responding".
    stuck: bool,
    /// The user asked to retry it ([`Handles::request_retry`]); done on the next service.
    retry: bool,
}

impl Staged {
    fn new(spec: InstanceSpec, stage: Stage, since: Instant) -> Staged {
        Staged { spec, stage, since, phases: PhaseLog::new(since), stuck: false, retry: false }
    }
    fn at(&self, track: TrackId, slot: usize) -> bool {
        self.spec.track == track && self.spec.slot == slot
    }
    fn waiting(&self) -> bool {
        !matches!(self.stage, Stage::Ready(_))
    }
}

pub struct Handles {
    /// Third-party instances made ahead of the compile on the host thread (see [`Handles::stage`]).
    staged: Vec<Staged>,
    /// Instances whose slots were compiled vacant while they are made on the host thread: one a
    /// compile asked for that `stage` did not predict, and one still being made after the display
    /// budget (issue #82). When one is ready, the request is compiled again with it (see
    /// [`Handles::retry`]). One whose creation ran past the host's creation budget is marked
    /// stuck ("Not responding") and is still taken if it returns.
    retrying: Vec<Staged>,
    /// A retried instance is ready and no request is pending: the owner requests its session
    /// again ([`Handles::take_republish`]).
    republish: bool,
    /// How long a request waits for a new instance before it compiles without it, and after
    /// which the slot reads "Loading… (slow)" ([`DEFAULT_SLOW_AFTER`]).
    slow_after: Duration,
    /// The loading display's time ([`Instant::now`]; a test sets a manual clock).
    clock: Clock,
    /// Test seams for a real Player ([`PluginTestHooks`]): the factory and which ids go to the
    /// host thread.
    #[cfg(any(test, feature = "test-hooks"))]
    test_factory: Option<(Factory, Deferred)>,
    /// Why editors that accepted an open request then failed, for the status line.
    editor_errors: Vec<(TrackId, usize, String)>,
    /// Plug-ins taken into a plan since the UI last asked ([`Self::take_loaded`]).
    loaded: Vec<LoadedIn>,
    /// Editors that finished opening since the UI last asked ([`Self::take_editor_opened`]).
    editor_opened: Vec<EditorOpened>,
    /// How long one [`Self::capture_states`] may wait for plug-ins in all.
    save_deadline: Duration,
    /// The plug-ins whose state the last capture could not read in time (their stored state is
    /// kept), for the status line.
    unrefreshed: Vec<String>,
    host: Host,
    /// Keyed by (track, generation): the generation is the instance's identity and survives a
    /// reorder, while the slot it sits at does not.
    entries: BTreeMap<(TrackId, SlotGen), Entry>,
    active: BTreeMap<(TrackId, usize), SlotGen>,
    generation: PlanGeneration,
    main_thread: PhantomData<Rc<()>>,
}

impl Default for Handles {
    fn default() -> Self {
        Handles::with_host(transit_plugin_host::shared().clone())
    }
}

impl Handles {
    /// Handles whose hosted instances are made on `host` (tests use their own).
    pub(crate) fn with_host(host: Host) -> Self {
        Handles {
            staged: Vec::new(),
            retrying: Vec::new(),
            republish: false,
            slow_after: DEFAULT_SLOW_AFTER,
            clock: Arc::new(Instant::now),
            #[cfg(any(test, feature = "test-hooks"))]
            test_factory: None,
            editor_errors: Vec::new(),
            loaded: Vec::new(),
            editor_opened: Vec::new(),
            save_deadline: DEFAULT_SAVE_DEADLINE,
            unrefreshed: Vec::new(),
            host,
            entries: BTreeMap::new(),
            active: BTreeMap::new(),
            generation: PlanGeneration::default(),
            main_thread: PhantomData,
        }
    }

    /// Installs the test seams ([`PluginTestHooks`]): later instances are made by its factory on
    /// its host, and the loading display runs on its clock and budget.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_test_hooks(&mut self, hooks: PluginTestHooks) {
        self.host = hooks.host;
        self.test_factory = Some((hooks.factory, hooks.deferred));
        self.set_load_clock(hooks.clock, hooks.slow_after);
    }

    /// The loading display's clock and budget (tests).
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn set_load_clock(&mut self, clock: Clock, slow_after: Duration) {
        self.clock = clock;
        self.slow_after = slow_after;
    }

    fn now(&self) -> Instant {
        (self.clock)()
    }

    /// Normal Player preparation: hosted instances are made on the plug-in host thread,
    /// built-ins (cheap) on this, the owner thread, during the compile.
    pub fn service<T: Copy, R: Copy>(&mut self, control: &mut Control<T, R>) {
        let target = control.sessions.target;
        let factory: Factory = Arc::new(move |spec: &InstanceSpec| {
            let restoring = || transit_plugin_host::report_phase(LoadPhase::Restoring);
            transit_mix::create_instance_with(
                &spec.id,
                spec.state.as_deref(),
                target.sample_rate as f32,
                target.max_block as usize,
                spec.ch,
                restoring,
            )
        });
        // The test seam makes only the ids it sends to the host thread; the rest as always.
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some((test, deferred)) = self.test_factory.clone() {
            let which = Arc::clone(&deferred);
            let both: Factory = Arc::new(move |spec: &InstanceSpec| if which(&spec.id) { test(spec) } else { factory(spec) });
            self.service_with(control, &*deferred, &both);
            return;
        }
        self.service_with(control, &transit_mix::is_third_party, &factory);
    }

    /// [`Self::service`] with the factory and the choice of what goes to the host thread
    /// supplied (the tests have no plug-in hosts to load).
    pub(super) fn service_with<T: Copy, R: Copy>(&mut self, control: &mut Control<T, R>, deferred: &dyn Fn(&str) -> bool, factory: &Factory) {
        self.host.check();
        control.sessions.reclaim(false);
        self.restart(factory);
        self.retry(&mut control.sessions);
        control.sessions.hold = self.stage(&mut control.sessions, deferred, factory);
        control.service(&mut |spec, generation| {
            let ready = self.staged.iter().position(|st| st.spec == *spec && matches!(st.stage, Stage::Ready(_)));
            let mut timed = None;
            let made = match ready.map(|i| self.staged.swap_remove(i)) {
                Some(Staged { stage: Stage::Ready(made), phases, since, .. }) => {
                    timed = Some((phases, since));
                    made
                }
                // Still being made past the display budget, or not responding: the slot stays
                // vacant and the instance joins the plan that follows its arrival (issue #82).
                _ if self.retrying.iter().any(|st| st.spec == *spec) => None,
                // Not predicted by `stage` (a slot that moved for layout reasons): submitted to
                // the host thread; the slot is compiled vacant and loads like an announced one.
                _ if deferred(&spec.id) => self.make_later(spec, factory),
                _ => factory(spec),
            }?;
            let name = made.plugin.info().name;
            let bound = self.capture_stored((spec.track, spec.slot, generation), made, spec.state.is_some());
            if let (Some((phases, since)), true) = (timed, bound.is_some()) {
                self.record_loaded(&spec.id, name, &phases, since);
            }
            bound
        });
        self.refresh(&mut control.sessions);
    }

    /// An instance the compile took (issue #67): its phases and total are logged at info level and
    /// kept for the UI ([`Self::take_loaded`]). Runs on the control thread, never the audio thread.
    fn record_loaded(&mut self, id: &str, name: &'static str, phases: &PhaseLog, since: Instant) {
        let now = self.now();
        let phases = phases.finish(now);
        let total = now.saturating_duration_since(since);
        for line in transit_plugin_host::log_lines(name, total, &phases) {
            log::info!("{line}");
        }
        self.loaded.push(LoadedIn { id: id.to_owned(), name, total, phases });
    }

    /// The plug-ins taken into a plan since the last call, each once.
    pub fn take_loaded(&mut self) -> Vec<LoadedIn> {
        std::mem::take(&mut self.loaded)
    }

    /// A hosted instance the compile asks for that `stage` did not predict (issue #56): it is
    /// submitted to the host thread and the compile leaves its slot vacant, reading "Loading…";
    /// [`Self::retry`] compiles the request again once it is ready. The UI never waits for it.
    /// Where the host runs inline (macOS) it is made here, as before.
    fn make_later(&mut self, spec: &InstanceSpec, factory: &Factory) -> Option<NewInstance> {
        self.retrying.retain(|st| st.spec != *spec);
        let since = self.now();
        let mut staged = Staged::new(spec.clone(), Stage::Queued, since);
        let pending = self.host.submit(creation(spec.clone(), Arc::clone(factory), Arc::clone(&staged.phases), Arc::clone(&self.clock)));
        match pending.poll() {
            Status::Ready(Ok(made)) => {
                // Made before this call returned (a quick plug-in, or the inline host): taken by
                // the compile that asked, so its time is reported here.
                if let Some(m) = &made {
                    self.record_loaded(&spec.id, m.plugin.info().name, &staged.phases, since);
                }
                made
            }
            Status::Ready(Err(())) => std::panic::resume_unwind(Box::new("a plug-in factory panicked on the host thread")),
            Status::Failed => None,
            Status::Waiting { .. } | Status::Stuck => {
                log::info!("{}: not predicted by staging; made on the plug-in host thread, its slot loads meanwhile", spec.id);
                staged.stage = Stage::Requested(pending);
                self.retrying.push(staged);
                None
            }
        }
    }

    /// Runs the retries the user asked for ([`Self::request_retry`]): each slot's creation starts
    /// again on the host's current thread (a fresh one took over from the wedged thread, ADR
    /// 0018), from the spec the request holds, so the stored state is restored. The abandoned
    /// creation, if it ever returns, is destroyed on its own thread.
    fn restart(&mut self, factory: &Factory) {
        let now = self.now();
        for st in self.retrying.iter_mut().filter(|st| st.retry) {
            log::info!("{}: retrying; a new instance is made with the stored state", st.spec.id);
            st.phases = PhaseLog::new(now);
            st.stage =
                Stage::Requested(self.host.submit(creation(st.spec.clone(), Arc::clone(factory), Arc::clone(&st.phases), Arc::clone(&self.clock))));
            st.since = now;
            st.stuck = false;
            st.retry = false;
        }
    }

    /// Polls the instances whose slots were compiled vacant ([`Self::make_later`], and the slow
    /// ones `stage` handed over). One that is ready goes to the staged list for the next compile,
    /// which is asked to make its vacant slot again; with no request pending, the owner is asked
    /// to request its session again ([`Self::take_republish`]). A failed one leaves its slot
    /// empty, as a failed factory does. One past the host's creation budget is marked stuck and
    /// kept: if it returns, it is adopted like any other (issue #82).
    fn retry(&mut self, sessions: &mut SessionControl) {
        let now = self.now();
        let slow_after = self.slow_after;
        for st in &mut self.retrying {
            let Stage::Requested(pending) = &st.stage else { continue };
            let elapsed = now.saturating_duration_since(st.since);
            match pending.poll() {
                Status::Waiting { .. } => {}
                Status::Ready(Ok(made)) => {
                    if made.is_some() && (st.stuck || elapsed >= slow_after) {
                        log::info!("{}: loaded after {:.1} s", st.spec.id, elapsed.as_secs_f64());
                    }
                    st.stuck = false;
                    st.stage = Stage::Ready(made);
                }
                Status::Ready(Err(())) => {
                    st.stage = Stage::Ready(None);
                    sessions.fault();
                }
                Status::Failed => {
                    log::error!("{}: the plug-in host could not make it", st.spec.id);
                    st.stage = Stage::Ready(None);
                }
                Status::Stuck if !st.stuck => {
                    log::error!(
                        "{}: not responding: not made after {:.0} s; the slot offers Retry, and the instance is still taken if it returns",
                        st.spec.id,
                        elapsed.as_secs_f64()
                    );
                    st.stuck = true;
                }
                Status::Stuck => {}
            }
        }
        self.retrying.retain(|st| !matches!(st.stage, Stage::Ready(None)));
        if !self.retrying.iter().any(|st| matches!(st.stage, Stage::Ready(Some(_)))) {
            return;
        }
        // `stage` keeps a staged instance only while a request it can compile wants it.
        let compilable = sessions.latest.is_some() && !sessions.faulted() && sessions.bridge.can_prepare();
        let mut i = 0;
        while i < self.retrying.len() {
            if !matches!(self.retrying[i].stage, Stage::Ready(Some(_))) {
                i += 1;
                continue;
            }
            if compilable {
                let st = self.retrying.swap_remove(i);
                sessions.bridge.retry_vacant(&st.spec);
                self.staged.retain(|s| s.spec != st.spec);
                self.staged.push(st);
            } else if sessions.latest.is_none() && sessions.bridge.can_prepare() && !sessions.bridge.retry_vacant(&self.retrying[i].spec) {
                // The active plan no longer has its vacant slot (the session changed): unwanted,
                // destroyed on its host thread. Otherwise the slot is marked for the next compile.
                self.retrying.swap_remove(i);
            } else {
                self.republish |= sessions.latest.is_none();
                i += 1;
            }
        }
    }

    /// Whether a retried instance waits for its session to be requested again; clears the wish.
    pub(crate) fn take_republish(&mut self) -> bool {
        std::mem::take(&mut self.republish)
    }

    /// The republish could not be requested now; ask again on the next service.
    pub(crate) fn keep_republish(&mut self) {
        self.republish = true;
    }

    /// Makes the third-party instances the latest request needs before it is compiled, so the
    /// compile finds them and the control thread never sits in a factory: the first call only
    /// announces them ([`Self::loading`] turns true and the frame is painted), the next submits
    /// them to the plug-in host thread, and each later call polls them; the request compiles in
    /// the call that finds the last one ready. The active plan keeps playing meanwhile and has no
    /// strip or slot for the new instance, which is silence. True while the request has to wait.
    ///
    /// The request waits at most the display budget ([`DEFAULT_SLOW_AFTER`]): an instance still
    /// being made then moves to the vacant-slot list (`retrying`), the request compiles without
    /// it, and it joins a later plan when it is ready (issue #82). The wait never abandons it.
    fn stage(&mut self, sessions: &mut SessionControl, deferred: &dyn Fn(&str) -> bool, factory: &Factory) -> bool {
        let now = self.now();
        // With a plan in flight the compiler's view is not current; wait for its return.
        let wanted: Vec<InstanceSpec> = match &sessions.latest {
            Some(request) if !sessions.faulted() && sessions.bridge.can_prepare() => {
                let all = sessions.restore.is_some();
                let specs = transit_mix::realtime_instance_specs(&request.session);
                // An instance on its way to a vacant slot is kept while the request has its slot
                // with the same plug-in (identity: id, uid, channels, as the compiler reuses the
                // vacant slot); one the request no longer has (removed, replaced) is dropped, and
                // destroyed on its host thread if it is made after all. The same plug-in with
                // another state (the instrument picked again: no state) starts again with the new
                // spec, since the compile keeps the vacant slot and stages nothing for it (review
                // B2 of #82).
                self.retrying.retain_mut(|st| match specs.iter().find(|spec| same_instance(spec, &st.spec)) {
                    None => false,
                    Some(spec) if *spec == st.spec => true,
                    Some(spec) => {
                        log::info!("{}: its slot changed while it loads; it is made again for the new state", spec.id);
                        st.spec = spec.clone();
                        st.retry = true;
                        true
                    }
                });
                let vacant = &self.retrying;
                specs
                    .into_iter()
                    .filter(|spec| deferred(&spec.id) && (all || !sessions.bridge.reuses(spec)) && !vacant.iter().any(|st| st.spec == *spec))
                    .collect()
            }
            _ => Vec::new(),
        };
        // What no request asks for any more dies here: a request not yet started is cancelled,
        // a running one finishes on the host thread and its instance is destroyed there.
        self.staged.retain(|st| wanted.contains(&st.spec));
        for spec in wanted {
            if !self.staged.iter().any(|st| st.spec == spec) {
                self.staged.push(Staged::new(spec, Stage::Queued, now));
            }
        }
        if self.staged.iter().any(|st| matches!(st.stage, Stage::Queued)) {
            for st in &mut self.staged {
                if matches!(st.stage, Stage::Queued) {
                    st.stage = Stage::Announced;
                }
            }
            return true;
        }
        // Inline (macOS) runs each job where it is submitted: one per call, as before.
        let inline = self.host.is_inline();
        for st in self.staged.iter_mut().filter(|st| matches!(st.stage, Stage::Announced)) {
            st.stage =
                Stage::Requested(self.host.submit(creation(st.spec.clone(), Arc::clone(factory), Arc::clone(&st.phases), Arc::clone(&self.clock))));
            if inline {
                break;
            }
        }
        for st in &mut self.staged {
            let Stage::Requested(pending) = &st.stage else { continue };
            match pending.poll() {
                Status::Waiting { .. } => {}
                Status::Ready(Ok(made)) => st.stage = Stage::Ready(made),
                Status::Ready(Err(())) => {
                    // A factory unwind is a session fault, as when the compile makes the instance.
                    st.stage = Stage::Ready(None);
                    sessions.fault();
                }
                Status::Failed => {
                    log::error!("{}: the plug-in host could not make it", st.spec.id);
                    st.stage = Stage::Ready(None);
                }
                // Past the host's creation budget (longer than the display budget, so normally
                // seen in `retry`): marked, and handed over below like a slow one.
                Status::Stuck => st.stuck = true,
            }
        }
        // The display budget: a creation still running no longer holds the request. Its slot is
        // compiled vacant and reads "Loading… (slow)"; the instance joins when it is made.
        let mut i = 0;
        while i < self.staged.len() {
            let st = &self.staged[i];
            let elapsed = now.saturating_duration_since(st.since);
            if matches!(st.stage, Stage::Requested(_)) && (st.stuck || elapsed >= self.slow_after) {
                log::warn!(
                    "{}: slow to load, still waiting ({:.1} s); the session goes on without it until it is ready",
                    st.spec.id,
                    elapsed.as_secs_f64()
                );
                let st = self.staged.swap_remove(i);
                self.retrying.push(st);
            } else {
                i += 1;
            }
        }
        self.staged.iter().any(Staged::waiting)
    }

    /// Whether the instance for this slot is announced or being made, and not yet in a plan
    /// (slow included; not one that is not responding).
    pub fn loading(&self, track: TrackId, slot: usize) -> bool {
        self.staged.iter().any(|st| st.at(track, slot) && st.waiting()) || self.retrying.iter().any(|st| st.at(track, slot) && !st.stuck)
    }

    /// Whether any instance is waiting to be made (or, made late, to be compiled in); one that is
    /// not responding does not count.
    pub fn any_loading(&self) -> bool {
        self.staged.iter().any(Staged::waiting) || self.retrying.iter().any(|st| !st.stuck)
    }

    /// What this slot shows while its plug-in is on its way (`None`: nothing pending for it):
    /// "Loading…", "Loading… (slow)" past the display budget, "Not responding" once the host's
    /// creation budget ran out (issue #82).
    pub fn slot_load(&self, track: TrackId, slot: usize) -> Option<SlotLoad> {
        let now = self.now();
        let st = self.staged.iter().filter(|st| st.waiting()).chain(&self.retrying).find(|st| st.at(track, slot))?;
        let elapsed = now.saturating_duration_since(st.since);
        Some(if st.stuck {
            SlotLoad::NotResponding { elapsed }
        } else if elapsed >= self.slow_after {
            SlotLoad::Slow { elapsed }
        } else {
            SlotLoad::Loading { elapsed }
        })
    }

    /// Which phase this slot's plug-in is in while it loads (issue #67; `None`: nothing pending).
    pub fn slot_phase(&self, track: TrackId, slot: usize) -> Option<LoadPhase> {
        self.staged.iter().filter(|st| st.waiting()).chain(&self.retrying).find(|st| st.at(track, slot)).map(|st| st.phases.current())
    }

    /// Whether this slot's plug-in did not finish loading within the host's creation budget: the
    /// slot is empty and the UI says it is not responding, with Retry.
    pub fn stuck(&self, track: TrackId, slot: usize) -> bool {
        self.retrying.iter().any(|st| st.at(track, slot) && st.stuck)
    }

    /// Whether any slot is stuck.
    pub fn any_stuck(&self) -> bool {
        self.retrying.iter().any(|st| st.stuck)
    }

    /// The slots that are not responding: (track, slot, plug-in id), for the status line's
    /// "Retry all".
    pub fn stuck_slots(&self) -> Vec<(TrackId, usize, String)> {
        self.retrying.iter().filter(|st| st.stuck).map(|st| (st.spec.track, st.spec.slot, st.spec.id.clone())).collect()
    }

    /// Asks for a new instance for a slot that is not responding (issue #82): on the next
    /// service its creation starts again on the host's current thread with the state the session
    /// stores. Not a document change. False when the slot is not stuck.
    pub fn request_retry(&mut self, track: TrackId, slot: usize) -> bool {
        let mut any = false;
        for st in self.retrying.iter_mut().filter(|st| st.at(track, slot) && st.stuck) {
            st.retry = true;
            any = true;
        }
        any
    }

    /// [`Self::request_retry`] for every slot that is not responding; how many.
    pub fn request_retry_all(&mut self) -> usize {
        let mut n = 0;
        for st in self.retrying.iter_mut().filter(|st| st.stuck) {
            st.retry = true;
            n += 1;
        }
        n
    }

    /// How many plug-in host threads are not responding now (the status line says so); it goes
    /// back down when a thread returns from the long call.
    pub fn host_wedged(&self) -> usize {
        self.host.wedged()
    }

    /// The calls the wedged host threads are stuck in, each naming its plug-in (issue #205).
    pub fn host_stuck(&self) -> Vec<transit_plugin_host::StuckCall> {
        self.host.stuck()
    }

    /// How many instances the compile did not predict are being made or wait for their
    /// compile (the tests check the path was taken).
    #[cfg(test)]
    pub(crate) fn retrying(&self) -> usize {
        self.retrying.len()
    }

    /// The plug-in host these handles use (the tests stall it).
    #[cfg(test)]
    pub(crate) fn host(&self) -> &Host {
        &self.host
    }

    /// Editor opens that failed after they were accepted: (track, slot, why), each once.
    pub fn take_editor_errors(&mut self) -> Vec<(TrackId, usize, String)> {
        std::mem::take(&mut self.editor_errors)
    }

    /// Take every possibly-panicking handle before committing the entry. On a factory
    /// unwind, local resources die on main and there is no partial visible binding.
    pub fn capture(&mut self, binding: SlotBinding, made: NewInstance) -> Option<NewInstance> {
        self.capture_stored(binding, made, false)
    }

    /// [`Self::capture`]; `stored`: the session holds a state for this instance (it was restored).
    pub fn capture_stored(&mut self, binding: SlotBinding, mut made: NewInstance, stored: bool) -> Option<NewInstance> {
        let key = (binding.0, binding.2);
        if self.entries.contains_key(&key) {
            return None;
        }
        let editor = made.plugin.editor();
        let state = made.plugin.state_reader();
        let key_input = made.plugin.has_key_input();
        let name = made.plugin.info().name;
        self.entries.insert(key, Entry { editor, state, refused: made.refused_state, key_input, name, stored });
        Some(made)
    }

    pub fn refresh(&mut self, sessions: &mut SessionControl) {
        self.release(sessions.take_released());
        let generation = sessions.bridge.active();
        if generation != self.generation {
            self.active.clear();
            for (track, slot, generation) in sessions.active_bindings() {
                self.active.insert((track, slot), generation);
            }
            self.generation = generation;
        }
    }

    pub fn release(&mut self, bindings: impl IntoIterator<Item = SlotBinding>) {
        for binding in bindings {
            if let Some(mut entry) = self.entries.remove(&(binding.0, binding.2))
                && let Some(editor) = &mut entry.editor
            {
                editor.close();
            }
            self.active.retain(|&(track, _), generation| (track, *generation) != (binding.0, binding.2));
        }
    }

    pub fn binding(&self, track: TrackId, slot: usize) -> Option<SlotBinding> {
        self.active.get(&(track, slot)).map(|generation| (track, slot, *generation))
    }
    fn entry(&self, track: TrackId, slot: usize) -> Option<&Entry> {
        let binding = self.binding(track, slot)?;
        self.entries.get(&(binding.0, binding.2))
    }
    fn entry_mut(&mut self, track: TrackId, slot: usize) -> Option<&mut Entry> {
        let binding = self.binding(track, slot)?;
        self.entries.get_mut(&(binding.0, binding.2))
    }
    pub fn has_editor(&self, track: TrackId, slot: usize) -> bool {
        self.entry(track, slot).is_some_and(|entry| entry.editor.is_some())
    }
    /// Whether the live plug-in in this slot has a sidechain input.
    pub fn has_key_input(&self, track: TrackId, slot: usize) -> bool {
        self.entry(track, slot).is_some_and(|entry| entry.key_input)
    }
    pub fn open_editor(&mut self, track: TrackId, slot: usize) -> Result<(), String> {
        let entry = self.entry_mut(track, slot).ok_or_else(|| "this plugin instance is not active".to_owned())?;
        let editor = entry.editor.as_mut().ok_or_else(|| "this plugin has no editor".to_owned())?;
        editor.open()?;
        entry.refused = false;
        Ok(())
    }
    /// How long this slot's editor open has been on its way, while it is (issue #67).
    pub fn editor_opening(&self, track: TrackId, slot: usize) -> Option<Duration> {
        self.entry(track, slot).and_then(|entry| entry.editor.as_ref()).and_then(|editor| editor.opening_for())
    }

    /// The editor opens that finished since the last call, each once.
    pub fn take_editor_opened(&mut self) -> Vec<EditorOpened> {
        std::mem::take(&mut self.editor_opened)
    }

    pub fn close_editor(&mut self, track: TrackId, slot: usize) {
        if let Some(entry) = self.entry_mut(track, slot)
            && let Some(editor) = &mut entry.editor
        {
            editor.close();
        }
    }
    pub fn close_editors(&mut self) {
        for entry in self.entries.values_mut() {
            if let Some(editor) = &mut entry.editor {
                editor.close();
            }
        }
    }
    pub fn editor_open(&self, track: TrackId, slot: usize) -> bool {
        self.entry(track, slot).and_then(|entry| entry.editor.as_ref()).is_some_and(|editor| editor.is_open())
    }
    pub fn any_editor_open(&self) -> bool {
        self.active.keys().any(|&(track, slot)| self.editor_open(track, slot))
    }
    pub fn set_param(&mut self, track: TrackId, slot: usize, id: &str, value: f32) {
        if let Some(entry) = self.entry_mut(track, slot)
            && let Some(editor) = &mut entry.editor
        {
            editor.set_param(id, value);
        }
    }
    /// Reads every live plug-in's state for a save (issue #56). A plug-in that is answering is
    /// waited for, however slowly. One whose thread is wedged is left out at once; one whose
    /// thread is held by a modal dialog is waited for at most what is left of the save deadline,
    /// shared by all of them, unless the session holds no state for it yet (then it is waited for
    /// like the others: it would otherwise be saved with none). A plug-in left out keeps its
    /// stored state and is named by [`Self::take_unrefreshed`].
    pub fn capture_states(&mut self) -> PluginStates {
        let deadline = Instant::now() + self.save_deadline;
        let mut unrefreshed = Vec::new();
        let states = self
            .active
            .iter()
            .filter_map(|(&(track, slot), &generation)| {
                let entry = self.entries.get_mut(&(track, generation))?;
                if entry.refused {
                    return None;
                }
                let busy = if entry.stored { deadline.saturating_duration_since(Instant::now()) } else { Duration::MAX };
                match entry.state.as_mut()?.save_state_within(busy) {
                    Ok(state) => {
                        let state = state?;
                        entry.stored = true;
                        Some((track, slot, state))
                    }
                    Err(StateBusy) => {
                        log::warn!("{}: state not read within the save deadline; its stored state is kept", entry.name);
                        unrefreshed.push(entry.name.to_owned());
                        None
                    }
                }
            })
            .collect();
        self.unrefreshed = unrefreshed;
        states
    }

    /// The plug-ins the last [`Self::capture_states`] could not read in time, once.
    pub fn take_unrefreshed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unrefreshed)
    }

    /// How long one save waits, in all, for plug-ins held by a modal dialog
    /// ([`DEFAULT_SAVE_DEADLINE`]).
    pub fn set_save_deadline(&mut self, deadline: Duration) {
        self.save_deadline = deadline;
    }
    /// What the header strip of this slot's editor window shows (issue #68); sent on to the
    /// editor's host thread when it changed.
    pub fn set_header(&mut self, track: TrackId, slot: usize, header: &EditorHeader) {
        if let Some(entry) = self.entry_mut(track, slot)
            && let Some(editor) = &mut entry.editor
        {
            editor.set_header(header);
        }
    }

    /// What the user did in the editor windows' header strips: (track, slot, action), in order.
    pub fn take_header_actions(&mut self) -> Vec<(TrackId, usize, HeaderAction)> {
        let mut out = Vec::new();
        for (&(track, slot), &generation) in &self.active {
            if let Some(entry) = self.entries.get_mut(&(track, generation))
                && let Some(editor) = &mut entry.editor
            {
                out.extend(editor.take_header_actions().into_iter().map(|a| (track, slot, a)));
            }
        }
        out
    }

    /// The live plug-in's own name in this slot (`None`: no live instance).
    pub fn live_name(&self, track: TrackId, slot: usize) -> Option<&'static str> {
        self.entry(track, slot).map(|entry| entry.name)
    }

    /// `Err(reason)` while this slot's editor cannot be driven (its plug-in host thread is not
    /// responding, or there is no live instance with an editor).
    pub fn editor_available(&self, track: TrackId, slot: usize) -> Result<(), String> {
        let entry = self.entry(track, slot).ok_or_else(|| "this plugin instance is not active".to_owned())?;
        entry.editor.as_ref().ok_or_else(|| "this plugin has no editor".to_owned())?.available()
    }

    /// Starts reading this slot's live state for a preset save or a copy (issue #68): on the
    /// plug-in host thread, polled once per frame.
    pub fn begin_save_state(&mut self, track: TrackId, slot: usize) -> Result<StateJob<Option<Vec<u8>>>, String> {
        let entry = self.entry_mut(track, slot).ok_or_else(|| "this plugin instance is not active".to_owned())?;
        Ok(entry.state.as_mut().ok_or_else(|| "this plugin has no state of its own".to_owned())?.begin_save())
    }

    /// Starts loading a state into this slot's live instance (a preset, issue #68): on the plug-in
    /// host thread, polled once per frame. The job's values are the plug-in's after the load.
    pub fn begin_load_state(&mut self, track: TrackId, slot: usize, data: Vec<u8>) -> Result<StateJob<Option<LoadedValues>>, String> {
        let entry = self.entry_mut(track, slot).ok_or_else(|| "this plugin instance is not active".to_owned())?;
        let job = entry.state.as_mut().ok_or_else(|| "this plugin has no state of its own".to_owned())?.begin_load(data);
        // A state the instance takes now is one it holds: a capture reads it again.
        entry.refused = false;
        Ok(job)
    }

    /// The default state of plug-in `id` (Reset to Default, issue #68): a fresh instance made on
    /// the plug-in host thread, its state read and the instance destroyed there.
    pub fn begin_default_state(&self, id: &str, ch: usize) -> StateJob<Option<Vec<u8>>> {
        let id = id.to_owned();
        let pending = self
            .host
            .submit(move |_ctx| transit_mix::create_instance(&id, None, 48_000.0, 512, ch.max(1)).and_then(|mut made| made.plugin.save_state()));
        StateJob::remote(move || match pending.poll() {
            Status::Ready(v) => JobPoll::Ready(v),
            Status::Waiting { .. } => JobPoll::Waiting,
            Status::Stuck => JobPoll::Failed("the plug-in did not load in time".into()),
            Status::Failed => JobPoll::Failed("the plug-in host could not make the plug-in".into()),
        })
    }

    pub fn idle(&mut self) -> Vec<EditorEdit> {
        let mut out = Vec::new();
        for (&(track, slot), &generation) in &self.active {
            if let Some(entry) = self.entries.get_mut(&(track, generation))
                && let Some(editor) = &mut entry.editor
            {
                out.extend(editor.idle().into_iter().map(|(id, value)| (track, slot, id, value)));
                if let Some(e) = editor.take_error() {
                    self.editor_errors.push((track, slot, e));
                }
                if let Some(report) = editor.take_open_report() {
                    log::info!("{}", transit_plugin_host::open_log_line(entry.name, report));
                    self.editor_opened.push(EditorOpened { track, slot, name: entry.name, report });
                }
            }
        }
        out
    }
}

impl Drop for Handles {
    fn drop(&mut self) {
        self.close_editors();
    }
}
