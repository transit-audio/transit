use super::*;

/// A strip's stereo meter (Fader > Meter) at `x`, spanning the fader's `travel`: the kit's
/// vertical channel meter fed with the strip's fall-off reading, the engine's RMS per channel (the
/// solid `meter.ok` layer over the 35 percent peak, #395), peak hold and clip latch. The main bus
/// has the wider bars.
pub(super) fn strip_meter(
    p: &egui::Painter,
    x: f32,
    travel: Rect,
    d: &crate::MeterDisplay,
    main: bool,
) -> crate::widgets::level_meter::ChannelMeterLayout {
    use crate::widgets::level_meter::{Channel, paint_channel};
    let db = |lin: f32| if lin > 0.0 { 20.0 * lin.log10() } else { f32::NEG_INFINITY };
    let channels =
        [0, 1].map(|c| Channel { peak_db: db(d.falloff[c]), rms_db: Some(db(d.rms[c])), hold_db: Some(db(d.hold[c])), clip: d.clip_ch[c] });
    let bar = if main { crate::tokens::fader::METER_BAR_MAIN } else { crate::tokens::fader::METER_BAR };
    paint_channel(p, x, travel, bar, &channels)
}

/// The rect around a channel meter's wells and clip cells (what a click on the meter hits).
pub(super) fn meter_bounds(m: &crate::widgets::level_meter::ChannelMeterLayout) -> Rect {
    m.wells.iter().chain(&m.clip_cells).fold(Rect::NOTHING, |acc, r| acc.union(Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]))))
}

/// Height of the clip latch's cap on the transport's main meter.
pub const CLIP_CAP: f32 = 3.0;

impl StudioApp {
    /// Clear the clip latch and the peak hold of one strip, or of every strip and the main meter.
    pub fn reset_meters(&mut self, only: Option<TrackId>) {
        match only {
            Some(id) => {
                if let Some(d) = self.meters.get_mut(&id) {
                    {
                        d.peak = 0.0;
                        d.clear_clip();
                    }
                }
                if self.engine.session().track(id).is_some_and(|t| t.kind == TrackKind::Master) {
                    {
                        self.main_meter.peak = 0.0;
                        self.main_meter.clear_clip();
                    }
                }
            }
            None => {
                for d in self.meters.values_mut().chain(std::iter::once(&mut self.main_meter)) {
                    {
                        d.peak = 0.0;
                        d.clear_clip();
                    }
                }
            }
        }
    }

    /// Clear the main meter and, when there is one, the Master track's own meter: the master
    /// strip shows `meters[master]` and the transport shows `main_meter`.
    pub fn reset_main_meter(&mut self) {
        {
            self.main_meter.peak = 0.0;
            self.main_meter.clear_clip();
        }
        let master = self.engine.session().tracks.iter().find(|t| t.kind == TrackKind::Master).map(|t| t.id);
        if let Some(d) = master.and_then(|id| self.meters.get_mut(&id)) {
            {
                d.peak = 0.0;
                d.clear_clip();
            }
        }
    }

    /// `track.clear_clip_indicators` (Alt+C), and playback starting: every clip latch off. The
    /// peak holds stay until clicked.
    pub fn clear_clip_latches(&mut self) {
        for d in self.meters.values_mut().chain(std::iter::once(&mut self.main_meter)) {
            d.clear_clip();
        }
    }
}

/// The peak readout's text (with the typographic minus, [`crate::fine::minus`]) and whether it
/// is above 0 dBFS.
pub fn peak_text(lin: f32) -> (String, bool) {
    if lin <= 0.0 {
        return (crate::fine::minus("-inf".into()), false);
    }
    let db = 20.0 * lin.log10();
    let t = format!("{db:.1}");
    // Hot only above 0 dBFS, the rule of the meter's fill (the README: `meter.hot` when > 0).
    let over = db > 0.0;
    (
        if over {
            format!("+{t}")
        } else if t == "-0.0" {
            "0.0".into()
        } else {
            crate::fine::minus(t)
        },
        over,
    )
}
