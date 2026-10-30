//! Per-track mixer state: inserts, sends, routing, fader, pan, mute/solo.

use crate::{AutoParam, AutomationMode, BusId, SourceId, TrackId};
use std::collections::BTreeMap;

pub const INSERT_SLOTS: usize = 10;
pub const SEND_SLOTS: usize = 10;

/// Channel formats (track widths).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum ChannelFormat {
    #[default]
    Mono,
    Stereo,
    Lcr,
    Quad,
    Lcrs,
    Surround50,
    Surround51,
    Surround60,
    Surround61,
    Sdds70,
    Sdds71,
    Surround70,
    Surround71,
    Atmos702,
    Atmos712,
    Atmos502,
    Atmos512,
    Atmos504,
    Atmos514,
    Atmos704,
    Atmos714,
    Atmos706,
    Atmos716,
    Atmos904,
    Atmos914,
    Atmos906,
    Atmos916,
    /// Ambisonics of order 1..=7 ((n+1)² channels).
    Ambisonics(u8),
}

impl ChannelFormat {
    pub const ALL: [ChannelFormat; 34] = [
        ChannelFormat::Mono,
        ChannelFormat::Stereo,
        ChannelFormat::Lcr,
        ChannelFormat::Quad,
        ChannelFormat::Lcrs,
        ChannelFormat::Surround50,
        ChannelFormat::Surround51,
        ChannelFormat::Surround60,
        ChannelFormat::Surround61,
        ChannelFormat::Sdds70,
        ChannelFormat::Sdds71,
        ChannelFormat::Surround70,
        ChannelFormat::Surround71,
        ChannelFormat::Atmos702,
        ChannelFormat::Atmos712,
        ChannelFormat::Ambisonics(1),
        ChannelFormat::Ambisonics(2),
        ChannelFormat::Ambisonics(3),
        ChannelFormat::Ambisonics(4),
        ChannelFormat::Ambisonics(5),
        ChannelFormat::Ambisonics(6),
        ChannelFormat::Ambisonics(7),
        ChannelFormat::Atmos502,
        ChannelFormat::Atmos512,
        ChannelFormat::Atmos504,
        ChannelFormat::Atmos514,
        ChannelFormat::Atmos704,
        ChannelFormat::Atmos714,
        ChannelFormat::Atmos706,
        ChannelFormat::Atmos716,
        ChannelFormat::Atmos904,
        ChannelFormat::Atmos914,
        ChannelFormat::Atmos906,
        ChannelFormat::Atmos916,
    ];
    pub fn channels(self) -> usize {
        match self {
            ChannelFormat::Mono => 1,
            ChannelFormat::Stereo => 2,
            ChannelFormat::Lcr => 3,
            ChannelFormat::Quad | ChannelFormat::Lcrs => 4,
            ChannelFormat::Surround50 => 5,
            ChannelFormat::Surround51 | ChannelFormat::Surround60 => 6,
            ChannelFormat::Surround61 | ChannelFormat::Sdds70 | ChannelFormat::Surround70 | ChannelFormat::Atmos502 => 7,
            ChannelFormat::Sdds71 | ChannelFormat::Surround71 | ChannelFormat::Atmos512 => 8,
            ChannelFormat::Atmos702 | ChannelFormat::Atmos504 => 9,
            ChannelFormat::Atmos712 | ChannelFormat::Atmos514 => 10,
            ChannelFormat::Atmos704 => 11,
            ChannelFormat::Atmos714 => 12,
            ChannelFormat::Atmos706 | ChannelFormat::Atmos904 => 13,
            ChannelFormat::Atmos716 | ChannelFormat::Atmos914 => 14,
            ChannelFormat::Atmos906 => 15,
            ChannelFormat::Atmos916 => 16,
            ChannelFormat::Ambisonics(n) => {
                let n = usize::from(n.clamp(1, 7)) + 1;
                n * n
            }
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ChannelFormat::Mono => "Mono",
            ChannelFormat::Stereo => "Stereo",
            ChannelFormat::Lcr => "LCR",
            ChannelFormat::Quad => "Quad",
            ChannelFormat::Lcrs => "LCRS",
            ChannelFormat::Surround50 => "5.0",
            ChannelFormat::Surround51 => "5.1",
            ChannelFormat::Surround60 => "6.0",
            ChannelFormat::Surround61 => "6.1",
            ChannelFormat::Sdds70 => "7.0 SDDS",
            ChannelFormat::Sdds71 => "7.1 SDDS",
            ChannelFormat::Surround70 => "7.0",
            ChannelFormat::Surround71 => "7.1",
            ChannelFormat::Atmos702 => "7.0.2",
            ChannelFormat::Atmos712 => "7.1.2",
            ChannelFormat::Atmos502 => "5.0.2",
            ChannelFormat::Atmos512 => "5.1.2",
            ChannelFormat::Atmos504 => "5.0.4",
            ChannelFormat::Atmos514 => "5.1.4",
            ChannelFormat::Atmos704 => "7.0.4",
            ChannelFormat::Atmos714 => "7.1.4",
            ChannelFormat::Atmos706 => "7.0.6",
            ChannelFormat::Atmos716 => "7.1.6",
            ChannelFormat::Atmos904 => "9.0.4",
            ChannelFormat::Atmos914 => "9.1.4",
            ChannelFormat::Atmos906 => "9.0.6",
            ChannelFormat::Atmos916 => "9.1.6",
            ChannelFormat::Ambisonics(1) => "1st Order Ambisonics",
            ChannelFormat::Ambisonics(2) => "2nd Order Ambisonics",
            ChannelFormat::Ambisonics(3) => "3rd Order Ambisonics",
            ChannelFormat::Ambisonics(4) => "4th Order Ambisonics",
            ChannelFormat::Ambisonics(5) => "5th Order Ambisonics",
            ChannelFormat::Ambisonics(6) => "6th Order Ambisonics",
            ChannelFormat::Ambisonics(_) => "7th Order Ambisonics",
        }
    }
    pub fn from_id(s: &str) -> Option<ChannelFormat> {
        ChannelFormat::ALL.into_iter().find(|f| f.label().eq_ignore_ascii_case(s.trim()))
    }
    /// Speakers in channel order, or empty for Ambisonics (ACN order, SN3D).
    ///
    /// Channel order is SMPTE / WAV order: speakers with a WAVE_FORMAT_EXTENSIBLE mask bit come in
    /// ascending bit order (L R C LFE Ls Rs Lc Rc Cs Lss Rss Ltf Rtf Ltr Rtr; 7.x puts the rear
    /// surrounds Lrs/Rrs on the "back" bits before the sides Lss/Rss), followed by speakers WAV
    /// cannot name (the top-middle pair of .6 layouts). E.g. 5.1 = L R C LFE Ls Rs,
    /// 7.1 = L R C LFE Lrs Rrs Lss Rss, 7.1.4 = L R C LFE Lrs Rrs Lss Rss Ltf Rtf Ltr Rtr.
    pub fn speakers(self) -> &'static [Speaker] {
        use Speaker::*;
        match self {
            ChannelFormat::Mono => &[C],
            ChannelFormat::Stereo => &[L, R],
            ChannelFormat::Lcr => &[L, R, C],
            ChannelFormat::Quad => &[L, R, Ls, Rs],
            ChannelFormat::Lcrs => &[L, R, C, Cs],
            ChannelFormat::Surround50 => &[L, R, C, Ls, Rs],
            ChannelFormat::Surround51 => &[L, R, C, Lfe, Ls, Rs],
            ChannelFormat::Surround60 => &[L, R, C, Ls, Rs, Cs],
            ChannelFormat::Surround61 => &[L, R, C, Lfe, Ls, Rs, Cs],
            ChannelFormat::Sdds70 => &[L, R, C, Ls, Rs, Lc, Rc],
            ChannelFormat::Sdds71 => &[L, R, C, Lfe, Ls, Rs, Lc, Rc],
            ChannelFormat::Surround70 => &[L, R, C, Lrs, Rrs, Lss, Rss],
            ChannelFormat::Surround71 => &[L, R, C, Lfe, Lrs, Rrs, Lss, Rss],
            ChannelFormat::Atmos702 => &[L, R, C, Lrs, Rrs, Lss, Rss, Ltm, Rtm],
            ChannelFormat::Atmos712 => &[L, R, C, Lfe, Lrs, Rrs, Lss, Rss, Ltm, Rtm],
            ChannelFormat::Atmos502 => &[L, R, C, Ls, Rs, Ltm, Rtm],
            ChannelFormat::Atmos512 => &[L, R, C, Lfe, Ls, Rs, Ltm, Rtm],
            ChannelFormat::Atmos504 => &[L, R, C, Ls, Rs, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos514 => &[L, R, C, Lfe, Ls, Rs, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos704 => &[L, R, C, Lrs, Rrs, Lss, Rss, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos714 => &[L, R, C, Lfe, Lrs, Rrs, Lss, Rss, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos706 => &[L, R, C, Lrs, Rrs, Lss, Rss, Ltf, Rtf, Ltr, Rtr, Ltm, Rtm],
            ChannelFormat::Atmos716 => &[L, R, C, Lfe, Lrs, Rrs, Lss, Rss, Ltf, Rtf, Ltr, Rtr, Ltm, Rtm],
            ChannelFormat::Atmos904 => &[L, R, C, Lrs, Rrs, Lw, Rw, Lss, Rss, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos914 => &[L, R, C, Lfe, Lrs, Rrs, Lw, Rw, Lss, Rss, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos906 => &[L, R, C, Lrs, Rrs, Lw, Rw, Lss, Rss, Ltf, Rtf, Ltr, Rtr, Ltm, Rtm],
            ChannelFormat::Atmos916 => &[L, R, C, Lfe, Lrs, Rrs, Lw, Rw, Lss, Rss, Ltf, Rtf, Ltr, Rtr, Ltm, Rtm],
            ChannelFormat::Ambisonics(_) => &[],
        }
    }
    pub fn is_ambisonic(self) -> bool {
        matches!(self, ChannelFormat::Ambisonics(_))
    }
    pub fn has_height(self) -> bool {
        self.speakers().iter().any(|s| s.is_height())
    }
    /// Short per-channel labels for meters (`W Y Z X 5…` for Ambisonics).
    pub fn channel_label(self, ch: usize) -> String {
        match self.speakers().get(ch) {
            Some(s) => s.label().to_string(),
            None if self.is_ambisonic() => ["W", "Y", "Z", "X"].get(ch).map_or_else(|| format!("{}", ch + 1), |s| s.to_string()),
            None => format!("{}", ch + 1),
        }
    }
    /// WAVE_FORMAT_EXTENSIBLE channel mask (0 = no positions, e.g. Ambisonics). Speakers that WAV
    /// cannot name (top middle) borrow the top-front bits when the layout has no top fronts.
    pub fn channel_mask(self) -> u32 {
        let sp = self.speakers();
        let has_tf = sp.contains(&Speaker::Ltf);
        sp.iter().fold(0u32, |m, s| match s {
            Speaker::Ltm if !has_tf => m | 0x1000,
            Speaker::Rtm if !has_tf => m | 0x4000,
            s => m | s.wave_bit(),
        })
    }
    /// Best format for a channel count.
    pub fn for_channels(n: usize) -> ChannelFormat {
        [
            ChannelFormat::Mono,
            ChannelFormat::Stereo,
            ChannelFormat::Lcr,
            ChannelFormat::Quad,
            ChannelFormat::Surround50,
            ChannelFormat::Surround51,
            ChannelFormat::Surround70,
            ChannelFormat::Surround71,
        ]
        .into_iter()
        .find(|f| f.channels() == n)
        .unwrap_or(if n <= 1 { ChannelFormat::Mono } else { ChannelFormat::Stereo })
    }
}

/// A loudspeaker position in a channel format.
///
/// Angles follow ITU-R BS.775 / Dolby conventions: azimuth in degrees, 0 = front centre, positive
/// = to the right; elevation in degrees above the listener plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Speaker {
    L,
    R,
    C,
    Lfe,
    /// 5.x surrounds (±110°).
    Ls,
    Rs,
    /// SDDS inner fronts (±15°).
    Lc,
    Rc,
    /// Centre surround (180°).
    Cs,
    /// 7.x side surrounds (±90°).
    Lss,
    Rss,
    /// 7.x rear surrounds (±150°).
    Lrs,
    Rrs,
    /// 9.x front wides (±60°).
    Lw,
    Rw,
    /// Top front (±45°, elevation 45°).
    Ltf,
    Rtf,
    /// Top middle (±90°, elevation 45°).
    Ltm,
    Rtm,
    /// Top rear (±135°, elevation 45°).
    Ltr,
    Rtr,
}

impl Speaker {
    pub fn azimuth(self) -> f32 {
        match self {
            Speaker::L => -30.0,
            Speaker::R => 30.0,
            Speaker::C | Speaker::Lfe => 0.0,
            Speaker::Ls => -110.0,
            Speaker::Rs => 110.0,
            Speaker::Lc => -15.0,
            Speaker::Rc => 15.0,
            Speaker::Cs => 180.0,
            Speaker::Lss => -90.0,
            Speaker::Rss => 90.0,
            Speaker::Lrs => -150.0,
            Speaker::Rrs => 150.0,
            Speaker::Lw => -60.0,
            Speaker::Rw => 60.0,
            Speaker::Ltf => -45.0,
            Speaker::Rtf => 45.0,
            Speaker::Ltm => -90.0,
            Speaker::Rtm => 90.0,
            Speaker::Ltr => -135.0,
            Speaker::Rtr => 135.0,
        }
    }
    pub fn elevation(self) -> f32 {
        if self.is_height() { 45.0 } else { 0.0 }
    }
    pub fn is_height(self) -> bool {
        matches!(self, Speaker::Ltf | Speaker::Rtf | Speaker::Ltm | Speaker::Rtm | Speaker::Ltr | Speaker::Rtr)
    }
    pub fn is_lfe(self) -> bool {
        self == Speaker::Lfe
    }
    pub fn label(self) -> &'static str {
        match self {
            Speaker::L => "L",
            Speaker::R => "R",
            Speaker::C => "C",
            Speaker::Lfe => "LFE",
            Speaker::Ls => "Ls",
            Speaker::Rs => "Rs",
            Speaker::Lc => "Lc",
            Speaker::Rc => "Rc",
            Speaker::Cs => "Cs",
            Speaker::Lss => "Lss",
            Speaker::Rss => "Rss",
            Speaker::Lrs => "Lrs",
            Speaker::Rrs => "Rrs",
            Speaker::Lw => "Lw",
            Speaker::Rw => "Rw",
            Speaker::Ltf => "Ltf",
            Speaker::Rtf => "Rtf",
            Speaker::Ltm => "Ltm",
            Speaker::Rtm => "Rtm",
            Speaker::Ltr => "Ltr",
            Speaker::Rtr => "Rtr",
        }
    }
    /// WAVE_FORMAT_EXTENSIBLE `dwChannelMask` bit (0 for speakers WAV cannot name).
    pub fn wave_bit(self) -> u32 {
        match self {
            Speaker::L => 0x1,
            Speaker::R => 0x2,
            Speaker::C => 0x4,
            Speaker::Lfe => 0x8,
            Speaker::Ls | Speaker::Lrs => 0x10,
            Speaker::Rs | Speaker::Rrs => 0x20,
            Speaker::Lc | Speaker::Lw => 0x40,
            Speaker::Rc | Speaker::Rw => 0x80,
            Speaker::Cs => 0x100,
            Speaker::Lss => 0x200,
            Speaker::Rss => 0x400,
            Speaker::Ltf => 0x1000,
            Speaker::Rtf => 0x4000,
            Speaker::Ltr => 0x8000,
            Speaker::Rtr => 0x2_0000,
            Speaker::Ltm | Speaker::Rtm => 0,
        }
    }
}

/// Surround panner position (Pro Tools-style X/Y puck). `None` on a mixer means "use the stereo
/// pan", which keeps sessions made before surround mixing sounding exactly as they did.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SurroundPan {
    /// -1 (left) ..= 1 (right).
    pub x: f32,
    /// -1 (back) ..= 1 (front).
    pub y: f32,
    /// Elevation 0 (ear level) ..= 1 (overhead), used by height formats.
    pub z: f32,
    /// 0 = point source ..= 1 = spread over every speaker.
    pub divergence: f32,
    /// Centre speaker share of front-centre images, 0..=100 % (0 = phantom centre on L/R).
    pub center: f32,
    /// LFE send level, dB (-144 = off).
    pub lfe_db: f32,
}

impl Default for SurroundPan {
    fn default() -> Self {
        SurroundPan { x: 0.0, y: 1.0, z: 0.0, divergence: 0.0, center: 100.0, lfe_db: -144.0 }
    }
}

impl SurroundPan {
    /// Puck at the stereo pan position (front edge).
    pub fn from_stereo(pan: f32) -> Self {
        SurroundPan { x: finite_or(pan, 0.0).clamp(-1.0, 1.0), ..SurroundPan::default() }
    }
    /// Clamp into range (NaN → defaults).
    pub fn sanitize(&mut self) {
        let d = SurroundPan::default();
        self.x = finite_or(self.x, d.x).clamp(-1.0, 1.0);
        self.y = finite_or(self.y, d.y).clamp(-1.0, 1.0);
        self.z = finite_or(self.z, d.z).clamp(0.0, 1.0);
        self.divergence = finite_or(self.divergence, d.divergence).clamp(0.0, 1.0);
        self.center = finite_or(self.center, d.center).clamp(0.0, 100.0);
        self.lfe_db = finite_or(self.lfe_db, d.lfe_db).clamp(-144.0, 12.0);
    }
}

/// An internal mix bus (Pro Tools "bus" paths).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bus {
    pub id: BusId,
    pub name: String,
    pub format: ChannelFormat,
}

/// Physical output paths (from the I/O setup).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutputPath {
    pub name: String,
    /// First hardware channel (0-based).
    pub first_channel: u16,
    pub format: ChannelFormat,
}

/// Where a signal goes or comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum Route {
    #[default]
    None,
    /// The main output (hardware outputs 1-2, or the bounce source).
    Main,
    Bus(BusId),
    /// Hardware input/output by path name.
    Hardware(String),
}

/// A process-unique identity for an [`Insert`] that survives moving it between slots, so the
/// mix engine can keep its plug-in instance (and state) through a reorder. Not saved: every
/// loaded or newly made insert gets a fresh one, and clones (undo snapshots) keep theirs. It
/// never takes part in equality.
#[derive(Debug, Clone, Copy)]
pub struct InsertUid(u64);

impl InsertUid {
    /// The identity as a number (never 0).
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl Default for InsertUid {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        InsertUid(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

impl PartialEq for InsertUid {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Where an insert's sidechain key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum KeyFrom {
    /// A track's signal. The track renders before the keyed one.
    Track(TrackId),
    /// What a bus has summed. Only a strip that renders after the bus fills (an aux or folder)
    /// can hear it.
    Bus(BusId),
}

/// Which point of the source strip the key is taken at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum Tap {
    /// After the strip's inserts, before its fader.
    Pre,
    /// After the fader (a bus has no such choice: it is always post).
    #[default]
    Post,
}

/// A sidechain key: the signal an insert detects on in place of its own input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct KeySource {
    pub from: KeyFrom,
    #[serde(default)]
    pub tap: Tap,
}

/// The plug-in id a hardware insert slot carries (ADR 0015 section 5). It names no plug-in: an
/// older build that does not know the kind sees an unknown plug-in id and passes the signal
/// through, so a session with a hardware insert still opens there.
pub const HARDWARE_PLUGIN: &str = "hardware.insert";

/// A hardware insert: the slot sends the strip's signal to an output path and takes the signal
/// that continues down the chain from an input path (ADR 0015 section 5). Both are path names,
/// resolved against the machine's I/O map, so the same session opens on another interface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct HardwareInsert {
    /// The output path the signal leaves on.
    pub send: String,
    /// The input path it comes back on.
    #[serde(rename = "return")]
    pub ret: String,
    /// Why the insert is switched off (`None`: it runs). A disabled insert passes the signal
    /// through dry and claims no send path; it is set when a session opens with an insert that
    /// cannot work, or on a duplicated track's copy, and cleared by making the insert again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<String>,
}

/// The plug-in id of an External Instrument (ADR 0020): an instrument slot that plays a hardware
/// synth. It names no plug-in: the track's MIDI goes to the port of `Track::midi_output` and the
/// synth's audio comes back on an input path. An older build sees an unknown instrument id (the
/// track is silent there) and still sends the MIDI to the port.
pub const EXTERNAL_INSTRUMENT_PLUGIN: &str = "external.instrument";

/// The largest latency an External Instrument's manual value takes, in milliseconds.
pub const MAX_EXTERNAL_LATENCY_MS: f32 = 500.0;

/// The return gain range of an External Instrument, in dB.
pub const EXTERNAL_GAIN_DB: (f32, f32) = (-60.0, 24.0);

/// An External Instrument (ADR 0020 decision 1): the synth's audio returns on input path `ret`,
/// gained by `gain_db` before the inserts. Its latency is `manual_ms` when set (the measured value
/// is stored per machine, not here). The MIDI port and channel are the track's `midi_output`.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ExternalInstrument {
    /// The input path the synth comes back on (empty: none chosen yet), resolved against the
    /// machine's I/O map like a hardware insert's return.
    #[serde(rename = "return", default)]
    pub ret: String,
    /// Gain on the return, before the inserts.
    #[serde(default)]
    pub gain_db: f32,
    /// The latency set by hand, in milliseconds (`None`: not set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_ms: Option<f32>,
}

impl ExternalInstrument {
    /// The return gain in dB, finite and clamped to [`EXTERNAL_GAIN_DB`]. Files are hostile.
    #[must_use]
    pub fn gain_db(&self) -> f32 {
        if self.gain_db.is_finite() { self.gain_db.clamp(EXTERNAL_GAIN_DB.0, EXTERNAL_GAIN_DB.1) } else { 0.0 }
    }

    /// The manual latency in milliseconds, clamped to 0..[`MAX_EXTERNAL_LATENCY_MS`] when read (a
    /// non-finite value reads as unset).
    #[must_use]
    pub fn manual_ms(&self) -> Option<f32> {
        self.manual_ms.filter(|ms| ms.is_finite()).map(|ms| ms.clamp(0.0, MAX_EXTERNAL_LATENCY_MS))
    }

    /// The manual latency in plan frames at `rate`: round(ms × r / 1000) (decision 4, case 1).
    #[must_use]
    pub fn manual_frames(&self, rate: u32) -> Option<u32> {
        self.manual_ms().map(|ms| (f64::from(ms) * f64::from(rate) / 1000.0).round() as u32)
    }
}

/// A plugin instance on an insert slot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Insert {
    /// Stable identity while the insert lives (see [`InsertUid`]).
    #[serde(skip)]
    pub uid: InsertUid,
    /// Registry id from `transit-dsp` (e.g. `eq7`, `comp`).
    pub plugin: String,
    #[serde(default, deserialize_with = "finite_params")]
    pub params: BTreeMap<String, f32>,
    #[serde(default)]
    pub bypass: bool,
    #[serde(default = "yes")]
    pub active: bool,
    /// Preset name shown in the plugin window header.
    #[serde(default)]
    pub preset: String,
    /// Opaque plugin state (third-party plugins: CLAP state / VST3 component + controller
    /// state), standard base64. Restored into a new instance before it first processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The session audio source a sample-playing instrument (the built-in Sampler) plays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<SourceId>,
    /// The sidechain key this insert detects on (built-in compressor and gate); the key belongs
    /// to the slot and moves with it. Hosted plug-ins ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeySource>,
    /// Set on a hardware insert slot (`plugin` is then [`HARDWARE_PLUGIN`]): the slot has no
    /// plug-in; the signal goes out one path of the audio interface and comes back on another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<HardwareInsert>,
    /// Set on an instrument slot holding an External Instrument (`plugin` is then
    /// [`EXTERNAL_INSTRUMENT_PLUGIN`]): a hardware synth played over MIDI, its audio returned on an
    /// input path (ADR 0020).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ExternalInstrument>,
}

fn yes() -> bool {
    true
}

impl Insert {
    pub fn new(plugin: impl Into<String>) -> Self {
        Insert {
            plugin: plugin.into(),
            params: BTreeMap::new(),
            bypass: false,
            active: true,
            preset: String::from("<factory default>"),
            state: None,
            sample: None,
            key: None,
            hardware: None,
            external: None,
            uid: InsertUid::default(),
        }
    }

    /// A hardware insert slot sending on output path `send` and returning on input path `ret`.
    pub fn hardware(send: impl Into<String>, ret: impl Into<String>) -> Self {
        Insert {
            hardware: Some(HardwareInsert { send: send.into(), ret: ret.into(), disabled: None }),
            preset: String::new(),
            ..Insert::new(HARDWARE_PLUGIN)
        }
    }

    /// An External Instrument returning on input path `ret` (ADR 0020).
    pub fn external(ret: impl Into<String>) -> Self {
        Insert {
            external: Some(ExternalInstrument { ret: ret.into(), gain_db: 0.0, manual_ms: None }),
            preset: String::new(),
            ..Insert::new(EXTERNAL_INSTRUMENT_PLUGIN)
        }
    }

    /// The External Instrument this slot holds, when it is one.
    #[must_use]
    pub fn external_instrument(&self) -> Option<&ExternalInstrument> {
        self.external.as_ref().filter(|_| self.plugin == EXTERNAL_INSTRUMENT_PLUGIN)
    }

    /// The decoded plugin state (`None` when absent or malformed).
    pub fn state_bytes(&self) -> Option<Vec<u8>> {
        self.state.as_deref().and_then(crate::b64::decode)
    }

    /// Stores a plugin state blob (base64-encoded); an empty blob clears it.
    pub fn set_state_bytes(&mut self, data: &[u8]) {
        self.state = (!data.is_empty()).then(|| crate::b64::encode(data));
    }
}

/// A send to a bus.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SendSlot {
    pub target: Route,
    pub level_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub pre_fader: bool,
    #[serde(default = "yes")]
    pub follow_main_pan: bool,
    /// Surround position into a multichannel bus (None = follow the track / stereo pan).
    #[serde(default)]
    pub surround: Option<SurroundPan>,
}

impl SendSlot {
    pub fn new(target: Route) -> Self {
        SendSlot { target, level_db: -144.0, pan: 0.0, mute: false, pre_fader: false, follow_main_pan: true, surround: None }
    }
}

/// The channel strip.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Mixer {
    pub volume_db: f32,
    /// One panner per source channel for mono/stereo (stereo tracks: two panners, default hard L/R).
    pub pan: Vec<f32>,
    pub mute: bool,
    pub solo: bool,
    #[serde(default)]
    pub solo_safe: bool,
    /// Runtime state: never saved, and false whenever a session is opened (old files that still
    /// carry the field are read and ignored), so opening a session never activates an input path.
    #[serde(skip)]
    pub record_arm: bool,
    /// Runtime state, like `record_arm`.
    #[serde(skip)]
    pub input_monitor: bool,
    #[serde(default)]
    pub phase_invert: bool,
    pub input: Route,
    pub output: Route,
    pub inserts: Vec<Option<Insert>>,
    pub sends: Vec<Option<SendSlot>>,
    pub automation_mode: AutomationMode,
    /// Input gain / trim, dB.
    #[serde(default)]
    pub trim_db: f32,
    /// VCA master that controls this track, if any (track id of the VCA).
    #[serde(default)]
    pub vca: Option<u64>,
    /// Surround panner, used when the output is multichannel (None = the stereo pan to L/R).
    #[serde(default)]
    pub surround: Option<SurroundPan>,
    /// Runtime state, like `record_arm` (ADR 0011): the automated parameters whose controls the
    /// user holds now. In Touch the mix does not read their lanes while they are held, and
    /// returns to the lanes over a short ramp on release (#188,
    /// `docs/contributing/knowledge/automation-modes.md`). Never saved; the UI sets it every
    /// frame through `Engine::set_touched`.
    #[serde(skip)]
    pub touched: Vec<AutoParam>,
}

impl Mixer {
    pub fn new(format: ChannelFormat) -> Self {
        let pan = match format {
            ChannelFormat::Mono => vec![0.0],
            ChannelFormat::Stereo => vec![-1.0, 1.0],
            _ => Vec::new(),
        };
        Mixer {
            volume_db: 0.0,
            pan,
            mute: false,
            solo: false,
            solo_safe: false,
            record_arm: false,
            input_monitor: false,
            phase_invert: false,
            input: Route::None,
            output: Route::Main,
            inserts: vec![None; INSERT_SLOTS],
            sends: vec![None; SEND_SLOTS],
            automation_mode: AutomationMode::Read,
            trim_db: 0.0,
            vca: None,
            surround: None,
            touched: Vec::new(),
        }
    }

    /// Clamp everything into range (after deserialising untrusted files).
    pub fn sanitize(&mut self) {
        self.volume_db = finite_or(self.volume_db, 0.0).clamp(-144.0, 12.0);
        for p in &mut self.pan {
            *p = finite_or(*p, 0.0).clamp(-1.0, 1.0);
        }
        self.inserts.resize(INSERT_SLOTS, None);
        self.sends.resize(SEND_SLOTS, None);
        for s in self.sends.iter_mut().flatten() {
            s.level_db = finite_or(s.level_db, -144.0).clamp(-144.0, 12.0);
            s.pan = finite_or(s.pan, 0.0).clamp(-1.0, 1.0);
            if let Some(sp) = &mut s.surround {
                sp.sanitize();
            }
        }
        if let Some(sp) = &mut self.surround {
            sp.sanitize();
        }
        self.trim_db = finite_or(self.trim_db, 0.0).clamp(-144.0, 24.0);
    }
}

impl crate::Session {
    /// The main mix format: the format of the first (main) output path, at least stereo and at
    /// most 16 channels (wider formats fall back to stereo).
    pub fn main_format(&self) -> ChannelFormat {
        match self.outputs.first().map(|o| o.format) {
            Some(f) if (2..=16).contains(&f.channels()) => f,
            _ => ChannelFormat::Stereo,
        }
    }
}

pub(crate) fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

/// Parameters saved as `null` (how JSON writes NaN and infinity) or beyond `f32` are dropped, so
/// they keep their defaults instead of failing the whole session.
fn finite_params<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, f32>, D::Error> {
    let params: BTreeMap<String, Option<f64>> = serde::Deserialize::deserialize(d)?;
    Ok(params.into_iter().filter_map(|(k, v)| Some((k, v.map(|v| v as f32).filter(|v| v.is_finite())?))).collect())
}

/// Fader taper used by Pro Tools-style faders: position 0..1 ↔ dB (-inf..+12).
pub fn fader_pos_to_db(pos: f32) -> f32 {
    let p = finite_or(pos, 0.0).clamp(0.0, 1.0);
    if p <= 0.0 {
        return -144.0;
    }
    // Piecewise: top 75 % covers -24..+12 linearly-ish, the bottom compresses to -inf.
    if p >= 0.25 { -24.0 + (p - 0.25) / 0.75 * 36.0 } else { -24.0 - (1.0 - p / 0.25).powf(1.5) * 120.0 }
}

pub fn fader_db_to_pos(db: f32) -> f32 {
    let db = finite_or(db, -144.0);
    if db <= -144.0 {
        return 0.0;
    }
    if db >= -24.0 {
        (0.25 + (db.min(12.0) + 24.0) / 36.0 * 0.75).min(1.0)
    } else {
        let x = ((-24.0 - db) / 120.0).clamp(0.0, 1.0).powf(1.0 / 1.5);
        ((1.0 - x) * 0.25).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_state_round_trips_through_json() {
        let mut i = Insert::new("vst3:00");
        assert_eq!(i.state_bytes(), None);
        let blob: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        i.set_state_bytes(&blob);
        let json = serde_json::to_string(&i).unwrap();
        let back: Insert = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state_bytes().unwrap(), blob);
        i.set_state_bytes(&[]);
        assert_eq!(i.state, None);
        // Old sessions have no `state`; stateless inserts don't write one.
        let old: Insert = serde_json::from_str(r#"{"plugin":"eq_7band"}"#).unwrap();
        assert_eq!(old.state, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("state"));
        // A hostile state string decodes to nothing rather than failing the load.
        let bad: Insert = serde_json::from_str(r#"{"plugin":"x","state":"!!!"}"#).unwrap();
        assert_eq!(bad.state_bytes(), None);
    }

    #[test]
    fn params_saved_as_null_or_out_of_range_are_dropped() {
        let mut i = Insert::new("eq_7band");
        i.params.extend([("gain".to_string(), f32::NAN), ("q".to_string(), 0.7)]);
        let back: Insert = serde_json::from_str(&serde_json::to_string(&i).unwrap()).unwrap();
        assert_eq!(back.params, BTreeMap::from([("q".to_string(), 0.7)]));
        let big: Insert = serde_json::from_str(r#"{"plugin":"x","params":{"gain":1e39,"q":0.5}}"#).unwrap();
        assert_eq!(big.params, BTreeMap::from([("q".to_string(), 0.5)]));
    }

    #[test]
    fn fader_taper_round_trips() {
        for db in [-100.0f32, -60.0, -24.0, -10.0, 0.0, 6.0, 12.0] {
            let p = fader_db_to_pos(db);
            assert!((fader_pos_to_db(p) - db).abs() < 0.05, "{db} → {p}");
        }
        assert_eq!(fader_pos_to_db(0.0), -144.0);
        assert_eq!(fader_db_to_pos(f32::NAN), 0.0);
    }

    #[test]
    fn mixer_defaults_and_sanitize() {
        let mut m = Mixer::new(ChannelFormat::Stereo);
        assert_eq!(m.pan, vec![-1.0, 1.0]);
        m.volume_db = f32::INFINITY;
        m.inserts.truncate(2);
        m.sanitize();
        assert_eq!(m.volume_db, 0.0);
        assert_eq!(m.inserts.len(), INSERT_SLOTS);
    }

    #[test]
    fn speaker_layouts_match_channel_counts_and_masks() {
        for f in ChannelFormat::ALL {
            if !f.is_ambisonic() {
                assert_eq!(f.speakers().len(), f.channels(), "{f:?}");
                // Mask bits are unique and in ascending channel order.
                let bits: Vec<u32> = f.speakers().iter().map(|s| s.wave_bit()).filter(|b| *b != 0).collect();
                assert!(bits.windows(2).all(|w| w[0] < w[1]), "{f:?} not in WAV order");
            }
        }
        assert_eq!(ChannelFormat::Surround51.channel_mask(), 0x3F);
        assert_eq!(ChannelFormat::Surround71.channel_mask(), 0x63F);
        assert_eq!(ChannelFormat::Atmos714.channel_mask(), 0x2_D63F);
        assert_eq!(ChannelFormat::Ambisonics(1).channel_mask(), 0);
    }

    #[test]
    fn surround_pan_defaults_and_old_sessions() {
        let m = Mixer::new(ChannelFormat::Mono);
        assert_eq!(m.surround, None);
        // A mixer saved before surround existed still loads.
        let mut v = serde_json::to_value(&m).unwrap();
        v.as_object_mut().unwrap().remove("surround");
        let back: Mixer = serde_json::from_value(v).unwrap();
        assert_eq!(back.surround, None);
        let mut sp: SurroundPan = serde_json::from_str(r#"{"x": 5.0}"#).unwrap();
        sp.sanitize();
        assert_eq!(sp.x, 1.0);
        assert_eq!(sp.center, 100.0);
    }

    #[test]
    fn formats() {
        assert_eq!(ChannelFormat::for_channels(6), ChannelFormat::Surround51);
        assert_eq!(ChannelFormat::from_id("stereo"), Some(ChannelFormat::Stereo));
    }

    #[test]
    fn arm_and_monitor_are_runtime_only() {
        use crate::{ChannelFormat, Session, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        {
            let m = &mut s.track_mut(t).unwrap().mixer;
            m.record_arm = true;
            m.input_monitor = true;
            m.volume_db = -6.0;
            m.solo_safe = true;
            m.trim_db = 3.0;
        }
        let text = s.to_json().unwrap();
        assert!(!text.contains("record_arm") && !text.contains("input_monitor"), "runtime state was saved");
        let back = Session::from_json(&text).unwrap();
        let m = &back.track(t).unwrap().mixer;
        assert!(!m.record_arm && !m.input_monitor);
        let mut expected = s.track(t).unwrap().mixer.clone();
        (expected.record_arm, expected.input_monitor) = (false, false);
        assert_eq!(*m, expected, "every other mixer field round-trips");
    }

    #[test]
    fn old_files_with_arm_and_monitor_open_disarmed() {
        use crate::{ChannelFormat, Session, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut v = serde_json::to_value(&s).unwrap();
        v["tracks"][0]["mixer"]["record_arm"] = serde_json::json!(true);
        v["tracks"][0]["mixer"]["input_monitor"] = serde_json::json!(true);
        let back = Session::from_json(&v.to_string()).unwrap();
        let m = &back.track(t).unwrap().mixer;
        assert!(!m.record_arm && !m.input_monitor);
    }

    #[test]
    fn a_hardware_insert_round_trips_and_other_inserts_do_not_carry_it() {
        let hw = Insert::hardware("Out 3-4", "In 3-4");
        assert_eq!(hw.plugin, HARDWARE_PLUGIN);
        let json = serde_json::to_string(&hw).unwrap();
        assert!(json.contains(r#""hardware":{"send":"Out 3-4","return":"In 3-4"}"#), "{json}");
        let back: Insert = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hardware, hw.hardware);
        assert!(!serde_json::to_string(&Insert::new("gain")).unwrap().contains("hardware"));
        let old: Insert = serde_json::from_str(r#"{"plugin":"eq_7band"}"#).unwrap();
        assert!(old.hardware.is_none());
        // A build that does not know the field reads the slot as an unknown plug-in.
        #[derive(serde::Deserialize)]
        struct Previous {
            plugin: String,
        }
        assert_eq!(serde_json::from_str::<Previous>(&json).unwrap().plugin, HARDWARE_PLUGIN);
    }

    #[test]
    fn a_key_round_trips_and_old_files_have_none() {
        use crate::{ChannelFormat, Session, TrackKind};
        let mut s = Session::default();
        let kick = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let bass = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let key = KeySource { from: KeyFrom::Track(kick), tap: Tap::Pre };
        let mut ins = Insert::new("compressor");
        ins.key = Some(key);
        s.track_mut(bass).unwrap().mixer.inserts[2] = Some(ins);
        let json = s.to_json().unwrap();
        let back = Session::from_json(&json).unwrap();
        assert_eq!(back.track(bass).unwrap().mixer.inserts[2].as_ref().unwrap().key, Some(key));
        // A build without keys reads the file: the extra field is ignored there (no
        // `deny_unknown_fields`), and a key-less insert writes none.
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let ins = &mut v["tracks"][1]["mixer"]["inserts"][2];
        assert!(ins.as_object_mut().unwrap().remove("key").is_some());
        let old = Session::from_json(&v.to_string()).unwrap();
        assert_eq!(old.track(bass).unwrap().mixer.inserts[2].as_ref().unwrap().key, None);
        assert!(!serde_json::to_string(&Insert::new("gain")).unwrap().contains("key"));
        // A bus key, and a tap left out of the file, which means post.
        let bus: KeySource = serde_json::from_str(r#"{"from":{"Bus":7}}"#).unwrap();
        assert_eq!(bus, KeySource { from: KeyFrom::Bus(BusId(7)), tap: Tap::Post });
    }
    #[test]
    fn an_external_instrument_round_trips_in_a_session_and_old_sessions_load_unchanged() {
        use crate::{MidiOutChannel, MidiOutPort, MidiOutput, Session, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Model D"));
        // A session without the device writes nothing new, so older builds read it as before.
        let before = s.to_json().unwrap();
        assert!(!before.contains("external"), "{before}");
        let mut ins = Insert::external("In 1");
        ins.external.as_mut().unwrap().gain_db = -3.0;
        ins.external.as_mut().unwrap().manual_ms = Some(19.5);
        let tr = s.track_mut(t).unwrap();
        tr.instrument = Some(ins);
        tr.midi_output = MidiOutput::External {
            port: MidiOutPort { backend: "winmm".into(), name: "MODEL D".into(), ordinal: 0, id: String::new() },
            channel: MidiOutChannel::Channel(0),
            offset_ms: 0.0,
        };
        let json = s.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let slot = &v["tracks"][0]["instrument"];
        assert_eq!(slot["plugin"], EXTERNAL_INSTRUMENT_PLUGIN, "{json}");
        assert_eq!(slot["external"], serde_json::json!({"return": "In 1", "gain_db": -3.0, "manual_ms": 19.5}), "{json}");
        let back = Session::from_json(&json).unwrap();
        let tr = back.track(t).unwrap();
        assert_eq!(tr.instrument, s.track(t).unwrap().instrument);
        assert_eq!(tr.external_instrument().and_then(ExternalInstrument::manual_ms), Some(19.5));
        // An old file (no `external` field) has no device; a file with a minimal object has the defaults.
        let old: Insert = serde_json::from_str(r#"{"plugin":"synth"}"#).unwrap();
        assert!(old.external.is_none() && old.external_instrument().is_none());
        let minimal: Insert = serde_json::from_str(r#"{"plugin":"external.instrument","external":{}}"#).unwrap();
        assert_eq!(minimal.external_instrument(), Some(&ExternalInstrument::default()));
        // A build that does not know the device reads the slot as an unknown instrument.
        #[derive(serde::Deserialize)]
        struct Previous {
            plugin: String,
        }
        let slot = serde_json::to_string(s.track(t).unwrap().instrument.as_ref().unwrap()).unwrap();
        assert_eq!(serde_json::from_str::<Previous>(&slot).unwrap().plugin, EXTERNAL_INSTRUMENT_PLUGIN);
        // The field is only the device's on the device's id.
        let mut other = Insert::new("synth");
        other.external = Some(ExternalInstrument::default());
        assert!(other.external_instrument().is_none());
    }

    #[test]
    fn manual_latency_converts_at_the_rate_and_hostile_values_are_clamped() {
        let ei = |ms: Option<f32>, gain_db: f32| ExternalInstrument { ret: "In 1".into(), gain_db, manual_ms: ms };
        assert_eq!(ei(Some(20.0), 0.0).manual_frames(48_000), Some(960));
        assert_eq!(ei(Some(20.0), 0.0).manual_frames(44_100), Some(882));
        assert_eq!(ei(Some(19.5), 0.0).manual_frames(48_000), Some(936));
        assert_eq!(ei(Some(1e9), 0.0).manual_ms(), Some(MAX_EXTERNAL_LATENCY_MS));
        assert_eq!(ei(Some(1e9), 0.0).manual_frames(48_000), Some(24_000));
        assert_eq!(ei(Some(-5.0), 0.0).manual_frames(48_000), Some(0));
        assert_eq!(ei(Some(f32::NAN), 0.0).manual_ms(), None);
        assert_eq!(ei(None, 0.0).manual_frames(48_000), None);
        assert_eq!(ei(None, f32::INFINITY).gain_db(), 0.0);
        assert_eq!(ei(None, 1e6).gain_db(), EXTERNAL_GAIN_DB.1);
        assert_eq!(ei(None, -1e6).gain_db(), EXTERNAL_GAIN_DB.0);
    }

    #[test]
    fn the_midi_offset_is_held_at_zero_on_an_external_instrument_track() {
        use crate::{MidiOutChannel, MidiOutPort, MidiOutput, Track, TrackId, TrackKind};
        let mut t = Track::new(TrackId(1), "Model D", TrackKind::Instrument, ChannelFormat::Stereo, [0; 3]);
        t.midi_output = MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::AsRecorded, offset_ms: 12.0 };
        assert_eq!(t.midi_out_offset_ms(), 12.0);
        t.instrument = Some(Insert::external("In 1"));
        assert_eq!(t.midi_out_offset_ms(), 0.0, "a hand-edited offset is not a second timing knob");
        // Only an instrument track's instrument slot holds the device.
        t.kind = TrackKind::Midi;
        assert!(t.external_instrument().is_none());
    }
}
