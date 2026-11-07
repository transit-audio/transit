//! Internal window title controls from the icon set (#227).
//!
//! egui draws a window's collapse triangle and close X itself, as a polygon and two line
//! segments, and offers no hook to change them. [`Window`] wraps `egui::Window`: after the window
//! is shown it finds those shapes in the window's paint list, removes them, and paints Lucide
//! `chevron-down` / `chevron-right` / `x` where they were, in the color egui chose (so hover
//! still reads). The hit areas are egui's own and stay as #206 set them.

use crate::icons::{self, Icon};
use crate::theme;
use egui::epaint::Shape;
use egui::{Align2, Color32, Context, Id, InnerResponse, LayerId, Pos2, Rect, Vec2, Vec2b};

/// The box of a title-bar icon, points. Lucide inks half of its 24-unit box, so a 20 pt box
/// inks 10 pt: the cap height of the 14 pt window title (Inter, 0.727 em = 10.2 pt). The box is
/// wider than the 18 pt close hit area; the overhang is empty padding, and 20 fits the 26 px bar.
pub const WINDOW_ICON_SIZE: f32 = 20.0;

/// An `egui::Window` whose collapse and close controls are icons. Build it with
/// [`theme::window`]; it has the builder methods transit uses.
pub struct Window<'a> {
    inner: egui::Window<'a>,
    collapsible: bool,
    closable: bool,
    /// The body's button padding when it differs from the theme's (device windows, #326).
    button_padding: Option<Vec2>,
}

impl<'a> Window<'a> {
    pub(crate) fn new(inner: egui::Window<'a>) -> Self {
        Window { inner, collapsible: true, closable: false, button_padding: None }
    }

    /// The body lays its buttons out with `padding` instead of the theme's.
    pub(crate) fn button_padding(mut self, padding: Vec2) -> Self {
        self.button_padding = Some(padding);
        self
    }

    pub fn id(mut self, id: Id) -> Self {
        self.inner = self.inner.id(id);
        self
    }

    pub fn open(mut self, open: &'a mut bool) -> Self {
        self.inner = self.inner.open(open);
        self.closable = true;
        self
    }

    pub fn collapsible(mut self, collapsible: bool) -> Self {
        self.inner = self.inner.collapsible(collapsible);
        self.collapsible = collapsible;
        self
    }

    pub fn resizable(mut self, resizable: impl Into<Vec2b>) -> Self {
        self.inner = self.inner.resizable(resizable);
        self
    }

    pub fn constrain(mut self, constrain: bool) -> Self {
        self.inner = self.inner.constrain(constrain);
        self
    }

    pub fn anchor(mut self, align: Align2, offset: impl Into<Vec2>) -> Self {
        self.inner = self.inner.anchor(align, offset);
        self
    }

    pub fn default_pos(mut self, pos: impl Into<Pos2>) -> Self {
        self.inner = self.inner.default_pos(pos);
        self
    }

    pub fn default_size(mut self, size: impl Into<Vec2>) -> Self {
        self.inner = self.inner.default_size(size);
        self
    }

    pub fn default_width(mut self, width: f32) -> Self {
        self.inner = self.inner.default_width(width);
        self
    }

    /// Exactly `size` (the outer size), with no resize handle (device windows, 326-X).
    pub fn fixed_size(mut self, size: impl Into<Vec2>) -> Self {
        self.inner = self.inner.fixed_size(size);
        self
    }

    pub fn min_size(mut self, size: impl Into<Vec2>) -> Self {
        self.inner = self.inner.min_size(size);
        self
    }

    pub fn max_size(mut self, size: impl Into<Vec2>) -> Self {
        self.inner = self.inner.max_size(size);
        self
    }

    /// Without egui's title bar: a device window draws its own 52 px A bar (326-M).
    pub fn title_bar(mut self, title_bar: bool) -> Self {
        self.inner = self.inner.title_bar(title_bar);
        self
    }

    pub fn movable(mut self, movable: bool) -> Self {
        self.inner = self.inner.movable(movable);
        self
    }

    pub fn current_pos(mut self, pos: impl Into<Pos2>) -> Self {
        self.inner = self.inner.current_pos(pos);
        self
    }

    /// Show the window, then swap egui's control shapes for icons.
    pub fn show<R>(self, ctx: &Context, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> Option<InnerResponse<Option<R>>> {
        let (collapsible, closable, padding) = (self.collapsible, self.closable, self.button_padding);
        let shown = self.inner.show(ctx, |ui| {
            if let Some(p) = padding {
                ui.spacing_mut().button_padding = p;
            }
            add_contents(ui)
        });
        if let Some(r) = &shown {
            swap_controls(ctx, r.response.layer_id, r.response.rect, r.inner.is_some(), collapsible, closable);
        }
        shown
    }
}

/// The icons painted on a window's title bar by the last frame (tests read them).
#[derive(Clone, Debug, PartialEq)]
pub struct Painted {
    pub icon: Icon,
    pub size: f32,
    pub center: Pos2,
}

/// The title bar and the line under it, as the last frame painted them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TitleLine {
    pub header: Rect,
    pub y: f32,
    pub color: Color32,
    /// The window frame's fill and edge.
    pub frame: Option<(Color32, egui::Stroke)>,
}

fn line_id(layer: LayerId) -> Id {
    Id::new(("title-line", layer))
}

/// The line under `layer`'s title bar as last painted (`None`: collapsed, or no title bar).
pub fn title_line(ctx: &Context, layer: LayerId) -> Option<TitleLine> {
    ctx.data(|d| d.get_temp::<Option<TitleLine>>(line_id(layer))).flatten()
}

fn painted_id(layer: LayerId) -> Id {
    Id::new(("title-icons", layer))
}

/// What the last frame painted on `layer`'s title bar.
pub fn painted(ctx: &Context, layer: LayerId) -> Vec<Painted> {
    ctx.data(|d| d.get_temp::<Vec<Painted>>(painted_id(layer))).unwrap_or_default()
}

fn swap_controls(ctx: &Context, layer: LayerId, window: Rect, expanded: bool, collapsible: bool, closable: bool) {
    let style = ctx.global_style();
    let heading = ctx.fonts_mut(|f| f.row_height(&egui::TextStyle::Heading.resolve(&style)));
    let stroke = style.visuals.window_stroke.width;
    let side = theme::WINDOW_TITLE_PAD_X as f32;
    let bar = window.top() + 2.0 * stroke + (theme::WINDOW_TITLE_PAD_TOP + theme::WINDOW_TITLE_PAD_BOTTOM) as f32 + heading + 2.0;
    let zone = stroke + side + heading + 2.0;
    let left = Rect::from_min_max(window.min, egui::pos2(window.left() + zone, bar));
    let right = Rect::from_min_max(egui::pos2(window.right() - zone, window.top()), egui::pos2(window.right(), bar));

    // Find egui's shapes first (shared borrow), then blank them.
    let mut triangle: Option<(usize, Rect, Color32)> = None;
    let mut cross: Vec<(usize, Rect, Color32)> = Vec::new();
    // The window's own frame, then its title bar: the rects that span the window's top edge. egui
    // fills the topmost window's title bar with `widgets.open` (the menu bar's open item,
    // `button.hover`); every title bar is `bg.titlebar` (#326), so its fill is put back (at the
    // window's fade-in opacity).
    let mut spans: Vec<(usize, Rect, f32, Color32, egui::Stroke)> = Vec::new();
    let titlebar = theme::TOKENS.bg_titlebar;
    let near = |a: f32, b: f32| (a - b).abs() <= stroke + 0.5;
    ctx.graphics(|g| {
        let Some(list) = g.get(layer) else { return };
        for (i, clipped) in list.all_entries().enumerate() {
            match &clipped.shape {
                Shape::Rect(r)
                    if spans.len() < 2
                        && near(r.rect.top(), window.top())
                        && near(r.rect.left(), window.left())
                        && near(r.rect.right(), window.right()) =>
                {
                    spans.push((i, r.rect, f32::from(r.fill.a()) / 255.0, r.fill, r.stroke));
                }
                Shape::Path(p) if collapsible && p.closed && p.points.len() == 3 && left.contains_rect(clipped.shape.visual_bounding_rect()) => {
                    triangle = Some((i, Rect::from_points(&p.points), p.fill));
                }
                Shape::LineSegment { points, stroke } if closable && right.contains_rect(Rect::from_two_pos(points[0], points[1])) => {
                    cross.push((i, Rect::from_two_pos(points[0], points[1]), stroke.color));
                }
                _ => {}
            }
        }
    });
    let cross = if cross.len() == 2 { cross } else { Vec::new() };

    let mut done = Vec::new();
    let mut paint = |center: Pos2, icon: Icon, color: Color32| {
        icons::paint(&ctx.layer_painter(layer), center, icon, WINDOW_ICON_SIZE, color);
        done.push(Painted { icon, size: WINDOW_ICON_SIZE, center });
    };
    if let Some((_, rect, color)) = triangle {
        paint(rect.center(), if expanded { Icon::ChevronDown } else { Icon::ChevronRight }, color);
    }
    if let [(_, a, color), (_, b, _)] = cross[..] {
        paint(a.union(b).center(), Icon::Remove, color);
    }
    // A 1 px `line` under the title bar when the window shows its body (#326).
    let header = spans.get(1).copied();
    if let Some((i, _, opacity, _, _)) = header {
        ctx.graphics_mut(|g| {
            if let Some(list) = g.get_mut(layer) {
                list.mutate_shape(egui::layers::ShapeIdx(i), |c| {
                    if let Shape::Rect(r) = &mut c.shape {
                        r.fill = titlebar.gamma_multiply(opacity);
                    }
                });
            }
        });
    }
    let frame = spans.first().map(|f| (f.3, f.4));
    let line = header.filter(|_| expanded).map(|(_, h, opacity, _, _)| {
        let y = h.bottom() - 0.5;
        let color = theme::TOKENS.line.gamma_multiply(opacity);
        ctx.layer_painter(layer).line_segment([egui::pos2(h.left() + stroke, y), egui::pos2(h.right() - stroke, y)], egui::Stroke::new(1.0, color));
        TitleLine { header: h, y, color, frame }
    });
    ctx.data_mut(|d| d.insert_temp(line_id(layer), line));
    let blank: Vec<usize> = triangle.iter().map(|t| t.0).chain(cross.iter().map(|c| c.0)).collect();
    ctx.graphics_mut(|g| {
        if let Some(list) = g.get_mut(layer) {
            for i in blank {
                list.reset_shape(egui::layers::ShapeIdx(i));
            }
        }
    });
    ctx.data_mut(|d| d.insert_temp(painted_id(layer), done));
}
