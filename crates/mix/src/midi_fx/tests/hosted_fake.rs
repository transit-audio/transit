//! An in-process fake of a hosted note plug-in for the hosted stage's tests (ADR 0064 HM2): it
//! implements the five note-output methods of the `Plugin` trait and is scripted like the fixture
//! plug-ins. Like the adapters it splits a block into native calls of at most its prepared block,
//! makes none for a block of no frames, skips a block (keeping its input for offset 0 of the next
//! actual call) while it is held or inactive, runs a requested reset at the start of the next
//! actual call (forgetting its scheduled releases unless it retains them), and reports every call
//! with its generation in `take_note_output`. Its `restart_processing` drops queued input that is
//! not a release, as the VST3 adapter's does (ADR 0059, 3d).

use std::sync::{Arc, Mutex, MutexGuard};
use transit_dsp::Plugin;
use transit_dsp::event::{Message, PluginEvent};
use transit_dsp::hosted_out::{CallRecord, CallReport, HostedOut, HostedOutBuffer, Lifecycle, NoteOutputInfo, OutForm, OutKind, OutValues, calls_due};

/// What a test scripts and reads.
pub(super) struct Script {
    pub(super) max_block: usize,
    /// Frames of every block the fake was given (processed or skipped).
    pub(super) frame: u64,
    /// Native calls made.
    pub(super) calls: u64,
    /// `process` calls (blocks).
    pub(super) blocks: u64,
    /// Events written at an absolute frame, in the call that contains it.
    pub(super) at: Vec<(u64, HostedOut)>,
    /// Events written in native call `n` at local offset `o` (as written, unclamped).
    pub(super) at_call: Vec<(u64, i64, HostedOut)>,
    /// A note effect: every input note is written again, `semitones` up, with its id and velocity.
    pub(super) echo: Option<i16>,
    /// HOLD: a note-on written at the frame and its note-off scheduled `len` frames later.
    pub(super) holds: Vec<(u64, HostedOut, u64)>,
    /// Scheduled note-offs (absolute frame).
    pub(super) due: Vec<(u64, HostedOut)>,
    /// Keep the scheduled note-offs through a reset.
    pub(super) retain: bool,
    /// Input as the plug-in received it: the absolute frame (call start + offset) and the event.
    pub(super) heard: Vec<(u64, PluginEvent)>,
    /// Input for the next actual call: block offset and event.
    queue: Vec<(u32, PluginEvent)>,
    /// Input events the host handed to it (`event` calls).
    pub(super) offered: u64,
    /// Absolute frames at which a reset ran (before a call's processing).
    pub(super) resets: Vec<u64>,
    reset_pending: bool,
    /// `restart_processing` calls.
    pub(super) restarts: u64,
    pub(super) note_output: Option<NoteOutputInfo>,
    pub(super) generation: u64,
    pub(super) active: bool,
    /// The main thread holds the instance: every call is skipped.
    pub(super) held: bool,
    /// That native call reports a processing error.
    pub(super) fail_call: Option<u64>,
    /// A generation change applied inside the next `process`, before its calls (the main thread
    /// loaded or restarted the instance after the stage's check): new active flag.
    pub(super) change_in_process: Option<bool>,
    /// The next `process` runs its calls under the generation before the current one (an old
    /// epoch's call after its barrier).
    pub(super) regress_in_process: bool,
    /// A requested reset never runs (a plug-in host that lost the request): no call follows one.
    pub(super) no_reset: bool,
    pub(super) collect: bool,
    buf: HostedOutBuffer,
}

impl Script {
    fn new(max_block: usize) -> Self {
        Script {
            max_block,
            frame: 0,
            calls: 0,
            blocks: 0,
            at: Vec::new(),
            at_call: Vec::new(),
            echo: None,
            holds: Vec::new(),
            due: Vec::new(),
            retain: false,
            heard: Vec::new(),
            queue: Vec::new(),
            offered: 0,
            resets: Vec::new(),
            reset_pending: false,
            restarts: 0,
            note_output: Some(NoteOutputInfo { read: 0, declared: 1 }),
            generation: 1,
            active: true,
            held: false,
            fail_call: None,
            change_in_process: None,
            regress_in_process: false,
            no_reset: false,
            collect: false,
            buf: HostedOutBuffer::empty(),
        }
    }

    /// A restart or a state load seen by the instance's lifecycle (a new generation).
    pub(super) fn bump(&mut self, active: bool) {
        self.generation += 1;
        self.active = active;
    }
}

/// A handle on the fake's script (the instance itself goes into the plan cell).
#[derive(Clone)]
pub(super) struct Handle(Arc<Mutex<Script>>);

impl Handle {
    pub(super) fn lock(&self) -> MutexGuard<'_, Script> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(super) struct Fake {
    s: Arc<Mutex<Script>>,
}

impl Fake {
    /// A fake prepared for native calls of at most `max_block` frames, collecting its output.
    pub(super) fn new(max_block: usize) -> (Fake, Handle) {
        let s = Arc::new(Mutex::new(Script::new(max_block)));
        let mut f = Fake { s: Arc::clone(&s) };
        f.collect_note_output(true);
        (f, Handle(s))
    }

    fn st(&self) -> MutexGuard<'_, Script> {
        self.s.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A native note event as an adapter hands it out: `kind` on port 0, `channel`, `key`, `id`, value.
pub(super) fn out(kind: OutKind, channel: i16, key: i16, id: i32, value: f64) -> HostedOut {
    let values = if kind == OutKind::PolyPressure { OutValues::Pressure(value) } else { OutValues::Velocity(value) };
    HostedOut { offset: 0, seq: 0, kind, port: 0, channel, key, note_id: id, form: OutForm::Native, values }
}

/// A MIDI-byte event (a CLAP MIDI event).
pub(super) fn midi(kind: OutKind, bytes: [u8; 3]) -> HostedOut {
    let key = if matches!(kind, OutKind::NoteOn | OutKind::NoteOff | OutKind::PolyPressure) { i16::from(bytes[1]) } else { -1 };
    HostedOut { offset: 0, seq: 0, kind, port: 0, channel: i16::from(bytes[0] & 0x0F), key, note_id: -1, form: OutForm::MidiBytes, values: OutValues::Midi(bytes) }
}

fn echo_of(ev: &PluginEvent, shift: i16) -> Option<HostedOut> {
    let (kind, key, velocity) = match ev.message {
        Message::NoteOn { key, velocity, .. } => (OutKind::NoteOn, key, velocity),
        Message::NoteOff { key, velocity, .. } => (OutKind::NoteOff, key, velocity),
        _ => return None,
    };
    Some(out(kind, i16::from(ev.channel), i16::from(key) + shift, ev.note_id, f64::from(velocity) / 127.0))
}

impl Plugin for Fake {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, max_block: usize, _: usize) {
        let mut s = self.st();
        s.max_block = max_block.max(1);
        s.bump(true);
    }
    fn reset(&mut self) {
        self.st().reset_pending = true;
    }
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        let mut s = self.st();
        s.offered += 1;
        s.queue.push((offset, *ev));
        true
    }
    fn process(&mut self, _io: &mut [Vec<f32>], frames: usize) {
        let s = &mut *self.st();
        s.blocks += 1;
        if let Some(active) = s.change_in_process.take() {
            s.bump(active);
        }
        if std::mem::take(&mut s.regress_in_process) {
            s.generation -= 1;
        }
        s.buf.begin_block();
        let due = calls_due(frames, s.max_block);
        if s.held || !s.active {
            s.buf.skipped(due);
            // Kept for offset 0 of the next actual call (ADR 0059, 3a).
            for q in &mut s.queue {
                q.0 = 0;
            }
            s.frame += frames as u64;
            return;
        }
        let max = s.max_block;
        for k in 0..due as usize {
            let start = k * max;
            let n = max.min(frames - start);
            let (from, to) = (s.frame + start as u64, s.frame + (start + n) as u64);
            let reset_before = !s.no_reset && std::mem::take(&mut s.reset_pending);
            if reset_before {
                s.resets.push(from);
                if !s.retain {
                    s.due.clear();
                }
            }
            let (start32, n32) = (start as u32, n as u32);
            let mut i = 0;
            while i < s.queue.len() {
                let (o, ev) = s.queue[i];
                if o < start32 + n32 {
                    s.queue.remove(i);
                    let local = o.saturating_sub(start32);
                    s.heard.push((from + u64::from(local), ev));
                    if let Some(e) = s.echo.and_then(|sh| echo_of(&ev, sh)) {
                        s.buf.push(i64::from(local), start32, n32, e);
                    }
                } else {
                    i += 1;
                }
            }
            let call = s.calls;
            let at: Vec<_> = s.at.iter().filter(|(f, _)| *f >= from && *f < to).copied().collect();
            for (f, e) in at {
                s.buf.push((f - from) as i64, start32, n32, e);
            }
            let here: Vec<_> = s.at_call.iter().filter(|(c, _, _)| *c == call).copied().collect();
            for (_, o, e) in here {
                s.buf.push(o, start32, n32, e);
            }
            let holds: Vec<_> = s.holds.iter().filter(|(f, _, _)| *f >= from && *f < to).copied().collect();
            for (f, e, len) in holds {
                s.buf.push((f - from) as i64, start32, n32, e);
                s.due.push((f + len, HostedOut { kind: OutKind::NoteOff, values: OutValues::Velocity(0.0), ..e }));
            }
            let offs: Vec<_> = s.due.iter().filter(|(f, _)| *f >= from && *f < to).copied().collect();
            s.due.retain(|(f, _)| !(*f >= from && *f < to));
            for (f, e) in offs {
                s.buf.push((f - from) as i64, start32, n32, e);
            }
            s.calls += 1;
            let error = s.fail_call == Some(call);
            s.buf.record_call(CallRecord { generation: s.generation, reset_before, error });
            if error {
                s.buf.skipped(due - 1 - k as u32);
                break;
            }
        }
        // Input past the block's frames (none from the host) would wait; offsets are block offsets.
        s.frame += frames as u64;
    }
    fn note_output(&self) -> Option<NoteOutputInfo> {
        self.st().note_output
    }
    fn take_note_output(&mut self, out: &mut dyn FnMut(HostedOut) -> bool) -> CallReport {
        self.st().buf.take(out)
    }
    fn lifecycle(&self) -> Lifecycle {
        let s = self.st();
        Lifecycle { generation: s.generation, active: s.active }
    }
    fn restart_processing(&mut self) {
        let mut s = self.st();
        s.restarts += 1;
        // Today's VST3 reset: queued releases stay, everything else queued goes.
        s.queue.retain(|(_, e)| e.is_release());
        s.reset_pending = true;
    }
    fn collect_note_output(&mut self, on: bool) {
        let mut s = self.st();
        s.collect = on;
        if on {
            s.buf.reserve();
        }
    }
}
