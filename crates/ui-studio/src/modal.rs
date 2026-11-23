//! The one fact every input path asks (#770): is a modal dialog open, or opening, this frame?
//!
//! egui's own answer, `Memory::top_modal_layer`, is the modal layer registered in the previous
//! frame, so on the frame a dialog opens it is still empty. The app adds what it knows sooner: a
//! dialog it was asked for since the last frame (`StudioApp::modal_opening`, marked here before the
//! frame's handlers run and again whenever a handler asks for one) and every modal drawn earlier in
//! this frame (each marks itself as it is shown). The frame on which a dialog opens also takes no
//! key, text, click or wheel at all (`StudioApp::raw_input_hook`).
//!
//! Keys belong to the top modal only: [`escape`] and [`enter`] answer for the modal on `layer`
//! only when it is the top one, and they consume the key, so one press never reaches a second
//! dialog under it or anything behind.

use egui::{Context, Id, LayerId, Modifiers};

fn flag() -> Id {
    Id::new("transit-modal-up")
}

/// Whether a modal holds the app this frame: one was registered last frame, one was drawn earlier
/// in this frame, or the app asked for one since the last frame.
#[must_use]
pub fn up(ctx: &Context) -> bool {
    let frame = ctx.cumulative_frame_nr();
    ctx.data(|d| d.get_temp::<u64>(flag())) == Some(frame) || ctx.memory(|m| m.top_modal_layer().is_some())
}

/// A modal is open or opening in this frame (held until the frame ends).
pub fn mark(ctx: &Context) {
    let _ = typed_last_frame(ctx);
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(flag(), frame));
}

/// The modals drawn in the current frame and the one before it (each registers as it is shown).
#[derive(Clone, Default)]
struct Drawn {
    frame: u64,
    now: Vec<LayerId>,
    last: Vec<LayerId>,
}

fn drawn_id() -> Id {
    Id::new("transit-modal-drawn")
}

/// Registers the modal on `layer` as drawn this frame (the window wrapper and `chrome::raw_modal`
/// call it).
pub fn register(ctx: &Context, layer: LayerId) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| {
        let r = d.get_temp_mut_or_default::<Drawn>(drawn_id());
        if r.frame != frame {
            r.last = if r.frame + 1 == frame { std::mem::take(&mut r.now) } else { Vec::new() };
            r.now.clear();
            r.frame = frame;
        }
        if !r.now.contains(&layer) {
            r.now.push(layer);
        }
    });
}

/// The modal that owns the keys this frame: of the modals drawn last frame, the one highest in the
/// layer order as it stands now. egui's own modal layer was settled against the order of the frame
/// before, so for one frame after a dialog is raised over another (the plug-in-not-responding
/// message coming back over About) it still names the covered one; the order already has the
/// raised one on top (#770 round 3).
#[must_use]
pub fn owner(ctx: &Context) -> Option<LayerId> {
    let frame = ctx.cumulative_frame_nr();
    let r = ctx.data(|d| d.get_temp::<Drawn>(drawn_id())).unwrap_or_default();
    let last = if r.frame == frame {
        r.last
    } else if r.frame + 1 == frame {
        r.now
    } else {
        Vec::new()
    };
    ctx.memory(|m| {
        let order: Vec<LayerId> = m.layer_ids().collect();
        last.into_iter().filter_map(|l| order.iter().position(|o| *o == l).map(|at| (at, l))).max_by_key(|(at, _)| *at).map(|(_, l)| l)
    })
}

/// Whether the modal on `layer` owns the keys this frame ([`owner`]).
#[must_use]
pub fn is_top(ctx: &Context, layer: LayerId) -> bool {
    owner(ctx) == Some(layer)
}

/// Whether the dialog's own keys are held back this frame: a text field (or a drag value's text
/// editor) has the keyboard, or had it when the last frame ended, or a list is open; those take
/// Enter and Escape themselves. egui lets go of a field's focus on Escape before the frame's
/// widgets run (`Focus::begin_pass`), so the field as the last frame left it is what counts.
#[must_use]
pub fn held(ctx: &Context) -> bool {
    ctx.text_edit_focused() || egui::Popup::is_any_open(ctx) || typed_last_frame(ctx)
}

/// Remembers, at the end of each pass, whether a text field had the keyboard ([`held`]).
struct Typing {
    /// The frame and whether a text field had the keyboard when its last pass ended.
    last: (u64, bool),
}

impl egui::Plugin for Typing {
    fn debug_name(&self) -> &'static str {
        "modal_typing"
    }

    fn on_end_pass(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx();
        self.last = (ctx.cumulative_frame_nr(), ctx.text_edit_focused());
    }
}

fn typed_last_frame(ctx: &Context) -> bool {
    let frame = ctx.cumulative_frame_nr();
    match ctx.with_plugin::<Typing, _>(|t| t.last) {
        Some((at, typed)) => typed && at + 1 == frame,
        None => {
            ctx.add_plugin(Typing { last: (frame, false) });
            false
        }
    }
}

/// Whether `action`'s key (a dialog scope row of the shortcut table) answers the modal on `layer`
/// this frame: only the top modal, only when no field or list holds the key, and the key is
/// consumed.
#[must_use]
pub fn answers(ctx: &Context, layer: LayerId, action: crate::shortcuts::Action) -> bool {
    // A later layout pass of the same frame replays its input: the first pass decided.
    if ctx.current_pass_index() > 0 || !is_top(ctx, layer) || held(ctx) {
        return false;
    }
    ctx.input_mut(|i| crate::shortcuts::rows(action).fold(false, |hit, s| hit | i.consume_key(Modifiers::NONE, s.key)))
}

/// Escape for the modal on `layer` (`Action::DialogCancel`): true (and the key consumed) only when
/// it is the top modal and no field or list holds the key.
#[must_use]
pub fn escape(ctx: &Context, layer: LayerId) -> bool {
    answers(ctx, layer, crate::shortcuts::Action::DialogCancel)
}

/// Enter for the modal on `layer` (`Action::DialogConfirm`), as [`escape`].
#[must_use]
pub fn enter(ctx: &Context, layer: LayerId) -> bool {
    answers(ctx, layer, crate::shortcuts::Action::DialogConfirm)
}

/// The layer of an `egui::Modal` made with `id` (the foreground order).
#[must_use]
pub fn layer_of(id: Id) -> LayerId {
    LayerId::new(egui::Order::Foreground, id)
}

/// Whether `event` is input a modal's opening frame drops: a key or button press, text, a paste,
/// copy or cut, a wheel or a zoom. Releases and pointer motion pass, so no press is left hanging.
#[must_use]
pub fn opening_drops(event: &egui::Event) -> bool {
    use egui::Event as E;
    matches!(
        event,
        E::Key { pressed: true, .. }
            | E::Text(_)
            | E::Paste(_)
            | E::Copy
            | E::Cut
            | E::PointerButton { pressed: true, .. }
            | E::MouseWheel { .. }
            | E::Zoom(_)
            | E::Ime(_)
    )
}

impl crate::StudioApp {
    /// The modal dialogs asked for now, one bit each (the app's dialog slot by its kind, so a new
    /// question in the slot counts as a new dialog). Every dialog the app draws has its flag here.
    #[must_use]
    pub(crate) fn modal_mask(&self) -> u64 {
        use crate::Dialog as D;
        let mut m = match &self.dialog {
            None => 0,
            Some(D::SaveChanges { .. }) => 1,
            Some(D::DeleteTracks { .. }) => 1 << 1,
            Some(D::Bounce) => 1 << 2,
            Some(D::DawprojectExport) => 1 << 3,
            Some(D::BounceInPlace) => 1 << 4,
            Some(D::HiddenTracks) => 1 << 5,
            Some(D::DeleteTake { .. }) => 1 << 6,
        };
        let mut bit = |on: bool, n: u32| {
            if on {
                m |= 1 << n;
            }
        };
        bit(self.about.open, 8);
        bit(self.keys.quantize_open, 9);
        bit(self.mixer.hw_picker.is_some(), 10);
        bit(self.ui.show_shortcuts, 11);
        bit(!self.leftovers.is_empty(), 12);
        bit(crate::host_hang::shown(self), 13);
        let views = self.builtin_editor.devices.sampler.views.values();
        let (mut kit, mut load, mut zone, mut replace) = (false, false, false, false);
        for v in views {
            kit |= v.kit.confirm.is_some();
            replace |= v.kit.pending.is_some();
            load |= v.kit_preview.is_some();
            zone |= v.instrument.confirm.is_some();
        }
        bit(kit, 14);
        bit(load, 15);
        bit(zone, 16);
        bit(replace, 17);
        // A new dialog: give it a bit here and a row in `modal_hold_tests::every_dialog_flag_is_in_the_mask`
        // (its call-site census fails until both are done).
        m
    }

    /// Whether a modal is opening this frame: asked for since the frame began, or between the last
    /// frame's start and this one's (the frame whose input [`Self::modal_input`] dropped).
    #[must_use]
    pub(crate) fn modal_opening(&self) -> bool {
        self.modal_entering || self.modal_mask() & !self.modal_seen != 0
    }

    /// Marks the frame modal when a dialog is opening (call before input handlers, and after any
    /// that can ask for one).
    pub(crate) fn sync_modal(&self, ctx: &Context) {
        let _ = typed_last_frame(ctx); // installs the field memory `held` reads
        if self.modal_opening() {
            mark(ctx);
        }
    }

    /// At the start of a frame, before egui sees its input: when a dialog was asked for since the
    /// last frame began, the frame drops every press, text, wheel and zoom ([`opening_drops`]), so
    /// nothing behind the dialog acts on input from the frame it opens in. Returns whether it did
    /// (synthetic input then waits a frame).
    pub(crate) fn modal_input(&mut self, raw: &mut egui::RawInput) -> bool {
        let mask = self.modal_mask();
        self.modal_entering = mask & !self.modal_seen != 0;
        self.modal_seen = mask;
        if self.modal_entering {
            raw.events.retain(|e| !opening_drops(e));
        }
        self.modal_entering
    }
}

/// Whether input reaches `ui`'s layer this frame: no modal holds the app, or the layer is the top
/// modal's or above it (its own fields and lists). Field-scoped key reads (a value being typed,
/// a rename) ask it, so a dialog's Enter or Escape never also answers a field behind it.
#[must_use]
pub fn reaches(ui: &egui::Ui) -> bool {
    let ctx = ui.ctx();
    if !up(ctx) {
        return true;
    }
    let Some(top) = owner(ctx) else { return false };
    let layer = ui.layer_id();
    ctx.memory(|m| {
        let order: Vec<LayerId> = m.layer_ids().collect();
        match (order.iter().position(|o| *o == layer), order.iter().position(|o| *o == top)) {
            (Some(at), Some(owner)) => at >= owner,
            // A layer new this frame (a list just opened): egui's own answer.
            _ => m.top_modal_layer().is_some() && m.is_above_modal_layer(layer),
        }
    })
}
