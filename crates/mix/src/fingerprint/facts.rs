//! Test seam (feature `testing`; ADR 0029, amendment for #742, section 14, the window rule): what
//! the scheduling gate's post-fault window is computed from, read from the configured, loaded
//! instances the active plan holds (after the latency probe block), not from instances built
//! anew without their content.

use super::super::*;

/// One audio instance of a strip: an insert, the instrument or a clip-effect module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFact {
    /// Its registry entry (the slot's plug-in id; the plug-in's own id when the slot has none).
    pub entry: String,
    /// The strip's instrument.
    pub instrument: bool,
    /// [`Plugin::tail_samples`] and [`Plugin::latency`] of the loaded instance.
    pub tail: usize,
    pub latency: usize,
    /// [`Plugin::print_complete`].
    pub complete: bool,
}

/// One MIDI effect of the strip's instrument's effective chain (the track's effects and its
/// rack's, every stage of the chain that holds an effect, bypassed or not).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiFact {
    pub entry: String,
    pub complete: bool,
}

/// A home strip's instances.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StripFacts {
    pub track: TrackId,
    pub audio: Vec<AudioFact>,
    pub midi: Vec<MidiFact>,
    /// The strip's hardware inserts and External Instrument, by entry (`hardware.insert`,
    /// `external.instrument`): slot state not inventoried yet (#742 ruling W-1).
    pub external: Vec<String>,
}

impl MixEngine {
    /// Every home strip's instances (strips a render worker holds are left out: call it between
    /// blocks with every strip home). Off the audio path.
    #[must_use]
    pub fn strip_plugin_facts(&self) -> Vec<StripFacts> {
        let Some(p) = self.plan.as_deref() else { return Vec::new() };
        let mut out = Vec::new();
        for (i, st) in p.tables.strips.iter().enumerate() {
            if self.pool.as_ref().is_some_and(|pool| pool.is_away(i)) {
                continue;
            }
            let fact = |s: &PluginSlot, instrument: bool| AudioFact {
                entry: if s.id.is_empty() { s.plugin.info().id.to_string() } else { s.id.clone() },
                instrument,
                tail: s.plugin.tail_samples(),
                latency: s.plugin.latency(),
                complete: s.plugin.print_complete(),
            };
            let mut audio: Vec<AudioFact> = st.plugins.iter().flatten().map(|s| fact(s, false)).collect();
            audio.extend(st.instrument.iter().map(|s| fact(s, true)));
            for (_, fx) in &st.clip_fx.0 {
                audio.extend(fx.eq.iter().chain(fx.comp.iter()).map(|s| fact(s, false)));
            }
            let midi = st
                .instrument
                .iter()
                .filter_map(|s| s.events.as_deref())
                .flat_map(|e| e.fx.chain_facts())
                .map(|(entry, complete)| MidiFact { entry: entry.to_string(), complete })
                .collect();
            let mut external: Vec<String> = st.hw.iter().map(|_| transit_model::HARDWARE_PLUGIN.to_string()).collect();
            external.extend(st.ext.iter().map(|_| transit_model::EXTERNAL_INSTRUMENT_PLUGIN.to_string()));
            out.push(StripFacts { track: st.id, audio, midi, external });
        }
        out
    }
}
