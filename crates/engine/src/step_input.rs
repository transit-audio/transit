//! Deterministic step-entry timing. The caller supplies monotonic milliseconds; no audio
//! device or wall clock is needed, and pending notes do not mutate the document.

/// First repeat interval after the threshold, before it eases toward the floor.
const HOLD_RAMP_START_MS: u64 = 400;
/// Upper bound on the ramp length, so the schedule loop stays small for any prefs file.
const MAX_RAMP_UNITS: u32 = 64;

/// Hold preferences, persisted by the UI rather than in the musical document.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HoldPrefs {
    pub enabled: bool,
    pub threshold_ms: u64,
    pub floor_ms: u64,
    /// How many units the repeat interval takes to ease from its start to the floor.
    pub ramp_units: u32,
}

impl Default for HoldPrefs {
    fn default() -> Self {
        Self { enabled: true, threshold_ms: 500, floor_ms: 200, ramp_units: 6 }
    }
}

impl HoldPrefs {
    /// Number of step units at this elapsed time. Arithmetic is bounded even for a
    /// malformed preference file or a very long suspended UI frame.
    pub fn units(self, elapsed_ms: u64) -> u32 {
        let threshold = self.threshold_ms.clamp(1, 60_000);
        let floor = self.floor_ms.clamp(1, threshold);
        if elapsed_ms < threshold {
            return 1;
        }
        let mut remaining = elapsed_ms - threshold;
        let mut units = 2u64;
        // The repeat interval eases linearly from `start` down to the floor over
        // `ramp_units` units, then stays at the floor. It never drops below the floor.
        let start = HOLD_RAMP_START_MS.min(threshold).max(floor);
        let ramp = u64::from(self.ramp_units.min(MAX_RAMP_UNITS));
        for i in 0..ramp {
            let interval = start - (start - floor) * i / ramp;
            if remaining < interval {
                return units as u32;
            }
            remaining -= interval;
            units += 1;
        }
        units.saturating_add(remaining / floor).min(u64::from(u32::MAX)) as u32
    }
}

/// One completed chord, including notes released before the last finger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepChord {
    pub notes: Vec<(u8, u8)>,
    pub units: u32,
}

/// A learnable key/pad or controller. Bindings apply only during step entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StepBinding {
    Note(u8),
    Cc(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepControl {
    TieRest,
    Rest,
    Back,
    Tie,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct StepBindings {
    // Older saved bindings keep their explicit assignments; do not shadow a
    // learned CC 64 action when the new field is absent. Fresh prefs use Default.
    #[serde(default)]
    pub tie_rest: Option<StepBinding>,
    pub rest: Option<StepBinding>,
    pub back: Option<StepBinding>,
    pub tie: Option<StepBinding>,
}

impl Default for StepBindings {
    fn default() -> Self {
        Self { tie_rest: Some(StepBinding::Cc(64)), rest: None, back: None, tie: None }
    }
}

impl StepBindings {
    /// One message performs one action. Learning a binding moves it from any old action.
    pub fn learn(&mut self, action: StepControl, binding: StepBinding) {
        for slot in [&mut self.tie_rest, &mut self.rest, &mut self.back, &mut self.tie] {
            if *slot == Some(binding) {
                *slot = None;
            }
        }
        *match action {
            StepControl::TieRest => &mut self.tie_rest,
            StepControl::Rest => &mut self.rest,
            StepControl::Back => &mut self.back,
            StepControl::Tie => &mut self.tie,
        } = Some(binding);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepAction {
    Enter(StepChord),
    /// Sustain plus a replayed chord: only extend if the previous pitches match.
    TieChord(StepChord),
    Rest(u32),
    Back,
    Tie,
}

/// UI-thread controller semantics. A pedal tap is deferred until release so pressing
/// a chord while the pedal is held can mean tie without also inserting a rest.
#[derive(Debug, Default)]
pub struct StepController {
    input: StepInput,
    pressed: Vec<StepBinding>,
    rest: Option<(u64, u32)>,
    pedal: bool,
    tie_chord: bool,
}

impl StepController {
    pub fn is_pending(&self) -> bool {
        self.rest.is_some() || self.input.started_ms.is_some()
    }

    pub fn cancel(&mut self) {
        *self = Self::default();
    }

    pub fn preview(&self, now_ms: u64, prefs: HoldPrefs) -> Option<StepChord> {
        self.input.preview(now_ms, prefs)
    }

    /// Emits a single bounded batch even after a suspended frame.
    pub fn poll(&mut self, now_ms: u64, prefs: HoldPrefs) -> Option<StepAction> {
        let (started, emitted) = self.rest.as_mut()?;
        let units = prefs.units(now_ms.saturating_sub(*started));
        if units <= 1 || units <= *emitted {
            return None;
        }
        let delta = units - *emitted;
        *emitted = units;
        Some(StepAction::Rest(delta))
    }

    /// Edges from a bound control or the computer keyboard. Resolve the combined
    /// action on press, so releasing the notes before the control cannot add a rest.
    pub fn control(&mut self, action: StepControl, down: bool, now_ms: u64, prefs: HoldPrefs) -> Option<StepAction> {
        match (action, down) {
            (StepControl::TieRest, true) if !self.input.held.is_empty() => {
                self.input.tie(now_ms, prefs);
                None
            }
            (StepControl::Rest | StepControl::TieRest, true) => {
                if self.input.started_ms.is_none() {
                    self.rest = Some((now_ms, 0));
                }
                None
            }
            (StepControl::Rest | StepControl::TieRest, false) => {
                let (started, emitted) = self.rest.take()?;
                let delta = prefs.units(now_ms.saturating_sub(started)).saturating_sub(emitted);
                (delta > 0).then_some(StepAction::Rest(delta))
            }
            (StepControl::Back, true) => {
                self.input.cancel();
                self.rest = None;
                self.tie_chord = false;
                Some(StepAction::Back)
            }
            (StepControl::Tie, true) => {
                self.rest = None;
                if self.input.tie(now_ms, prefs) {
                    None
                } else {
                    self.tie_chord = false;
                    Some(StepAction::Tie)
                }
            }
            _ => None,
        }
    }

    /// `value` is note velocity or a CC value; CC >=64 is down. Repeated CC values
    /// do not retrigger an action. Note/pad bindings consume both press and release.
    pub fn message(&mut self, binding: StepBinding, value: u8, now_ms: u64, prefs: HoldPrefs, bindings: StepBindings) -> Option<StepAction> {
        let down = match binding {
            StepBinding::Note(_) => value > 0,
            StepBinding::Cc(_) => value >= 64,
        };
        if binding == StepBinding::Cc(64) {
            self.pedal = down;
        }
        let action = if bindings.tie_rest == Some(binding) {
            Some(StepControl::TieRest)
        } else if bindings.rest == Some(binding) {
            Some(StepControl::Rest)
        } else if bindings.back == Some(binding) {
            Some(StepControl::Back)
        } else if bindings.tie == Some(binding) {
            Some(StepControl::Tie)
        } else {
            None
        };
        if let Some(action) = action {
            let was_down = self.pressed.contains(&binding);
            if down == was_down {
                return None;
            }
            if down {
                self.pressed.push(binding);
            } else {
                self.pressed.retain(|b| *b != binding);
            }
            return self.control(action, down, now_ms, prefs);
        }
        let StepBinding::Note(pitch) = binding else { return None };
        if down {
            let fresh = self.input.started_ms.is_none();
            if fresh {
                self.tie_chord = self.pedal;
            }
            self.rest = None;
            self.input.note(pitch, value, true, now_ms, prefs);
            if fresh && self.tie_chord {
                // Tie mode (#138): the strike is one unit however long it is held, so the hold
                // repeater cannot stack on the tie. The mode is fixed when the strike begins; a
                // pedal released mid-hold does not turn the repeater back on for this strike.
                self.input.manual_units = Some(1);
            }
            return None;
        }
        let chord = self.input.note(pitch, value, down, now_ms, prefs)?;
        Some(if std::mem::take(&mut self.tie_chord) { StepAction::TieChord(chord) } else { StepAction::Enter(chord) })
    }
}

/// Pending chord state; duplicate note-ons are counted so an early note-off cannot
/// finish a still-held unison. Cancellation discards a preview, never inserts it.
#[derive(Debug, Default)]
pub struct StepInput {
    held: std::collections::BTreeMap<u8, u32>,
    notes: std::collections::BTreeMap<u8, u8>,
    started_ms: Option<u64>,
    manual_units: Option<u32>,
}

impl StepInput {
    pub fn cancel(&mut self) {
        self.held.clear();
        self.notes.clear();
        self.started_ms = None;
        self.manual_units = None;
    }

    pub fn preview(&self, now_ms: u64, prefs: HoldPrefs) -> Option<StepChord> {
        let start = self.started_ms?;
        Some(StepChord {
            notes: self.notes.iter().map(|(&pitch, &velocity)| (pitch, velocity)).collect(),
            units: self.manual_units.unwrap_or_else(|| if prefs.enabled { prefs.units(now_ms.saturating_sub(start)) } else { 1 }),
        })
    }

    /// Add one unit to the current preview and freeze its timer until release.
    fn tie(&mut self, now_ms: u64, prefs: HoldPrefs) -> bool {
        let Some(chord) = self.preview(now_ms, prefs) else { return false };
        self.manual_units = Some(chord.units.saturating_add(1));
        true
    }

    /// Note-on velocity zero follows MIDI's note-off convention. Unknown releases
    /// and invalid pitches are ignored.
    pub fn note(&mut self, pitch: u8, velocity: u8, on: bool, now_ms: u64, prefs: HoldPrefs) -> Option<StepChord> {
        if pitch > 127 {
            return None;
        }
        if on && velocity > 0 {
            self.started_ms.get_or_insert(now_ms);
            let count = self.held.entry(pitch).or_default();
            *count = count.saturating_add(1);
            self.notes.entry(pitch).or_insert(velocity.min(127));
            return None;
        }
        let count = self.held.get_mut(&pitch)?;
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.held.remove(&pitch);
        }
        if !self.held.is_empty() {
            return None;
        }
        let chord = self.preview(now_ms, prefs);
        self.cancel();
        chord
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn held_tie_freezes_chord_length_and_empty_tie_still_extends_previous() {
        let mut input = StepController::default();
        let prefs = HoldPrefs::default();
        let mut bindings = StepBindings::default();
        bindings.learn(StepControl::Tie, StepBinding::Note(21));
        input.message(StepBinding::Note(60), 90, 0, prefs, bindings);
        input.message(StepBinding::Note(64), 80, 10, prefs, bindings);
        assert_eq!(input.message(StepBinding::Note(21), 100, 50, prefs, bindings), None);
        input.message(StepBinding::Note(21), 0, 60, prefs, bindings);
        assert_eq!(input.preview(10_000, prefs).unwrap().units, 2);
        input.message(StepBinding::Note(21), 100, 10_001, prefs, bindings);
        input.message(StepBinding::Note(21), 0, 10_002, prefs, bindings);
        input.message(StepBinding::Note(60), 0, 11_000, prefs, bindings);
        assert_eq!(
            input.message(StepBinding::Note(64), 0, 12_000, prefs, bindings),
            Some(StepAction::Enter(StepChord { notes: vec![(60, 90), (64, 80)], units: 3 }))
        );
        assert_eq!(input.message(StepBinding::Note(21), 100, 13_000, prefs, bindings), Some(StepAction::Tie));
        input.message(StepBinding::Note(62), 90, 14_000, prefs, bindings);
        assert_eq!(input.preview(14_500, prefs).unwrap().units, 2, "next entry resumes timer");
    }

    #[test]
    fn combined_control_worked_example_and_release_order() {
        let mut input = StepController::default();
        let prefs = HoldPrefs::default();
        let bindings = StepBindings::default();
        let pedal = StepBinding::Cc(64);
        let mut lengths = Vec::new();
        for (pitch, start) in [(60, 0), (62, 3000)] {
            input.message(StepBinding::Note(pitch), 90, start, prefs, bindings);
            input.message(pedal, 127, start + 10, prefs, bindings);
            assert_eq!(input.message(pedal, 127, start + 20, prefs, bindings), None);
            let Some(StepAction::Enter(chord)) = input.message(StepBinding::Note(pitch), 0, start + 2000, prefs, bindings) else {
                panic!("expected entry")
            };
            lengths.push(chord.units);
            assert_eq!(input.message(pedal, 0, start + 2100, prefs, bindings), None, "release cannot create a rest");
        }
        input.message(pedal, 127, 6000, prefs, bindings);
        assert_eq!(input.message(pedal, 0, 6100, prefs, bindings), Some(StepAction::Rest(1)));
        for (pitch, start) in [(64, 6200), (65, 6400), (67, 6600), (69, 6800)] {
            input.message(StepBinding::Note(pitch), 90, start, prefs, bindings);
            let Some(StepAction::Enter(chord)) = input.message(StepBinding::Note(pitch), 0, start + 100, prefs, bindings) else {
                panic!("expected entry")
            };
            lengths.push(chord.units);
        }
        assert_eq!(lengths, [2, 2, 1, 1, 1, 1]);
    }

    #[test]
    fn disabled_hold_keeps_note_lengths_manual_but_rest_timer_is_unchanged() {
        let mut input = StepController::default();
        let prefs = HoldPrefs { enabled: false, ..HoldPrefs::default() };
        let bindings = StepBindings::default();
        input.message(StepBinding::Note(60), 90, 0, prefs, bindings);
        assert_eq!(input.preview(10_000, prefs).unwrap().units, 1);
        input.control(StepControl::Tie, true, 10_001, prefs);
        assert_eq!(input.preview(20_000, prefs).unwrap().units, 2);
        input.message(StepBinding::Note(60), 0, 20_001, prefs, bindings);
        input.control(StepControl::Rest, true, 21_000, prefs);
        assert_eq!(input.poll(21_500, prefs), Some(StepAction::Rest(2)));
        assert_eq!(input.control(StepControl::Rest, false, 21_900, prefs), Some(StepAction::Rest(1)));
        input.cancel();
        input.message(StepBinding::Note(62), 90, 22_000, prefs, bindings);
        assert_eq!(input.preview(u64::MAX, prefs).unwrap().units, 1);
    }

    #[test]
    fn hold_threshold_and_accelerating_repeat_schedule() {
        let prefs = HoldPrefs::default();
        // Intervals after unit 2 (500 ms): 400, 367, 334, 300, 267, 234, then 200 each.
        // Units begin at 500, 900, 1267, 1601, 1901, 2168, 2402, then every 200 ms.
        for (ms, units) in [
            (0, 1),
            (499, 1),
            (500, 2),
            (899, 2),
            (900, 3),
            (1266, 3),
            (1267, 4),
            (1600, 4),
            (1601, 5),
            (1900, 5),
            (1901, 6),
            (2167, 6),
            (2168, 7),
            (2401, 7),
            (2402, 8),
            (2601, 8),
            (2602, 9),
            (2802, 10),
            (4402, 18),
        ] {
            assert_eq!(prefs.units(ms), units, "at {ms} ms");
        }
        assert_eq!(HoldPrefs { threshold_ms: 200, floor_ms: 100, ..HoldPrefs::default() }.units(199), 1);
        assert_eq!(HoldPrefs { threshold_ms: 200, floor_ms: 100, ..HoldPrefs::default() }.units(200), 2);
        assert_eq!(prefs.units(u64::MAX), u32::MAX);
    }

    #[test]
    fn ramp_is_monotone_and_never_shorter_than_the_floor() {
        for threshold_ms in [0, 1, 50, 200, 500, 5000, 100_000] {
            for floor_ms in [0, 1, 80, 200, 400, 1000, 70_000] {
                for ramp_units in [0, 1, 2, 6, 20, 64, 1000, u32::MAX] {
                    let prefs = HoldPrefs { enabled: true, threshold_ms, floor_ms, ramp_units };
                    let threshold = threshold_ms.clamp(1, 60_000);
                    let floor = floor_ms.clamp(1, threshold);
                    let mut last = 1;
                    let mut last_change = threshold;
                    for ms in (0..40_000u64).step_by(7) {
                        let units = prefs.units(ms);
                        assert!(units >= last, "units fell at {ms} ms for {prefs:?}");
                        if units != last && ms >= threshold {
                            // Units advance at most one per `floor` ms, so no interval is shorter.
                            let gap = ms - last_change;
                            if units == last + 1 && last > 2 {
                                assert!(gap + 7 > floor, "interval {gap} under floor {floor} for {prefs:?}");
                            }
                            last_change = ms;
                        }
                        last = units;
                    }
                    assert!(u64::from(prefs.units(threshold + 1000 * floor)) <= 2 + 64 + 1000, "{prefs:?}");
                }
            }
        }
    }

    #[test]
    fn old_prefs_without_ramp_units_load_with_the_default() {
        let old: HoldPrefs = serde_json::from_str(r#"{"enabled":true,"threshold_ms":500,"floor_ms":80}"#).unwrap();
        assert_eq!(old.ramp_units, 6);
        assert_eq!(old.floor_ms, 80, "a stored floor is the user's choice");
        let empty: HoldPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!((empty.threshold_ms, empty.floor_ms, empty.ramp_units), (500, 200, 6));
    }

    #[test]
    fn chord_commits_on_last_release_with_one_shared_duration() {
        let mut input = StepInput::default();
        let prefs = HoldPrefs::default();
        for (pitch, ms) in [(60, 0), (64, 20), (67, 40)] {
            assert!(input.note(pitch, 90, true, ms, prefs).is_none());
        }
        assert!(input.note(60, 0, false, 200, prefs).is_none());
        assert!(input.note(64, 0, false, 600, prefs).is_none());
        let chord = input.note(67, 0, false, 1050, prefs).unwrap();
        assert_eq!(chord.notes, vec![(60, 90), (64, 90), (67, 90)]);
        assert_eq!(chord.units, 3);
        assert!(input.preview(1100, prefs).is_none());
    }

    #[test]
    fn quick_entry_unmatched_releases_and_cancellation() {
        let mut input = StepInput::default();
        let prefs = HoldPrefs::default();
        input.note(60, 70, true, 0, prefs);
        assert!(input.note(62, 0, false, 100, prefs).is_none());
        assert_eq!(input.note(60, 0, true, 100, prefs).unwrap().units, 1);
        input.note(60, 70, true, 200, prefs);
        input.cancel();
        assert!(input.note(60, 0, false, 2000, prefs).is_none());
    }

    #[test]
    fn pedal_tap_and_hold_repeat_but_duplicate_cc_does_not_retrigger() {
        let mut input = StepController::default();
        let prefs = HoldPrefs::default();
        let bindings = StepBindings::default();
        let pedal = StepBinding::Cc(64);
        assert_eq!(input.message(pedal, 127, 0, prefs, bindings), None);
        assert_eq!(input.message(pedal, 0, 100, prefs, bindings), Some(StepAction::Rest(1)));
        input.message(pedal, 127, 200, prefs, bindings);
        assert_eq!(input.poll(699, prefs), None);
        assert_eq!(input.poll(700, prefs), Some(StepAction::Rest(2)));
        assert_eq!(input.message(pedal, 127, 800, prefs, bindings), None);
        assert_eq!(input.poll(1100, prefs), Some(StepAction::Rest(1)));
        assert_eq!(input.message(pedal, 0, 1101, prefs, bindings), None);
        assert!(!input.is_pending());
    }

    fn pedal_setup() -> (StepController, HoldPrefs, StepBindings, StepBinding) {
        (StepController::default(), HoldPrefs::default(), StepBindings::default(), StepBinding::Cc(64))
    }

    #[test]
    fn pedal_down_strikes_each_tie_one_unit_however_long_they_are_held() {
        let (mut input, prefs, bindings, pedal) = pedal_setup();
        input.message(pedal, 127, 0, prefs, bindings);
        let n = StepBinding::Note(60);
        let mut total = 0;
        for i in 0..3u64 {
            let start = 100 + i * 5000;
            input.message(n, 90, start, prefs, bindings);
            assert_eq!(input.poll(start + 4000, prefs), None, "no repeater while the pedal is down");
            assert_eq!(input.preview(start + 4000, prefs).unwrap().units, 1);
            let Some(StepAction::TieChord(chord)) = input.message(n, 0, start + 4500, prefs, bindings) else { panic!("expected a tie") };
            total += chord.units;
        }
        assert_eq!(total, 3, "three strikes, three units");
    }

    #[test]
    fn pedal_up_strike_follows_the_ramp_and_a_pedal_tap_alone_is_a_rest() {
        let (mut input, prefs, bindings, pedal) = pedal_setup();
        let n = StepBinding::Note(60);
        input.message(n, 90, 0, prefs, bindings);
        let Some(StepAction::Enter(chord)) = input.message(n, 0, 1000, prefs, bindings) else { panic!("expected entry") };
        assert_eq!(chord.units, prefs.units(1000));
        assert!(chord.units > 2);
        input.message(pedal, 127, 2000, prefs, bindings);
        assert_eq!(input.message(pedal, 0, 2050, prefs, bindings), Some(StepAction::Rest(1)));
    }

    #[test]
    fn pedal_released_mid_hold_keeps_the_strike_at_one_unit() {
        let (mut input, prefs, bindings, pedal) = pedal_setup();
        let n = StepBinding::Note(60);
        input.message(pedal, 127, 0, prefs, bindings);
        input.message(n, 90, 100, prefs, bindings);
        assert_eq!(input.message(pedal, 0, 200, prefs, bindings), None, "no rest, no extra unit");
        let Some(StepAction::TieChord(chord)) = input.message(n, 0, 5000, prefs, bindings) else { panic!("expected a tie") };
        assert_eq!(chord.units, 1);
        input.message(n, 90, 6000, prefs, bindings);
        let Some(StepAction::Enter(next)) = input.message(n, 0, 7000, prefs, bindings) else { panic!("expected entry") };
        assert_eq!(next.units, prefs.units(1000), "the next strike lengthens normally");
    }

    #[test]
    fn a_pedal_tap_of_any_length_with_no_key_is_one_rest_on_release() {
        let (mut input, prefs, bindings, pedal) = pedal_setup();
        assert_eq!(input.message(pedal, 127, 5, prefs, bindings), None, "the press waits: a key may still arrive");
        assert_eq!(input.message(pedal, 0, 6, prefs, bindings), Some(StepAction::Rest(1)));
        assert_eq!(input.message(pedal, 127, 7, prefs, bindings), None);
        assert_eq!(input.message(pedal, 0, 7, prefs, bindings), Some(StepAction::Rest(1)), "same millisecond");
        assert_eq!(input.message(pedal, 127, 8, prefs, bindings), None);
        assert_eq!(input.message(pedal, 0, 9, prefs, bindings), Some(StepAction::Rest(1)), "two fast taps, two rests");
        assert!(!input.is_pending());
    }

    #[test]
    fn pedal_down_key_struck_pedal_up_is_a_tie_and_no_rest() {
        let (mut input, prefs, bindings, pedal) = pedal_setup();
        let n = StepBinding::Note(60);
        input.message(pedal, 127, 0, prefs, bindings);
        input.message(n, 90, 1, prefs, bindings);
        assert_eq!(input.message(pedal, 0, 2, prefs, bindings), None);
        assert!(matches!(input.message(n, 0, 3, prefs, bindings), Some(StepAction::TieChord(_))));
        assert_eq!(input.poll(10_000, prefs), None);
    }

    #[test]
    fn pedal_chord_ties_without_an_extra_rest_even_if_pedal_releases_first() {
        let mut input = StepController::default();
        let prefs = HoldPrefs::default();
        let bindings = StepBindings::default();
        input.message(StepBinding::Cc(64), 127, 0, prefs, bindings);
        input.message(StepBinding::Note(60), 90, 50, prefs, bindings);
        assert_eq!(input.poll(600, prefs), None);
        assert_eq!(input.message(StepBinding::Cc(64), 0, 650, prefs, bindings), None);
        assert_eq!(
            input.message(StepBinding::Note(60), 0, 700, prefs, bindings),
            Some(StepAction::TieChord(StepChord { notes: vec![(60, 90)], units: 1 }))
        );
    }

    #[test]
    fn learned_back_and_tie_keys_are_actions_not_entered_notes() {
        let mut input = StepController::default();
        let prefs = HoldPrefs::default();
        let mut bindings = StepBindings::default();
        bindings.learn(StepControl::Back, StepBinding::Note(21));
        bindings.learn(StepControl::Tie, StepBinding::Note(22));
        for (pitch, expected) in [(21, StepAction::Back), (22, StepAction::Tie)] {
            assert_eq!(input.message(StepBinding::Note(pitch), 100, 0, prefs, bindings), Some(expected));
            assert_eq!(input.message(StepBinding::Note(pitch), 0, 100, prefs, bindings), None);
            assert_eq!(input.preview(100, prefs), None);
        }
        bindings.learn(StepControl::Tie, StepBinding::Note(21));
        assert_eq!(bindings.back, None);
        assert_eq!(bindings.tie, Some(StepBinding::Note(21)));
    }
}
