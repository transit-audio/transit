use super::*;

enum PRow {
    Heading(String),
    Vendor(String),
    Note(String),
    Item(usize),
}

impl StudioApp {
    /// Put a plug-in where it was dropped: an instrument on an instrument track (or a new
    /// instrument track below the tracks), an effect at the end of a track's chain.
    pub fn drop_plugin(&mut self, id: &str, name: &str, instrument: bool, target: Target) -> Result<(), String> {
        let track = match target {
            Target::Track { track, .. } => track,
            Target::Below { .. } if instrument => return self.new_instrument_track(id, name),
            Target::Below { .. } => {
                self.status = format!("Drop {name} on a track, a mixer strip or the device chain");
                return Err(self.status.clone());
            }
        };
        let Some(t) = self.engine.session().track(track).map(|t| (t.kind, t.name.clone())) else { return Err("no such track".into()) };
        if instrument {
            if t.0 != TrackKind::Instrument {
                self.status = format!("{name} is an instrument: drop it on an instrument track (or below the tracks for a new one)");
                return Err(self.status.clone());
            }
            self.set_instrument(track, id)?;
            self.status = format!("Loaded {name} on {}", t.1);
        } else {
            let before = self.status.clone();
            self.add_device(track, id)?;
            if self.status == before {
                self.status = format!("Added {name} to {}", t.1);
            }
        }
        self.ui.focus_track = Some(track);
        Ok(())
    }

    /// Double-click on a plug-in: the same as a drop on the focused track.
    pub fn plugin_to_focus(&mut self, id: &str, name: &str, instrument: bool) -> Result<(), String> {
        let Some(track) = self.focus_track() else {
            self.status = format!("Click a track first, then double-click {name}");
            return Err(self.status.clone());
        };
        self.drop_plugin(id, name, instrument, Target::Track { track, at: None })
    }

    /// A new instrument track playing `id`: `track.new` and `mix.instrument` as one undo step.
    fn new_instrument_track(&mut self, id: &str, name: &str) -> Result<(), String> {
        let before = self.engine.session_arc();
        self.browser.serial += 1;
        let key = format!("browser:instrument:{}", self.browser.serial);
        self.engine.end_merge();
        let r = self.engine.execute_merged("track.new", &json!({"kind": "instrument"}), &key).and_then(|v| {
            let t = v["tracks"][0].as_u64().unwrap_or_default();
            self.engine.execute_merged("mix.instrument", &json!({"track": t, "plugin": id}), &key).map(|_| TrackId(t))
        });
        self.engine.end_merge();
        match r {
            Ok(t) => {
                let tn = self.engine.session().track(t).map_or_else(String::new, |t| t.name.clone());
                self.status = format!("Loaded {name} on a new track, {tn}");
                self.ui.focus_track = Some(t);
                Ok(())
            }
            Err(e) => {
                if !std::sync::Arc::ptr_eq(&before, &self.engine.session_arc()) {
                    self.engine.undo();
                }
                self.status = format!("{name}: {e}");
                Err(self.status.clone())
            }
        }
    }
}

/// "Scanning…", "Not scanned yet…" or a failure, for the Plug-ins tab; `None` once scanned.
pub(super) fn scan_note() -> Option<String> {
    if let Some(s) = crate::plugin_scan::scanning_label() {
        return Some(s);
    }
    match crate::plugin_scan::state().as_str() {
        "idle" => Some("VST3 and CLAP: not scanned yet (Audio › Scan Plug-ins)".into()),
        "done" => None,
        other => Some(format!("Scan {other}")),
    }
}

/// Built-in plug-ins plus the scan caches, refreshed when the scan state changes.
fn catalog(app: &mut StudioApp) -> Vec<PluginItem> {
    let state = crate::plugin_scan::state();
    if let Some((s, items)) = &app.browser.catalog
        && *s == state
    {
        return items.clone();
    }
    let mut items: Vec<PluginItem> = app
        .builtin_plugins()
        .into_iter()
        .map(|e| PluginItem { id: e.id, name: e.name, vendor: "Built-in".into(), format: "Built-in", instrument: e.instrument, category: e.category })
        .collect();
    for (format, list) in [("VST3", crate::plugin_scan::cached_vst3()), ("CLAP", crate::plugin_scan::cached_clap())] {
        for crate::plugin_scan::CachedPlugin { id, name, vendor, instrument } in list.unwrap_or_default() {
            let vendor = if vendor.trim().is_empty() { "Other".to_string() } else { vendor };
            items.push(PluginItem { id, name, vendor, format, instrument, category: String::new() });
        }
    }
    app.browser.catalog = Some((state, items.clone()));
    items
}

/// The Plug-ins tab's rows: favorites, then instruments, then effects, each by vendor
/// (built-in first).
fn plugin_rows(items: &[PluginItem], favorites: &[String], filter: &str) -> Vec<PRow> {
    let f = filter.trim().to_lowercase();
    let keep = |p: &PluginItem| {
        f.is_empty() || [p.name.as_str(), p.vendor.as_str(), p.format, p.category.as_str()].iter().any(|s| s.to_lowercase().contains(&f))
    };
    let mut out = Vec::new();
    let favs: Vec<usize> = favorites.iter().filter_map(|id| items.iter().position(|p| &p.id == id)).filter(|i| keep(&items[*i])).collect();
    if !favs.is_empty() {
        out.push(PRow::Heading("Favorites".into()));
        out.extend(favs.into_iter().map(PRow::Item));
    }
    for (heading, instrument) in [("Instruments", true), ("Effects", false)] {
        let mut idx: Vec<usize> = (0..items.len()).filter(|i| items[*i].instrument == instrument && keep(&items[*i])).collect();
        idx.sort_by_key(|i| {
            let p = &items[*i];
            (p.vendor != "Built-in", p.vendor.to_lowercase(), p.name.to_lowercase())
        });
        out.push(PRow::Heading(heading.into()));
        if idx.is_empty() {
            out.push(PRow::Note(if f.is_empty() { "None".into() } else { "No match".into() }));
        }
        let mut vendor: Option<&str> = None;
        for i in idx {
            let v = items[i].vendor.as_str();
            if vendor != Some(v) {
                out.push(PRow::Vendor(v.to_string()));
                vendor = Some(v);
            }
            out.push(PRow::Item(i));
        }
    }
    out
}

pub(super) fn plugins(app: &mut StudioApp, ui: &mut Ui, area: Rect, layout: &mut BrowserLayout) {
    let items = catalog(app);
    let mut rows = Vec::new();
    if let Some(note) = scan_note() {
        rows.push(PRow::Note(note));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    rows.extend(plugin_rows(&items, &app.ui.browser.favorites, &app.browser.plugin_filter));
    let content_h = rows.len() as f32 * ROW_H + 8.0;
    app.browser.plugin_scroll = wheel(ui, area, app.browser.plugin_scroll, content_h);
    let p = ui.painter().with_clip_rect(area);
    let mut star_click: Option<String> = None;
    let mut pick: Option<(usize, bool)> = None;
    let mut drag_from: Option<usize> = None;
    for (i, row) in rows.iter().enumerate() {
        let y = area.min.y + i as f32 * ROW_H - app.browser.plugin_scroll;
        let r = Rect::from_min_size(pos2(area.min.x, y), vec2(area.width(), ROW_H));
        let shown = r.max.y >= area.min.y && r.min.y <= area.max.y;
        let mut out = PluginRowLayout { rect: shown.then(|| r4(r)), ..PluginRowLayout::default() };
        match row {
            PRow::Heading(t) => {
                out.row = "heading";
                out.name.clone_from(t);
                if shown {
                    p.text(pos2(r.min.x + 8.0, r.center().y + 2.0), Align2::LEFT_CENTER, t, FontId::proportional(12.0), TOKENS.text);
                }
            }
            PRow::Vendor(t) => {
                out.row = "vendor";
                out.name.clone_from(t);
                if shown {
                    p.text(pos2(r.min.x + 14.0, r.center().y), Align2::LEFT_CENTER, t, FontId::proportional(10.5), TOKENS.text_dim);
                }
            }
            PRow::Note(t) => {
                out.row = "note";
                out.name.clone_from(t);
                if shown {
                    p.text(pos2(r.min.x + 14.0, r.center().y), Align2::LEFT_CENTER, t, FontId::proportional(10.5), TOKENS.text_dim);
                }
            }
            PRow::Item(k) => {
                let it = &items[*k];
                let fav = app.ui.browser.favorites.contains(&it.id);
                let selected = app.browser.plugin_selected.as_deref() == Some(it.id.as_str());
                out = PluginRowLayout {
                    row: "plugin",
                    id: it.id.clone(),
                    name: it.name.clone(),
                    vendor: it.vendor.clone(),
                    format: it.format.to_string(),
                    instrument: it.instrument,
                    favorite: fav,
                    selected,
                    rect: out.rect,
                    star: None,
                };
                if shown {
                    let resp = ui.interact(r.intersect(area), ui.id().with(("browser_plugin", i, &it.id)), Sense::click_and_drag());
                    if selected {
                        p.rect_filled(r, 0.0, TOKENS.accent.gamma_multiply(0.28));
                    } else if resp.hovered() {
                        p.rect_filled(r, 0.0, TOKENS.button);
                    }
                    let sr = Rect::from_min_size(pos2(r.max.x - 24.0, r.min.y + 2.0), vec2(16.0, 16.0));
                    let tag = if it.format == "Built-in" { it.category.clone() } else { it.format.to_string() };
                    p.text(pos2(sr.min.x - 6.0, r.center().y), Align2::RIGHT_CENTER, &tag, FontId::proportional(10.0), TOKENS.text_dim);
                    let name_r = Rect::from_min_max(pos2(r.min.x + 24.0, r.min.y), pos2(sr.min.x - 60.0, r.max.y));
                    p.with_clip_rect(name_r.intersect(area)).text(
                        name_r.left_center(),
                        Align2::LEFT_CENTER,
                        &it.name,
                        FontId::proportional(12.0),
                        TOKENS.text,
                    );
                    let sresp = ui.interact(sr, ui.id().with(("browser_star", i, &it.id)), Sense::click());
                    let ink = if fav {
                        TOKENS.accent
                    } else if sresp.hovered() {
                        TOKENS.text
                    } else {
                        TOKENS.text_dim.gamma_multiply(0.6)
                    };
                    crate::icons::paint(&p, sr.center(), if fav { Icon::StarFilled } else { Icon::Star }, 13.0, ink);
                    if sresp.on_hover_text(if fav { "Remove from Favorites" } else { "Add to Favorites" }).clicked() {
                        star_click = Some(it.id.clone());
                    }
                    out.star = Some(r4(sr));
                    if resp.clicked() {
                        pick = Some((*k, false));
                    }
                    if resp.double_clicked() {
                        pick = Some((*k, true));
                    }
                    if resp.drag_started() {
                        drag_from = Some(*k);
                    }
                }
            }
        }
        layout.plugins.push(out);
    }
    if let Some(id) = star_click {
        let favs = &mut app.ui.browser.favorites;
        if let Some(k) = favs.iter().position(|f| *f == id) {
            favs.remove(k);
        } else {
            favs.push(id);
        }
    } else if let Some((k, double)) = pick {
        let it = &items[k];
        app.browser.plugin_selected = Some(it.id.clone());
        if double {
            let _ = app.plugin_to_focus(&it.id, &it.name, it.instrument);
        }
    }
    if let Some(k) = drag_from {
        let it = &items[k];
        app.browser.plugin_selected = Some(it.id.clone());
        app.browser.drag =
            Some(Drag { payload: Payload::Plugin { id: it.id.clone(), name: it.name.clone(), instrument: it.instrument }, target: None });
    }
}
