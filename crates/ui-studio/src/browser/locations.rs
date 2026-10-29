//! Locations from the Files tree (#212): the right-click menu on a folder row ("Add to
//! Locations", "Remove from Locations") and the drag of a folder row onto the Locations header.
//! Locations stay a view preference in `BrowserPrefs`; nothing here touches a folder on disk.

use super::{Row, StudioApp};
use crate::mixer::MenuItem;
use egui::{Response, Ui};
use std::path::{Path, PathBuf};

/// What a menu pick asks for; applied after the row loop.
pub(super) enum Act {
    Add(PathBuf),
    Remove(usize),
}

fn parts(p: &str) -> Vec<String> {
    Path::new(p).components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect()
}

/// The location that is `path` itself (case-insensitively), if any.
pub(super) fn same_as(locations: &[String], path: &Path) -> Option<String> {
    let p = parts(&path.to_string_lossy());
    locations.iter().find(|l| parts(l) == p).cloned()
}

/// The location `path` lies inside (a proper parent), if any. The first one added wins.
pub(super) fn inside(locations: &[String], path: &Path) -> Option<String> {
    let p = parts(&path.to_string_lossy());
    locations
        .iter()
        .find(|l| {
            let q = parts(l);
            q.len() < p.len() && p.starts_with(&q)
        })
        .cloned()
}

impl StudioApp {
    /// Add a folder seen in the tree to Locations (a no-op with a note when it already is one).
    /// A folder inside another location is allowed: it then also shows once at the top.
    pub fn add_location_from_tree(&mut self, dir: &Path) -> bool {
        self.add_location(&dir.to_string_lossy())
    }
}

/// The context menu of a Files row: folders offer Add, a user location offers Remove.
pub(super) fn row_menu(app: &mut StudioApp, resp: &Response, row: &Row) -> Option<Act> {
    if row.kind != super::Kind::Folder {
        return None;
    }
    let mut act = None;
    resp.context_menu(|ui| {
        if let Some(Some(li)) = row.root {
            let b = ui.button("Remove from Locations");
            publish(app, ui, &b, "Remove from Locations", "remove");
            if b.clicked() {
                act = Some(Act::Remove(li));
                ui.close();
            }
        } else if row.root.is_none() {
            let same = same_as(&app.ui.browser.locations, &row.path);
            let b = ui.add_enabled(same.is_none(), egui::Button::new("Add to Locations"));
            publish(app, ui, &b, "Add to Locations", if same.is_some() { "disabled" } else { "add" });
            if let Some(by) = same {
                ui.weak(format!("Already in Locations: {by}"));
            } else if let Some(by) = inside(&app.ui.browser.locations, &row.path) {
                ui.weak(format!("Inside the location {by}; adds it as its own"));
            }
            if b.clicked() {
                act = Some(Act::Add(row.path.clone()));
                ui.close();
            }
        }
    });
    act
}

fn publish(app: &mut StudioApp, ui: &Ui, b: &Response, label: &str, value: &str) {
    app.browser.menu.retain(|m| m.label != label);
    app.browser.menu.push(MenuItem::at(ui, label.into(), value.into(), b.rect));
}
