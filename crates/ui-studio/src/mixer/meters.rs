use super::*;

fn meter_k(lin: f32) -> f32 {
    if lin <= 0.0 {
        return 0.0;
    }
    ((20.0 * lin.log10() + 60.0) / 66.0).clamp(0.0, 1.0)
}

pub(super) fn draw_meter(p: &egui::Painter, r: Rect, level: [f32; 2], clip: bool) {
    p.rect_filled(r, crate::theme::RADIUS_SM, TOKENS.bg);
    let w = (r.width() - 3.0) / 2.0;
    for (c, lvl) in level.iter().enumerate() {
        let x = r.min.x + 1.0 + c as f32 * (w + 1.0);
        let h = (r.height() - 2.0) * meter_k(*lvl);
        let bar = Rect::from_min_max(pos2(x, r.max.y - 1.0 - h), pos2(x + w, r.max.y - 1.0));
        p.rect_filled(bar, 0.0, if *lvl > 0.9 { TOKENS.solo } else { TOKENS.monitor });
    }
    // A clip latch is a small red cap at the top of the bar, not a red bar.
    if clip {
        p.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), CLIP_CAP)), 0.0, TOKENS.record);
    }
}

/// Height of the clip latch's cap at the top of a meter.
pub const CLIP_CAP: f32 = 3.0;

impl StudioApp {
    /// Clear the clip latch and the peak hold of one strip, or of every strip and the main meter.
    pub fn reset_meters(&mut self, only: Option<TrackId>) {
        match only {
            Some(id) => {
                if let Some(d) = self.meters.get_mut(&id) {
                    (d.peak, d.clip) = (0.0, false);
                }
                if self.engine.session().track(id).is_some_and(|t| t.kind == TrackKind::Master) {
                    (self.main_meter.peak, self.main_meter.clip) = (0.0, false);
                }
            }
            None => {
                for d in self.meters.values_mut().chain(std::iter::once(&mut self.main_meter)) {
                    (d.peak, d.clip) = (0.0, false);
                }
            }
        }
    }

    /// Clear the main meter and, when there is one, the Master track's own meter: the master
    /// strip shows `meters[master]` and the transport shows `main_meter`.
    pub fn reset_main_meter(&mut self) {
        (self.main_meter.peak, self.main_meter.clip) = (0.0, false);
        let master = self.engine.session().tracks.iter().find(|t| t.kind == TrackKind::Master).map(|t| t.id);
        if let Some(d) = master.and_then(|id| self.meters.get_mut(&id)) {
            (d.peak, d.clip) = (0.0, false);
        }
    }

    /// `track.clear_clip_indicators` (Alt+C), and playback starting: every clip latch off. The
    /// peak holds stay until clicked.
    pub fn clear_clip_latches(&mut self) {
        for d in self.meters.values_mut().chain(std::iter::once(&mut self.main_meter)) {
            d.clip = false;
        }
    }
}

pub fn peak_text(lin: f32) -> (String, bool) {
    if lin <= 0.0 {
        return ("-inf".into(), false);
    }
    let db = 20.0 * lin.log10();
    let t = format!("{db:.1}");
    let over = db >= -0.05;
    (
        if db > 0.05 {
            format!("+{t}")
        } else if t == "-0.0" {
            "0.0".into()
        } else {
            t
        },
        over,
    )
}
