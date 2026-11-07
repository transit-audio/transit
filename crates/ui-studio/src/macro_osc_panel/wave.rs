//! The hero well's picture of the model (#326, job 326-W): one cycle of the current model (a
//! 50 ms burst for the noise and percussion group) rendered from the parameters by
//! `transit_dsp::plugins::macro_osc::preview`, never a live scope of the summed output.
//!
//! - **The request:** the newest voice's published note, Harmonics, Timbre and Morph while a key
//!   holds a voice, else the knobs at note 60 (C3); the model, Decay, color and seed from the
//!   instance ([`request`]).
//! - **The worker:** one thread per app, owned by the editor state and started on first use. The
//!   panel sends a request only when it differs from the last one it sent for that instance; the
//!   worker renders the newest request per instance (an older one still queued is dropped), stores
//!   the result and asks for a repaint. The panel never waits: it draws the last result it has.
//!   The thread ends when the app drops the editor state (its channel closes). In unit tests the
//!   render runs inline at the request, so a test reads the result in the same frame.

use egui::{Pos2, Rect, pos2};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use transit_dsp::VoiceShape;
use transit_dsp::plugins::macro_osc::preview::{self, INSTANCE_SEED, Kind, Request};

/// The note the well renders at while no key holds a voice: C3 (middle C).
pub const REST_NOTE: f32 = 60.0;

/// A finished render: the request, what it holds, the samples.
pub type Rendered = (Request, Kind, Vec<f32>);

/// The knobs the request reads (0..1) and the model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    pub engine: usize,
    pub harmonics: f32,
    pub timbre: f32,
    pub morph: f32,
    pub decay: f32,
    pub lpg_color: f32,
}

/// The request for the well: the newest voice's note and values while one sounds, else the knobs
/// at [`REST_NOTE`].
#[must_use]
pub fn request(k: Knobs, newest: Option<VoiceShape>) -> Request {
    let (note, harmonics, timbre, morph) = match newest {
        Some(v) => (f32::from(v.note), v.harmonics, v.timbre, v.morph),
        None => (REST_NOTE, k.harmonics, k.timbre, k.morph),
    };
    Request { engine: k.engine, note, harmonics, timbre, morph, decay: k.decay, lpg_color: k.lpg_color, seed: INSTANCE_SEED }
}

/// The trace of `samples` in `plot`: stretched across the full width, centered on the midline,
/// scaled so the peak reaches half the height (a silent render lies on the midline).
#[must_use]
pub fn trace(plot: Rect, samples: &[f32]) -> Vec<Pos2> {
    let peak = samples.iter().filter(|x| x.is_finite()).fold(0.0f32, |m, x| m.max(x.abs()));
    let scale = if peak > 0.0 { plot.height() / 2.0 / peak } else { 0.0 };
    let mid = plot.center().y;
    let last = samples.len().saturating_sub(1).max(1) as f32;
    samples
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let y = if x.is_finite() { (mid - x * scale).clamp(plot.min.y, plot.max.y) } else { mid };
            pos2(plot.min.x + plot.width() * i as f32 / last, y)
        })
        .collect()
}

/// The well's renders: the worker, what was last sent and the last result per instance (by uid).
#[derive(Default)]
pub struct Preview {
    tx: Option<Sender<(u64, Request)>>,
    done: Arc<Mutex<HashMap<u64, Rendered>>>,
    sent: HashMap<u64, Request>,
    shown: HashMap<u64, Rendered>,
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Preview").field("running", &self.tx.is_some()).field("sent", &self.sent.len()).finish_non_exhaustive()
    }
}

impl Preview {
    /// Sends `req` for instance `uid` when it differs from the last one sent, takes in any result
    /// the worker finished, and returns the last result for `uid` (`None` before the first).
    pub fn show(&mut self, ctx: &egui::Context, uid: u64, req: Request) -> Option<&Rendered> {
        if self.sent.get(&uid) != Some(&req) {
            self.sent.insert(uid, req);
            self.send(ctx, uid, req);
        }
        if let Ok(mut done) = self.done.lock() {
            self.shown.extend(done.drain());
        }
        self.shown.get(&uid)
    }

    /// The request last sent for `uid`.
    #[must_use]
    pub fn sent(&self, uid: u64) -> Option<Request> {
        self.sent.get(&uid).copied()
    }

    #[cfg(test)]
    fn send(&mut self, _ctx: &egui::Context, uid: u64, req: Request) {
        // Unit tests render inline: the result is there in the same frame, no thread, no waiting.
        if let Ok(mut done) = self.done.lock() {
            done.insert(uid, render(req));
        }
    }

    #[cfg(not(test))]
    fn send(&mut self, ctx: &egui::Context, uid: u64, req: Request) {
        if let Some(tx) = &self.tx
            && tx.send((uid, req)).is_ok()
        {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let done = Arc::clone(&self.done);
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("macro-osc-preview".into()).spawn(move || work(&rx, &done, &ctx));
        if spawned.is_ok() && tx.send((uid, req)).is_ok() {
            self.tx = Some(tx);
        }
    }
}

/// Renders one request.
#[must_use]
pub fn render(req: Request) -> Rendered {
    let mut out = Vec::new();
    let kind = preview::render(&req, &mut out);
    (req, kind, out)
}

/// The newest request per instance among `queued` and whatever else waits in `rx`.
pub fn latest(queued: &mut HashMap<u64, Request>, rx: &Receiver<(u64, Request)>) {
    for (uid, req) in rx.try_iter() {
        queued.insert(uid, req);
    }
}

/// The worker: waits for a request, renders the newest per instance, stores and repaints (a result
/// a newer request overtook is dropped); ends when the channel closes.
#[cfg_attr(test, allow(dead_code))]
fn work(rx: &Receiver<(u64, Request)>, done: &Mutex<HashMap<u64, Rendered>>, ctx: &egui::Context) {
    let mut queued = HashMap::new();
    while let Ok((uid, req)) = rx.recv() {
        queued.insert(uid, req);
        latest(&mut queued, rx);
        while let Some(uid) = queued.keys().next().copied() {
            let Some(req) = queued.remove(&uid) else { break };
            let r = render(req);
            // A newer request for the instance that came in during the render drops this result:
            // the panel keeps the previous one until the newer render lands.
            latest(&mut queued, rx);
            if queued.contains_key(&uid) {
                continue;
            }
            if let Ok(mut d) = done.lock() {
                d.insert(uid, r);
            }
            ctx.request_repaint();
        }
    }
}
