//! The Compressor's floating window: the shared device window ([`crate::device_editor::window`])
//! with this panel's widths (the board's 960, the content's minimum), body, footer and record.

use super::{CompressorLayout, Dev, FOOTER_H, MIN_WIDTH};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::device_editor::title_bar::TitleLayout;
use crate::device_editor::window as shared;
use crate::device_window::{self, Widths};
use crate::tokens::layout as space;
use egui::{Rect, Vec2};
use transit_model::TrackId;

/// The panel's widths for the shared policy.
pub const WIDTHS: Widths = Widths { design: super::DESIGN_WIDTH, min: MIN_WIDTH };

/// The minimum width the window and its content keep.
pub fn min_width(app: &StudioApp) -> f32 {
    shared::min_width(app, MIN_WIDTH)
}

/// The window's size on `screen` for a body `content_h` tall.
pub fn window_size(screen: Rect, content_h: f32) -> Vec2 {
    device_window::size(WIDTHS, screen, space::TITLE_BAR_HEIGHT, content_h, FOOTER_H)
}

/// The Compressor in the shared window.
pub(crate) struct Panel;

impl shared::Panel for Panel {
    type Record = CompressorLayout;

    fn widths(app: &StudioApp) -> Widths {
        Widths { min: min_width(app), ..WIDTHS }
    }

    fn footer_h() -> f32 {
        FOOTER_H
    }

    fn content_height(app: &StudioApp, ctx: &egui::Context, key: (TrackId, usize), width: f32) -> f32 {
        super::content_height(app, ctx, key, width)
    }

    fn body(app: &mut StudioApp, ui: &mut egui::Ui, dev: Dev, lay: &mut EditorLayout, rec: &mut CompressorLayout) {
        super::body(app, ui, dev, lay, rec);
    }

    fn footer(app: &StudioApp, ui: &mut egui::Ui, rect: Rect, lay: &mut EditorLayout, rec: &mut CompressorLayout) {
        super::footer(app, ui, rect, lay, rec);
    }

    fn title(rec: &mut CompressorLayout) -> &mut TitleLayout {
        &mut rec.title
    }

    fn set_window(rec: &mut CompressorLayout, rect: [f32; 4]) {
        rec.window = rect;
    }

    fn typing(rec: &CompressorLayout, uid: u64) -> Option<egui::Id> {
        rec.knobs.iter().find(|k| k.knob.editing).map(|k| super::knob_id(uid, &k.param))
    }

    fn store(layout: &mut EditorLayout, rec: CompressorLayout) {
        layout.compressor = Some(Box::new(rec));
    }
}
