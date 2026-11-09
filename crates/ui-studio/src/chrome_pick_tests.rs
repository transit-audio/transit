//! Choosing in the chrome (#326): the option look, the check rows and the combo's gap.
//! Deterministic: rects and tokens, no pixels, no wall clock.

use crate::chrome_pick::{Combo, choice_label, option_label, option_toggle, option_value};
use crate::theme::DARK;
use crate::tokens::chrome as c;
use crate::widgets::test_support::{Rig, mid};
use egui::{Color32, pos2};

fn rect_fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<Color32> {
    fn walk(s: &egui::Shape, out: &mut Vec<Color32>) {
        match s {
            egui::Shape::Rect(r) => out.push(r.fill),
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    shapes.iter().for_each(|c| walk(&c.shape, &mut out));
    out
}

#[test]
fn an_option_label_is_the_option_toggle_and_egui_selectable_label_is_not() {
    let mut rig = Rig::new();
    let (r, shapes) = rig.shapes(|ui| option_label(ui, true, "Notes"));
    assert!(rect_fills(&shapes).contains(&DARK.toggle_option_fill), "on: the option fill");
    assert_eq!(r.rect.height(), crate::tokens::toggle::HEIGHT, "24 high");
    let (r, shapes) = rig.shapes(|ui| option_label(ui, false, "Notes"));
    assert!(!rect_fills(&shapes).contains(&DARK.toggle_option_fill), "off: not the option fill");
    assert!(rect_fills(&shapes).contains(&DARK.surface_raised), "off: surface.raised");
    assert_eq!(r.rect.height(), crate::tokens::toggle::HEIGHT);
    // Negative control: egui's selectable label, selected, is the selection fill.
    let (_, egui_shapes) = rig.shapes(|ui| ui.selectable_label(true, "Notes"));
    assert!(!rect_fills(&egui_shapes).contains(&DARK.toggle_option_fill));
    assert!(rect_fills(&egui_shapes).contains(&DARK.signal.gamma_multiply(c::SELECTION_ALPHA_DARK)));
}

#[test]
fn option_value_and_option_toggle_behave_like_the_egui_calls() {
    let mut rig = Rig::new();
    let (mut which, mut flag) = (1u8, false);
    let rects =
        rig.idle(|ui| (option_value(ui, &mut which, 1, "A").rect, option_value(ui, &mut which, 2, "B").rect, option_toggle(ui, &mut flag, "F").rect));
    rig.click(mid(crate::widgets::r4(rects.1)), |ui| {
        option_value(ui, &mut which, 1, "A");
        option_value(ui, &mut which, 2, "B");
        option_toggle(ui, &mut flag, "F");
    });
    assert_eq!((which, flag), (2, false), "a click on B picks B and touches nothing else");
    rig.click(mid(crate::widgets::r4(rects.2)), |ui| {
        option_value(ui, &mut which, 1, "A");
        option_value(ui, &mut which, 2, "B");
        option_toggle(ui, &mut flag, "F");
    });
    assert!(flag, "a click flips the flag");
    // Negative control: a click beside the controls changes nothing.
    rig.click(pos2(600.0, 400.0), |ui| {
        option_toggle(ui, &mut flag, "F");
    });
    assert!(flag);
}

#[test]
fn a_choice_label_is_a_menu_row_not_a_toggle() {
    let mut rig = Rig::new();
    let (r, shapes) = rig.shapes(|ui| choice_label(ui, true, "Sustain"));
    assert!(!rect_fills(&shapes).contains(&DARK.toggle_option_fill));
    assert!(r.rect.height() >= 18.0);
}

/// The popup rect after a click on a face drawn by `face`, found by id.
fn popup_top_and_face_bottom(face: impl Fn(&mut egui::Ui) -> egui::Response) -> (f32, f32) {
    let mut rig = Rig::new();
    let r = rig.idle(&face);
    let at = r.rect.center();
    rig.click(at, &face);
    let mut area = None;
    for _ in 0..4 {
        rig.idle(&face);
        area = rig.ctx.memory(|m| m.area_rect(r.id.with("popup")));
    }
    (area.expect("the popup opened").min.y, r.rect.max.y)
}

#[test]
fn a_combo_list_opens_four_px_below_its_field() {
    let (top, bottom) = popup_top_and_face_bottom(|ui| {
        Combo::new("pick", "Value")
            .show(ui, |ui| {
                choice_label(ui, true, "Value");
            })
            .response
    });
    assert_eq!(c::COMBO_POPUP_GAP, 4.0);
    assert_eq!(top, bottom + c::COMBO_POPUP_GAP, "popup top = field bottom + 4");
    // Negative control: egui's own popup (gap 0) sits flush.
    let (top, bottom) = popup_top_and_face_bottom(|ui| {
        let resp = ui.button("Value");
        egui::Popup::menu(&resp).show(|ui| {
            ui.label("x");
        });
        resp
    });
    assert_eq!(top, bottom, "gap 0 is flush");
}

#[test]
fn egui_menu_bar_menus_are_flush_and_the_chrome_bar_menu_is_clear_of_the_bar() {
    let (top, bottom) = popup_top_and_face_bottom(|ui| {
        egui::MenuBar::new()
            .ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    ui.label("Open");
                })
                .response
            })
            .inner
    });
    // Negative control (#418): egui's own bar menu sits flush under its item.
    assert_eq!(top, bottom, "egui's menus from the bar are flush");
    // The chrome's bar menu: the bar's padding under the item plus `chrome.comboPopupGap`.
    let (top, bottom) = popup_top_and_face_bottom(|ui| {
        egui::MenuBar::new().ui(ui, |ui| crate::chrome::bar_menu(ui, "File", false, |ui| ui.label("Open")).response).inner
    });
    let below_item = (c::MENU_BAR_HEIGHT - c::MENU_BAR_ITEM_HEIGHT) / 2.0;
    assert_eq!(top, bottom + below_item + crate::chrome::POPUP_MENU_BAR_GAP, "the bar's menus clear the bar by 4");
}

#[test]
fn no_view_uses_egui_selectable_widgets_any_more() {
    fn offenders(src: &str) -> usize {
        src.lines()
            .filter(|l| {
                !l.trim_start().starts_with("//")
                    && (l.contains(".selectable_label(") || l.contains(".selectable_value(") || l.contains(".toggle_value("))
            })
            .count()
    }
    assert_eq!(offenders("let b = ui.selectable_label(a, \"x\");"), 1, "the scan sees one");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name.ends_with("tests.rs") || name == "tests.rs" {
                continue;
            }
            if offenders(&std::fs::read_to_string(&path).unwrap()) > 0 {
                found.push(name);
            }
        }
    }
    assert!(found.is_empty(), "migrated to the chrome's option and choice helpers: {found:?}");
}

/// The x of the text "Row" a popup list painted, and the left edge of its row.
fn list_text_offset(icon_spacing: Option<f32>) -> f32 {
    use std::cell::Cell;
    let row = Cell::new(egui::Rect::NOTHING);
    let face = |ui: &mut egui::Ui| {
        Combo::new("rows", "Value")
            .show(ui, |ui| {
                if let Some(g) = icon_spacing {
                    ui.spacing_mut().icon_spacing = g;
                }
                row.set(choice_label(ui, true, "Row").rect);
            })
            .response
    };
    let mut rig = Rig::new();
    let r = rig.idle(face);
    rig.click(r.rect.center(), face);
    for _ in 0..3 {
        rig.idle(face);
    }
    let (_, shapes) = rig.shapes(face);
    fn find(s: &egui::Shape, out: &mut Option<f32>) {
        match s {
            egui::Shape::Text(t) if t.galley.job.text == "Row" => *out = Some(t.pos.x),
            egui::Shape::Vec(v) => v.iter().for_each(|x| find(x, out)),
            _ => {}
        }
    }
    let mut x = None;
    shapes.iter().for_each(|c| find(&c.shape, &mut x));
    x.expect("the row text was painted") - row.get().min.x
}

#[test]
fn a_combo_list_row_starts_its_text_after_the_padding_and_the_check_gutter() {
    let offset = list_text_offset(None);
    assert_eq!(offset, c::POPUP_PADDING + c::CHECK_GUTTER, "text x = padding 4 + gutter 16");
    // Negative control: egui's default icon spacing left in adds its gap.
    assert!(list_text_offset(Some(4.0)) > offset);
}

/// Whether a list holding a text field is still open after a click into the field.
fn open_after_clicking_the_field(popup: impl Fn(&egui::Response) -> egui::Popup<'static>) -> bool {
    let mut text = String::new();
    let field_at = std::cell::Cell::new(None);
    let mut face = |ui: &mut egui::Ui| {
        let r = ui.button("Open");
        popup(&r).show(|ui| {
            field_at.set(Some(ui.text_edit_singleline(&mut text).rect.center()));
        });
        r
    };
    let mut rig = Rig::new();
    let r = rig.idle(&mut face);
    rig.click(r.rect.center(), &mut face);
    for _ in 0..3 {
        rig.idle(&mut face);
    }
    let at = field_at.get().expect("the list opened");
    rig.click(at, &mut face);
    rig.idle(&mut face);
    egui::Popup::is_id_open(&rig.ctx, egui::Popup::default_response_id(&r))
}

#[test]
fn a_list_with_a_field_closes_on_the_click_into_it_unless_it_is_dropdown_with_fields() {
    assert!(open_after_clicking_the_field(crate::chrome_pick::dropdown_with_fields), "stays open");
    // Negative control: the default close-on-click closes it at the first click.
    assert!(!open_after_clicking_the_field(crate::chrome_pick::dropdown), "the plain dropdown closes");
}

/// Gap 11: a combo inside a device panel is the dense control height, a chrome combo the chrome's.
#[test]
fn a_panel_combo_face_is_the_dense_control_height() {
    use crate::tokens::layout;
    let mut rig = Rig::new();
    let dense = rig.idle(|ui: &mut egui::Ui| Combo::new("d", "Value").dense().show(ui, |_| ()).response);
    assert_eq!(dense.rect.height(), layout::DENSE_CONTROL_HEIGHT, "panel combo is 24 (fails at 22)");
    let chrome = rig.idle(|ui: &mut egui::Ui| Combo::new("c", "Value").show(ui, |_| ()).response);
    assert_eq!(chrome.rect.height(), layout::CHROME_CONTROL_HEIGHT, "chrome combo stays 26");
}
