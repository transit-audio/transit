//! The per-plug-in editor Scale setting (issue #153) and the decision behind the editor window's
//! Scale menu: given what the editor answered to the host's scale call, the user's setting and the
//! display's scale, which factor is sent to the plug-in, whether Windows stretches its drawing,
//! which mode is in use, and which menu steps are enabled (with the reason for the others).
//!
//! The rule that holds for every input: **no factor is ever applied twice.** A factor sent to the
//! plug-in (VST3 `IPlugViewContentScaleSupport::setContentScaleFactor`, CLAP `set_scale`) and the
//! Windows stretch (a DPI-unaware content container) are never both in use ([`ScalePlan::factor`]
//! and [`ScalePlan::stretch`] are never both set), and the plug-in's own zoom gets neither.
//!
//! "Accepted" is not "applied" (issue #153, the third kind of editor): an editor that accepts the
//! host's factor and keeps drawing at its own size cannot be told apart by the call's answer, so
//! Auto picks the host factor for it and the menu lets the user pick the plug-in's own zoom or the
//! Windows stretch instead. A hint line says so when the editor accepted a factor other than 1 and
//! reported the same size afterwards ([`ScaleInputs::kept_size`]).
//!
//! The setting is per machine, keyed by plug-in id, and never in the session (the app keeps it
//! with its other per-machine preferences).

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The percentages the Scale menu offers besides Auto.
pub const PERCENTS: [u16; 6] = [50, 75, 100, 125, 150, 200];

/// The size step of the setting: Auto (the display's scale) or a percentage of the plug-in's
/// size at 100 percent. Saved as `"auto"` or the number; anything else does not load (the app's
/// settings loader then keeps that field at its default and says so in the log, as for every
/// other preference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleStep {
    /// The display's scale.
    #[default]
    Auto,
    /// One of [`PERCENTS`].
    Percent(u16),
}

impl ScaleStep {
    /// The factor this step asks for on a display at `display`.
    #[must_use]
    pub fn factor(self, display: f64) -> f64 {
        match self {
            ScaleStep::Auto => display,
            ScaleStep::Percent(p) => f64::from(p) / 100.0,
        }
    }

    /// What the menu reads.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            ScaleStep::Auto => "Auto".into(),
            ScaleStep::Percent(p) => format!("{p}%"),
        }
    }
}

impl Serialize for ScaleStep {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ScaleStep::Auto => s.serialize_str("auto"),
            ScaleStep::Percent(p) => s.serialize_u16(*p),
        }
    }
}

impl<'de> Deserialize<'de> for ScaleStep {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(u64),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Number(n) => u16::try_from(n)
                .ok()
                .filter(|p| PERCENTS.contains(p))
                .map(ScaleStep::Percent)
                .ok_or_else(|| serde::de::Error::custom(format!("scale step {n} is not one of {PERCENTS:?}"))),
            Raw::Text(t) if t == "auto" => Ok(ScaleStep::Auto),
            Raw::Text(t) => Err(serde::de::Error::custom(format!("scale step \"{t}\" is not \"auto\" or a percentage"))),
        }
    }
}

/// How the editor is brought to its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleMode {
    /// The plug-in draws itself at the factor the host sends.
    HostFactor,
    /// The plug-in's own zoom: no factor sent, no stretch.
    PluginZoom,
    /// Windows stretches the plug-in's 100 percent drawing to the display's scale (no factor
    /// sent). Windows only.
    WindowsStretch,
}

impl ScaleMode {
    /// What the menu's mode entry reads.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ScaleMode::HostFactor => "Drawn by the plug-in at the host's factor",
            ScaleMode::PluginZoom => "The plug-in's own zoom (no stretch)",
            ScaleMode::WindowsStretch => "Stretched by Windows",
        }
    }
}

/// One plug-in's setting. The default (Auto, automatic mode) is not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorScale {
    /// The size step.
    pub step: ScaleStep,
    /// The mode the user chose; `None`: automatic ([`ScalePlan`] picks it).
    pub mode: Option<ScaleMode>,
}

impl EditorScale {
    /// The default: Auto, automatic mode.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == EditorScale::default()
    }
}

/// What the editor answered to the host's scale call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleSupport {
    /// It took the factor (VST3 `kResultTrue`, CLAP `set_scale` true). Not proof that it drew at it.
    Accepts,
    /// It has no such call, or it answered no (VST3 no interface or not `kResultTrue`, CLAP no
    /// `set_scale` or false). Nothing was applied.
    Refuses,
    /// It has the call but was not asked (the user's mode sends no factor). Treated as able to
    /// take one: asking is the only way to know.
    Unasked,
}

/// The platform the editor runs on (the menu says what applies there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Host factor, plug-in zoom and the Windows stretch.
    Windows,
    /// The system scales editors itself: nothing to choose.
    MacOs,
    /// Only editors that accept a factor can be scaled.
    Linux,
}

impl Platform {
    /// The platform this build runs on (other targets behave as Linux: no stretch).
    #[must_use]
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

/// What the decision depends on besides the setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleInputs {
    /// What the editor answered to the scale call.
    pub support: ScaleSupport,
    /// The display's scale where the editor window is (its DPI over 96).
    pub display: f64,
    /// The platform.
    pub platform: Platform,
    /// The editor accepted a factor other than 1 and reported the same size afterwards (a hint
    /// that it ignored it; the decision does not change).
    pub kept_size: bool,
    /// Windows can host the DPI-unaware content container (Windows 10 version 1803 or later;
    /// `transit_rt::editor_window::stretch_available`). Ignored on other platforms.
    pub stretch_available: bool,
}

/// What [`decide`] settled on.
#[derive(Debug, Clone, PartialEq)]
pub struct ScalePlan {
    /// The factor sent to the plug-in; `None`: no scale call at all.
    pub factor: Option<f64>,
    /// Windows stretches the plug-in's drawing by this factor (the display's scale; a DPI-unaware
    /// content container). Never set together with `factor`.
    pub stretch: Option<f64>,
    /// The mode in use (with no factor and no stretch it is [`ScaleMode::PluginZoom`]).
    pub mode: ScaleMode,
    /// The step in effect (a stored step the mode cannot offer acts as Auto).
    pub step: ScaleStep,
    /// The mode the user chose that cannot be used for this editor here (it acts as automatic).
    pub mode_unavailable: bool,
    /// Which steps are enabled, in menu order (Auto first, then [`PERCENTS`]).
    pub steps: Vec<(ScaleStep, bool)>,
    /// Which modes are enabled, with the reason for a disabled one.
    pub modes: Vec<(ScaleMode, Result<(), &'static str>)>,
    /// Why the disabled steps are disabled.
    pub reason: Option<String>,
    /// One line on how the editor is drawn now.
    pub in_use: String,
    /// A hint (the editor may have ignored the factor it accepted).
    pub hint: Option<String>,
}

/// The display's scale as a whole percentage.
#[must_use]
pub fn percent_of(scale: f64) -> u16 {
    if scale.is_finite() && scale > 0.0 {
        // Clamped to the u16 range first: the cast below cannot truncate.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let p = (scale * 100.0).round().clamp(1.0, 1_000.0) as u16;
        p
    } else {
        100
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.005
}

fn all_steps() -> impl Iterator<Item = ScaleStep> {
    std::iter::once(ScaleStep::Auto).chain(PERCENTS.iter().map(|&p| ScaleStep::Percent(p)))
}

/// The decision (see the module docs). Pure; the format hosts call it on the editor's thread and
/// the tests through the fake editors.
#[must_use]
pub fn decide(setting: EditorScale, inputs: ScaleInputs) -> ScalePlan {
    let display = if inputs.display.is_finite() && inputs.display > 0.0 { inputs.display } else { 1.0 };
    let shown = percent_of(display);
    let can_factor = inputs.support != ScaleSupport::Refuses;
    let windows = inputs.platform == Platform::Windows && inputs.stretch_available;
    if inputs.platform == Platform::MacOs {
        return ScalePlan {
            factor: None,
            stretch: None,
            mode: ScaleMode::PluginZoom,
            step: ScaleStep::Auto,
            mode_unavailable: setting.mode.is_some(),
            steps: all_steps().map(|s| (s, false)).collect(),
            modes: [ScaleMode::HostFactor, ScaleMode::PluginZoom, ScaleMode::WindowsStretch]
                .into_iter()
                .map(|m| (m, Err("macOS scales plug-in editors itself")))
                .collect(),
            reason: Some("macOS scales plug-in editors itself: transit does not change their size".into()),
            in_use: "In use: scaled by macOS".into(),
            hint: None,
        };
    }
    let factor_ok: Result<(), &'static str> = if can_factor { Ok(()) } else { Err("the plug-in does not take a scale from the host") };
    let stretch_ok: Result<(), &'static str> = if windows {
        Ok(())
    } else if inputs.platform == Platform::Windows {
        Err("needs Windows 10 version 1803 or later")
    } else {
        Err("Windows only")
    };
    let modes = vec![(ScaleMode::HostFactor, factor_ok), (ScaleMode::PluginZoom, Ok(())), (ScaleMode::WindowsStretch, stretch_ok)];
    let automatic = if can_factor {
        ScaleMode::HostFactor
    } else if windows {
        ScaleMode::WindowsStretch
    } else {
        ScaleMode::PluginZoom
    };
    let chosen = setting.mode.filter(|m| modes.iter().any(|(x, ok)| x == m && ok.is_ok()));
    let mode_unavailable = setting.mode.is_some() && chosen.is_none();
    let mode = chosen.unwrap_or(automatic);
    match mode {
        ScaleMode::HostFactor => {
            let factor = setting.step.factor(display);
            let hint = (inputs.kept_size && inputs.support == ScaleSupport::Accepts && !near(factor, 1.0)).then(|| {
                let other = if windows { "Stretched by Windows or the plug-in's own zoom" } else { "the plug-in's own zoom" };
                format!("The plug-in accepted the factor but kept its size: if it draws too small, choose {other}")
            });
            ScalePlan {
                factor: Some(factor),
                stretch: None,
                mode,
                step: setting.step,
                mode_unavailable,
                steps: all_steps().map(|s| (s, true)).collect(),
                modes,
                reason: None,
                in_use: format!("In use: drawn by the plug-in at {}%", percent_of(factor)),
                hint,
            }
        }
        ScaleMode::WindowsStretch => {
            // Windows stretches a DPI-unaware window by the display's scale and by nothing else, so
            // the sizes on offer are the plug-in's own (100 percent) and the display's.
            let offered = |s: ScaleStep| match s {
                ScaleStep::Auto => true,
                ScaleStep::Percent(p) => p == 100 || p == shown,
            };
            let step = if offered(setting.step) { setting.step } else { ScaleStep::Auto };
            let stretched = !near(display, 1.0) && step != ScaleStep::Percent(100);
            let reason = format!(
                "This plug-in is not drawn at the host's factor here: 100% (its own drawing) or the display's {shown}% (stretched by Windows)"
            );
            ScalePlan {
                factor: None,
                stretch: stretched.then_some(display),
                mode: if stretched { ScaleMode::WindowsStretch } else { ScaleMode::PluginZoom },
                step,
                mode_unavailable,
                steps: all_steps().map(|s| (s, offered(s))).collect(),
                modes,
                reason: Some(reason),
                in_use: if stretched {
                    format!("In use: stretched by Windows to {shown}%")
                } else {
                    "In use: the plug-in's own drawing (100%)".into()
                },
                hint: None,
            }
        }
        ScaleMode::PluginZoom => {
            let reason = if !can_factor && inputs.platform == Platform::Linux {
                "On Linux only editors that take a scale from the host can be scaled; this one does not".to_owned()
            } else if !can_factor && !windows {
                "This plug-in does not take a scale from the host, and this Windows cannot stretch it".to_owned()
            } else {
                "The plug-in's own zoom sets the size: use its zoom control".to_owned()
            };
            ScalePlan {
                factor: None,
                stretch: None,
                mode,
                step: ScaleStep::Auto,
                mode_unavailable,
                steps: all_steps().map(|s| (s, s == ScaleStep::Auto)).collect(),
                modes,
                reason: Some(reason),
                in_use: "In use: the plug-in's own zoom".into(),
                hint: None,
            }
        }
    }
}

/// When a change of plan reaches an open editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applies {
    /// Nothing the editor sees changes.
    Unchanged,
    /// At once: a new factor sent to an editor that can take one.
    Now,
    /// The next time the editor opens: a stretch turned on or off (the content container's DPI
    /// awareness is fixed when the plug-in's window is made), or a factor withdrawn (a factor the
    /// plug-in took cannot be taken back).
    NextOpen,
}

/// Whether the open editor drawn per `in_use` can move to `wanted` at once (see [`Applies`]).
#[must_use]
pub fn applies(in_use: &ScalePlan, wanted: &ScalePlan) -> Applies {
    let same_factor = match (in_use.factor, wanted.factor) {
        (Some(a), Some(b)) => near(a, b),
        (None, None) => true,
        _ => false,
    };
    let same_stretch = in_use.stretch.is_some() == wanted.stretch.is_some();
    if same_factor && same_stretch {
        Applies::Unchanged
    } else if same_stretch && wanted.factor.is_some() {
        Applies::Now
    } else {
        Applies::NextOpen
    }
}

/// An entry of the Scale submenu.
#[derive(Debug, Clone, PartialEq)]
pub enum ScaleEntry {
    /// A line of text (not selectable).
    Info(String),
    /// A separator.
    Separator,
    /// A choice: picking it stores `value` as the plug-in's setting.
    Choice {
        /// What it reads.
        label: String,
        /// The setting picking it stores.
        value: EditorScale,
        /// Shown checked.
        checked: bool,
        /// Selectable.
        enabled: bool,
    },
}

/// The Scale submenu for `setting`: the mode in use (and what applies on the next open when the
/// open editor cannot change at once), the steps, the modes, the reason for disabled steps and the
/// hint. `in_use` is the open editor's plan (`None`: the plan for `setting` is the one in use).
#[must_use]
pub fn menu(setting: EditorScale, inputs: ScaleInputs, in_use: Option<&ScalePlan>) -> Vec<ScaleEntry> {
    let wanted = decide(setting, inputs);
    let now = in_use.unwrap_or(&wanted);
    let mut out = vec![ScaleEntry::Info(now.in_use.clone())];
    if let Some(p) = in_use
        && applies(p, &wanted) == Applies::NextOpen
    {
        out.push(ScaleEntry::Info(format!("Next open: {}", wanted.in_use.trim_start_matches("In use: "))));
    }
    out.push(ScaleEntry::Separator);
    for (step, enabled) in &wanted.steps {
        let label = match step {
            ScaleStep::Auto => format!("Auto ({}%)", percent_of(inputs.display)),
            s => s.label(),
        };
        out.push(ScaleEntry::Choice { label, value: EditorScale { step: *step, ..setting }, checked: *step == wanted.step, enabled: *enabled });
    }
    out.push(ScaleEntry::Separator);
    let auto_mode = decide(EditorScale { mode: None, ..setting }, inputs).mode;
    out.push(ScaleEntry::Choice {
        label: format!("Automatic: {}", auto_mode.label()),
        value: EditorScale { mode: None, ..setting },
        checked: setting.mode.is_none() || wanted.mode_unavailable,
        enabled: inputs.platform != Platform::MacOs,
    });
    for (mode, ok) in &wanted.modes {
        let label = match ok {
            Ok(()) => mode.label().to_owned(),
            Err(why) => format!("{} ({why})", mode.label()),
        };
        out.push(ScaleEntry::Choice {
            label,
            value: EditorScale { mode: Some(*mode), ..setting },
            checked: setting.mode == Some(*mode) && !wanted.mode_unavailable,
            enabled: ok.is_ok(),
        });
    }
    if let Some(r) = &wanted.reason {
        out.push(ScaleEntry::Separator);
        out.push(ScaleEntry::Info(r.clone()));
    }
    if let Some(h) = &now.hint {
        out.push(ScaleEntry::Info(h.clone()));
    }
    out
}

/// One editor's scale state on its own thread: the format hosts keep one per editor and drive it
/// through an open (`before_open`, then `opened` with the editor's answer), a setting change while
/// open (`set_setting`), a DPI change (`dpi_changed`) and the menu (`menu`, `pick`).
#[derive(Debug)]
pub struct ScaleState {
    /// The user's setting (from the app's header).
    pub setting: EditorScale,
    /// The editor's last answer to the scale call (kept across opens); `None`: not asked yet.
    pub support: Option<ScaleSupport>,
    /// The last accepted factor left the editor's size as it was.
    pub kept_size: bool,
    /// The display's scale at the window.
    pub display: f64,
    /// The plan of the open editor; `None` while it is closed.
    pub in_use: Option<ScalePlan>,
    /// The Scale submenu last built ([`ScaleState::menu`]); a click's index maps into it.
    pub entries: Vec<ScaleEntry>,
    /// Windows can stretch here (see [`ScaleInputs::stretch_available`]).
    pub stretch_available: bool,
    /// The platform the editor runs on ([`Platform::current`]; tests set another).
    pub platform: Platform,
}

impl Default for ScaleState {
    fn default() -> Self {
        ScaleState {
            setting: EditorScale::default(),
            support: None,
            kept_size: false,
            display: 1.0,
            in_use: None,
            entries: Vec::new(),
            stretch_available: false,
            platform: Platform::current(),
        }
    }
}

impl ScaleState {
    /// The decision's inputs now.
    #[must_use]
    pub fn inputs(&self) -> ScaleInputs {
        ScaleInputs {
            support: self.support.unwrap_or(ScaleSupport::Unasked),
            display: self.display,
            platform: self.platform,
            kept_size: self.kept_size,
            stretch_available: self.stretch_available,
        }
    }

    /// The plan an open starts from, before the editor is asked: `has_call` says whether it has a
    /// scale call at all (VST3 `IPlugViewContentScaleSupport`, CLAP `set_scale`); without one it
    /// refuses. Send its factor (if any) and report the answer to [`ScaleState::opened`].
    pub fn before_open(&mut self, display: f64, has_call: bool) -> ScalePlan {
        self.display = display;
        if !has_call {
            self.support = Some(ScaleSupport::Refuses);
        }
        decide(self.setting, self.inputs())
    }

    /// The open's plan, from the editor's answer to the factor `before_open` asked for (`None`:
    /// it was not asked); the editor is in use with it from now on.
    pub fn opened(&mut self, answer: Option<(bool, bool)>) -> ScalePlan {
        if let Some((accepted, kept_size)) = answer {
            self.support = Some(if accepted { ScaleSupport::Accepts } else { ScaleSupport::Refuses });
            self.kept_size = accepted && kept_size;
        }
        let plan = decide(self.setting, self.inputs());
        self.in_use = Some(plan.clone());
        plan
    }

    /// The editor could not be stretched after all (the window failed): it is in use as drawn.
    pub fn stretch_failed(&mut self) {
        if let Some(p) = &mut self.in_use {
            p.stretch = None;
            p.mode = ScaleMode::PluginZoom;
            p.in_use = "In use: the plug-in's own drawing (100%; the stretch failed)".into();
        }
    }

    /// The editor closed.
    pub fn closed(&mut self) {
        self.in_use = None;
    }

    /// A new setting. While the editor is open: [`Applies::Now`] means send the plan's factor now
    /// (the caller then calls [`ScaleState::applied`]); [`Applies::NextOpen`] leaves the editor as
    /// it is. While it is closed: [`Applies::NextOpen`] (or `Unchanged`).
    pub fn set_setting(&mut self, setting: EditorScale) -> (Applies, ScalePlan) {
        let changed = setting != self.setting;
        self.setting = setting;
        let wanted = decide(setting, self.inputs());
        let applies = match &self.in_use {
            _ if !changed => Applies::Unchanged,
            Some(now) => applies(now, &wanted),
            None => Applies::NextOpen,
        };
        (applies, wanted)
    }

    /// The editor took `plan`'s factor at once (after [`Applies::Now`]); `accepted` is its answer.
    pub fn applied(&mut self, accepted: bool, kept_size: bool) {
        self.support = Some(if accepted { ScaleSupport::Accepts } else { ScaleSupport::Refuses });
        self.kept_size = accepted && kept_size;
        let plan = decide(self.setting, self.inputs());
        if self.in_use.is_some() {
            self.in_use = Some(plan);
        }
    }

    /// The window's DPI changed to `display`: the factor to send now (only when the editor is
    /// drawn at the host's factor under Auto; a chosen percentage stays, and Windows re-stretches
    /// a stretched editor by itself).
    pub fn dpi_changed(&mut self, display: f64) -> Option<f64> {
        self.display = display;
        let now = self.in_use.as_ref()?;
        let send = (now.factor.is_some() && self.setting.step == ScaleStep::Auto && now.mode == ScaleMode::HostFactor).then_some(display);
        let mut plan = decide(self.setting, self.inputs());
        if now.factor.is_none() {
            // Nothing is sent: the editor keeps what it was opened with, at the new display.
            plan = ScalePlan { factor: None, ..now.clone() };
            plan.stretch = now.stretch.map(|_| display);
        }
        self.in_use = Some(plan);
        send
    }

    /// Builds the Scale submenu (kept for [`ScaleState::pick`]).
    pub fn menu(&mut self) -> &[ScaleEntry] {
        self.entries = menu(self.setting, self.inputs(), self.in_use.as_ref());
        &self.entries
    }

    /// The setting the entry at `index` of the last menu stores (`None`: not an enabled choice).
    #[must_use]
    pub fn pick(&self, index: usize) -> Option<EditorScale> {
        match self.entries.get(index)? {
            ScaleEntry::Choice { value, enabled: true, .. } => Some(*value),
            _ => None,
        }
    }

    /// The info log line for an open editor (issue #153): whether it accepts a factor, the factor
    /// sent, the mode in use.
    #[must_use]
    pub fn log_line(&self) -> String {
        let accepts = match self.support {
            Some(ScaleSupport::Accepts) => "yes",
            Some(ScaleSupport::Refuses) => "no",
            Some(ScaleSupport::Unasked) | None => "not asked",
        };
        let Some(p) = &self.in_use else { return format!("accepts a factor: {accepts}; closed") };
        let sent = p.factor.map_or_else(|| "none".to_owned(), |f| f.to_string());
        let stretch = p.stretch.map_or_else(|| "none".to_owned(), |s| s.to_string());
        format!(
            "accepts a factor: {accepts}; factor in use: {sent}; Windows stretch: {stretch}; in use: {}; setting {:?}, display {}",
            p.in_use.trim_start_matches("In use: "),
            self.setting,
            self.display
        )
    }
}

#[cfg(test)]
#[path = "editor_scale_tests.rs"]
mod tests;
