//! The machine's I/O map (ADR 0015): how many channels the open audio device has and the named
//! paths laid over them. Per machine, not per session: the app keeps it with its preferences
//! (keyed by audio system plus device name) and hands it to the engine. A session refers to
//! paths by name (`Route::Hardware`), so the same session opens on another interface.

use crate::{ChannelFormat, MidiOutPort, MidiOutput, Track};

/// Most device channels opened in either direction. A path or bus is narrower
/// (`MAX_PATH_WIDTH`); this is the width of the device.
pub const MAX_DEVICE_CHANNELS: usize = 32;
/// Widest path (the mix's `MAX_CHANNELS`).
pub const MAX_PATH_WIDTH: usize = 16;
/// The input path of what the computer plays (#268): a stereo pair after the device's own input
/// channels while it runs ([`IoMap::system_audio`]); not part of the saved table.
pub const SYSTEM_AUDIO_PATH: &str = "System Audio";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum IoDirection {
    Input,
    Output,
}

/// A name, a width and a channel set; `channels` may be non-contiguous. 0-based device channels.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IoPath {
    pub name: String,
    pub direction: IoDirection,
    pub channels: Vec<u16>,
    pub format: ChannelFormat,
    /// Keep this output's own arrival instead of shared device padding. Per machine only.
    #[serde(default)]
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoError {
    EmptyName,
    Duplicate(String),
    Width(usize),
    OutOfRange {
        channel: u16,
        opened: u16,
    },
    Missing(String),
    Repeated(u16),
    /// Main plays on this path.
    MainPath,
    LiveMain,
    LiveInput,
    /// The name belongs to a built-in input ("System Audio").
    Reserved(String),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::EmptyName => write!(f, "a path needs a name"),
            IoError::Duplicate(n) => write!(f, "a path named `{n}` exists"),
            IoError::Width(w) => write!(f, "a path is 1 to {MAX_PATH_WIDTH} channels wide, not {w}"),
            IoError::OutOfRange { channel, opened } => write!(f, "channel {} is not open (the device opened {opened})", channel + 1),
            IoError::Repeated(c) => write!(f, "channel {} is listed twice", c + 1),
            IoError::Missing(n) => write!(f, "no path named `{n}`"),
            IoError::MainPath => write!(f, "Main plays on this path; choose another for Main first"),
            IoError::LiveMain => write!(f, "Main cannot be a live path; turn off Live first"),
            IoError::LiveInput => write!(f, "Live is only available on output paths"),
            IoError::Reserved(n) => write!(f, "`{n}` is the built-in input of what the computer plays; choose another name"),
        }
    }
}

impl std::error::Error for IoError {}

/// A measured round trip through a hardware insert (ADR 0015 section 5): the frames between a
/// sample written to the `send` output path and the same sample arriving on the `ret` input path,
/// at one session `rate`. Stored per machine with the path table, since it is a fact about this
/// interface and its cabling; a session names the paths only.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HardwareLatency {
    pub send: String,
    pub ret: String,
    pub round_trip_frames: u32,
    pub rate: u32,
    /// Seconds since the Unix epoch when it was measured (0 when unknown).
    #[serde(default)]
    pub measured_at: u64,
    /// The identity of the input device the return came through (ADR 0015, amendment for #431,
    /// rule 13); empty for a figure stored before it was recorded (see [`IoMap::returns_through`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub input_device: String,
}

/// A measured External Instrument latency C (ADR 0020 decisions 4 and 5): the plan frames from
/// the render frame of a note-on sent to the MIDI port to the render frame at which its onset
/// enters the strip from the `ret` input path, at one `rate` and device `buffer`. Stored per
/// machine with the path table, next to (never merged with) the hardware insert round trips: it
/// is a fact about the synth, its port and this interface's settings, and a session names the port
/// and the return only. The key is (port id, else port name; return; rate; buffer).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalLatency {
    /// The port's stable backend id, empty when it has none.
    #[serde(default)]
    pub port_id: String,
    pub port_name: String,
    pub ret: String,
    pub rate: u32,
    /// Device buffer in frames when it was measured (C moves with it); 0 when the device did not
    /// say.
    pub buffer: u32,
    /// The median over the notes, in plan frames at `rate`.
    pub frames: u32,
    /// The standard deviation of the notes' latencies, in frames (the spread the card shows).
    #[serde(default)]
    pub spread_frames: u32,
    /// Notes with an onset (of the 8 measured).
    #[serde(default)]
    pub notes: u32,
    /// Seconds since the Unix epoch when it was measured (0 when unknown).
    #[serde(default)]
    pub measured_at: u64,
    /// The identity of the input device the return came through (as [`HardwareLatency::input_device`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub input_device: String,
}

impl ExternalLatency {
    /// Whether this measurement was made through `port`: by id when both have one, else by name.
    #[must_use]
    pub fn port_is(&self, port: &MidiOutPort) -> bool {
        if !self.port_id.is_empty() && !port.id.is_empty() { self.port_id == port.id } else { self.port_name.eq_ignore_ascii_case(&port.name) }
    }

    /// Same port and return (any rate and buffer).
    #[must_use]
    pub fn chain_is(&self, port: &MidiOutPort, ret: &str) -> bool {
        self.port_is(port) && self.ret.eq_ignore_ascii_case(ret)
    }

    /// Same key: port, return, rate and buffer (the input device is compared by the table, which
    /// knows what an empty one stands for).
    #[must_use]
    pub fn same_key(&self, other: &ExternalLatency) -> bool {
        let port = MidiOutPort { name: other.port_name.clone(), id: other.port_id.clone(), ..MidiOutPort::default() };
        self.chain_is(&port, &other.ret) && self.rate == other.rate && self.buffer == other.buffer
    }

    /// `frames` at `rate`: rescaled by time when it was measured at another rate (1061 at 48 kHz
    /// is 975 at 44.1 kHz), as is otherwise.
    #[must_use]
    pub fn frames_at(&self, rate: u32) -> u32 {
        if rate == self.rate || self.rate == 0 {
            self.frames
        } else {
            (f64::from(self.frames) * f64::from(rate) / f64::from(self.rate)).round() as u32
        }
    }
}

/// Where an External Instrument's latency comes from (ADR 0020 decision 4's lookup).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalSource {
    /// `manual_ms`, which wins whenever it is set.
    Manual,
    /// A measurement whose key matches exactly.
    Measured(ExternalLatency),
    /// The newest measurement of the same port and return at another rate or buffer: used (a
    /// rate rescaled by time), and labeled "measure again".
    Stale(ExternalLatency),
}

/// An External Instrument's latency C in plan frames at the current rate, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalLookup {
    pub frames: u32,
    pub source: ExternalSource,
}

/// One device's channel counts and path table. `inputs` and `outputs` are the counts the stream
/// opened (stored here, so one device in two USB modes is one entry that adapts).
///
/// Since the amendment for #431 (rule 8) the saved tables are per side: this struct is the output
/// record (outputs, Main, measurements; keyed by system and output device) and, composed with an
/// [`crate::InputMap`], the table the engine holds ([`IoMap::compose`]). A record an older build
/// saved holds both halves; its input half is written back as it was read and is read only where
/// its key is the open input's identity ([`crate::adopt_input`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IoMap {
    pub system: String,
    pub device: String,
    /// The identity of the input device whose record the input half came from (the capture
    /// endpoint actually opened, rule 8); empty when no input is composed in. Not saved in an
    /// output record.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub input_device: String,
    pub inputs: u16,
    pub outputs: u16,
    pub paths: Vec<IoPath>,
    /// The output path Main plays on.
    pub main: String,
    /// Measured hardware-insert round trips (see [`HardwareLatency`]).
    pub hardware_latency: Vec<HardwareLatency>,
    /// Measured External Instrument latencies (see [`ExternalLatency`]); a separate table.
    pub external_latency: Vec<ExternalLatency>,
    /// The buffer, in frames, the open device runs (the Player sets it; not saved): the part of
    /// an External Instrument measurement's key the table cannot know by itself.
    #[serde(skip)]
    pub buffer: Option<u32>,
    /// The first live input channel of the System Audio pair while it runs (the Player sets it;
    /// not saved): the pair follows the device's own input channels. `None` when it is off or did
    /// not open, and a route to it is then silent.
    #[serde(skip)]
    pub system_audio: Option<u16>,
}

impl Default for IoMap {
    fn default() -> Self {
        Self::for_device("", "", 0, 2)
    }
}

fn format_of(n: usize) -> ChannelFormat {
    ChannelFormat::for_channels(n.clamp(1, MAX_PATH_WIDTH))
}

/// "Out 3-4" for two channels from 2 (1-based names, today's labels).
fn label(prefix: &str, first: u16, width: u16) -> String {
    if width <= 1 { format!("{prefix} {}", first + 1) } else { format!("{prefix} {}-{}", first + 1, first + width) }
}

impl IoPath {
    /// The device channel strip channel `k` of a `strip`-channel route through this path uses:
    /// the path's own channels in order. A mono path feeds both sides of a stereo strip (a stereo
    /// track on "In 3" records and hears channel 3 on both sides, #681); any other strip wider
    /// than the path takes the channels after its last one.
    pub fn source_channel(&self, k: usize, strip: usize) -> usize {
        if let [only] = self.channels[..]
            && strip <= 2
        {
            return usize::from(only);
        }
        match self.channels.get(k) {
            Some(&c) => usize::from(c),
            None => self.channels.last().map_or(k, |&l| usize::from(l).saturating_add(k + 1 - self.channels.len())),
        }
    }
}

/// The default input paths for an input `inputs` channels wide (capped at
/// [`MAX_DEVICE_CHANNELS`]): "In 1" per channel, then the stereo pairs "In 1-2", "In 3-4"
/// (section 1.2: the labels sessions already use).
pub fn default_input_paths(inputs: u16) -> Vec<IoPath> {
    let inputs = inputs.min(MAX_DEVICE_CHANNELS as u16);
    let mono = (0..inputs).map(|c| IoPath {
        live: false,
        name: label("In", c, 1),
        direction: IoDirection::Input,
        channels: vec![c],
        format: ChannelFormat::Mono,
    });
    let pairs = (0..inputs / 2).map(|i| 2 * i).map(|c| IoPath {
        live: false,
        name: label("In", c, 2),
        direction: IoDirection::Input,
        channels: vec![c, c + 1],
        format: ChannelFormat::Stereo,
    });
    mono.chain(pairs).collect()
}

/// Why an input path the table defines cannot be heard or recorded (ADR 0015, amendment for #431,
/// rules 8 to 10): it monitors silence, refuses to record, a hardware insert returning on it runs
/// dry and an External Instrument has no return. It never falls back to another channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputUnavailable {
    /// An output path has the same name (the two halves met in one table).
    Clash,
    /// A channel is not open: the input is narrower than the path (or not open at all).
    OutOfRange { opened: u16 },
}

impl std::fmt::Display for InputUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputUnavailable::Clash => write!(f, "same name as an output path: rename one"),
            InputUnavailable::OutOfRange { opened: 0 } => write!(f, "out of range: no input channels are open"),
            InputUnavailable::OutOfRange { opened: 1 } => write!(f, "out of range: the input has 1 channel open"),
            InputUnavailable::OutOfRange { opened } => write!(f, "out of range: the input has {opened} channels open"),
        }
    }
}

/// The device input channel (not yet wrapped to the channels the device opened) strip channel `k`
/// of a `strip`-channel strip listens to or records for an input route named `name`, or `None`
/// when the name is an unavailable input path ([`InputUnavailable`]: silence, never another
/// channel). One function for the mix's monitoring and the recorder, so they cannot disagree (ADR
/// 0015 section 1.4). A name in `io`'s table resolves through it, so a path renamed "Guitar" on
/// channel 5 is channel 5. A name that is not there (an old session, or one made on another
/// machine) keeps the label meaning it always had: the number in "In 3" or "In 3-4" is the first
/// channel (amendment for #431, rule 10: "on port groups, other unresolved names keep today's
/// label meaning"). A mono label ("In 3") feeds both sides of a stereo strip, as a mono path in
/// the table does.
pub fn input_channel(io: Option<&IoMap>, name: &str, k: usize, strip: usize) -> Option<usize> {
    if let Some(state) = io.and_then(|m| m.input_state(name)) {
        return state.ok().map(|p| p.source_channel(k, strip));
    }
    let first = name.trim_start_matches("In ").split(['-', ' ']).next().and_then(|n| n.parse::<usize>().ok()).map_or(0, |n| n.saturating_sub(1));
    Some(if strip <= 2 && is_mono_label(name) { first } else { first.saturating_add(k) })
}

/// Whether `name` is a one-channel input label: "In " and a number, nothing after it.
fn is_mono_label(name: &str) -> bool {
    name.strip_prefix("In ").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether an input route named `name` is the System Audio path.
pub fn is_system_audio(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case(SYSTEM_AUDIO_PATH)
}

/// The live input channel strip channel `k` (of `strip`) of an input route named `name` reads, among the
/// `live` channels the input delivers, or `None` for silence. The one resolution for the mix's
/// monitoring and the recorder. A device path wraps to the live channels as it always has
/// ([`input_channel`]); an unavailable input path is silent; a device channel at or past
/// [`MAX_DEVICE_CHANNELS`] is never read (#431 Q6: a JACK port group of more than 32 ports opens
/// them all, and the table stops at 32). System Audio is its pair (a mono strip hears the left
/// side), and silent while it does not run, never another input.
pub fn live_input_channel(io: Option<&IoMap>, name: &str, k: usize, strip: usize, live: usize) -> Option<usize> {
    if is_system_audio(name) {
        let first = usize::from(io?.system_audio?);
        let channel = first + k % 2;
        return (channel < live).then_some(channel);
    }
    let channel = input_channel(io, name, k, strip)? % live.max(1);
    (channel < MAX_DEVICE_CHANNELS).then_some(channel)
}

impl IoMap {
    /// The default table for an opened device (counts capped at [`MAX_DEVICE_CHANNELS`]): outputs
    /// "Out 1-2" (Main) and one stereo path per remaining pair, a lone trailing channel mono;
    /// inputs "In 1" per channel plus stereo pairs "In 1-2" (the labels sessions already use).
    pub fn for_device(system: &str, device: &str, inputs: u16, outputs: u16) -> Self {
        let cap = MAX_DEVICE_CHANNELS as u16;
        let (inputs, outputs) = (inputs.min(cap), outputs.min(cap));
        let mut paths = Vec::new();
        let mut c = 0;
        while c < outputs {
            let width = (outputs - c).min(2);
            paths.push(IoPath {
                live: false,
                name: label("Out", c, width),
                direction: IoDirection::Output,
                channels: (c..c + width).collect(),
                format: format_of(usize::from(width)),
            });
            c += width;
        }
        paths.extend(default_input_paths(inputs));
        let main = paths.iter().find(|p| p.direction == IoDirection::Output).map_or_else(|| "Out 1-2".to_string(), |p| p.name.clone());
        IoMap {
            system: system.into(),
            device: device.into(),
            // One device in both directions (ASIO's driver, a duplex fake): the input is the device.
            input_device: if inputs > 0 { device.into() } else { String::new() },
            inputs,
            outputs,
            paths,
            main,
            hardware_latency: Vec::new(),
            external_latency: Vec::new(),
            buffer: None,
            system_audio: None,
        }
    }

    /// The key the app stores this table under.
    pub fn key(&self) -> (&str, &str) {
        (&self.system, &self.device)
    }

    /// The path named `name` (an output first when an input path clashes with it).
    pub fn path(&self, name: &str) -> Option<&IoPath> {
        self.paths.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// The position of the path named `name`, of `direction` when given (a clash leaves two paths
    /// of one name, one per direction, until one is renamed), else the first.
    fn position(&self, name: &str, direction: Option<IoDirection>) -> Result<usize, IoError> {
        self.paths
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name) && direction.is_none_or(|d| p.direction == d))
            .ok_or_else(|| IoError::Missing(name.into()))
    }

    /// The input path named `name`, available or not.
    pub fn input_path(&self, name: &str) -> Option<&IoPath> {
        self.paths.iter().find(|p| p.direction == IoDirection::Input && p.name.eq_ignore_ascii_case(name))
    }

    /// Why the input path `p` of this table cannot be used now, or `None` when it can: a name an
    /// output path also has, or a channel past the open input's count.
    pub fn input_unavailable(&self, p: &IoPath) -> Option<InputUnavailable> {
        if self.paths.iter().any(|o| o.direction == IoDirection::Output && o.name.eq_ignore_ascii_case(&p.name)) {
            return Some(InputUnavailable::Clash);
        }
        p.channels.iter().any(|&c| c >= self.inputs).then_some(InputUnavailable::OutOfRange { opened: self.inputs })
    }

    /// The input path named `name` and whether it can be used; `None` when the table has no input
    /// path of that name.
    pub fn input_state(&self, name: &str) -> Option<Result<&IoPath, InputUnavailable>> {
        let p = self.input_path(name)?;
        Some(self.input_unavailable(p).map_or(Ok(p), Err))
    }

    /// The input path named `name` when it can be used now.
    pub fn available_input(&self, name: &str) -> Option<&IoPath> {
        self.input_state(name)?.ok()
    }

    /// Whether a measurement whose return came through `input_device` applies to the input open
    /// now (amendment for #431, rules 11 and 13). A figure that names its input applies only while
    /// that input is open. A figure stored before inputs were recorded (empty) applies only where
    /// its return's identity is established: this record's key is the open input's identity (one
    /// device in both directions, as ASIO's driver). Equal display names elsewhere are not enough.
    #[must_use]
    pub fn returns_through(&self, input_device: &str) -> bool {
        !self.input_device.is_empty() && self.input_device == self.figure_input(input_device)
    }

    /// The input a figure came back through: its own, or for an older figure this record's device.
    fn figure_input<'a>(&'a self, input_device: &'a str) -> &'a str {
        if input_device.is_empty() { &self.device } else { input_device }
    }

    pub fn paths_of(&self, direction: IoDirection) -> impl Iterator<Item = &IoPath> {
        self.paths.iter().filter(move |p| p.direction == direction)
    }

    fn opened(&self, direction: IoDirection) -> u16 {
        match direction {
            IoDirection::Input => self.inputs,
            IoDirection::Output => self.outputs,
        }
    }

    /// A name, width 1..=16, every channel open. Name clashes are the editing methods' check.
    pub fn validate(&self, path: &IoPath) -> Result<(), IoError> {
        if path.live {
            if path.direction == IoDirection::Input {
                return Err(IoError::LiveInput);
            }
            if path.name.eq_ignore_ascii_case(&self.main) {
                return Err(IoError::LiveMain);
            }
        }
        if path.name.trim().is_empty() {
            return Err(IoError::EmptyName);
        }
        if path.channels.is_empty() || path.channels.len() > MAX_PATH_WIDTH {
            return Err(IoError::Width(path.channels.len()));
        }
        if let Some((_, &c)) = path.channels.iter().enumerate().find(|(i, c)| path.channels[..*i].contains(c)) {
            return Err(IoError::Repeated(c));
        }
        let opened = self.opened(path.direction);
        if let Some(&channel) = path.channels.iter().find(|&&c| c >= opened) {
            return Err(IoError::OutOfRange { channel, opened });
        }
        Ok(())
    }

    /// Names are unique across both directions. Output paths stay ahead of input paths, as the
    /// composed table has them ([`IoMap::compose`]).
    pub fn add_path(&mut self, mut path: IoPath) -> Result<(), IoError> {
        path.name = path.name.trim().to_string();
        self.validate(&path)?;
        if is_system_audio(&path.name) {
            return Err(IoError::Reserved(path.name));
        }
        if self.path(&path.name).is_some() {
            return Err(IoError::Duplicate(path.name));
        }
        let at = match path.direction {
            IoDirection::Output => self.paths.iter().position(|p| p.direction == IoDirection::Input).unwrap_or(self.paths.len()),
            IoDirection::Input => self.paths.len(),
        };
        self.paths.insert(at, path);
        Ok(())
    }

    /// Rename a path. Sessions refer to paths by name and keep the names they have: nothing here or
    /// in the engine rewrites a `Route::Hardware`, so a route left naming the old name is an
    /// unresolved route (silent, listed by the engine), not a silent change of destination.
    pub fn rename_path(&mut self, name: &str, to: &str) -> Result<(), IoError> {
        self.rename_path_in(name, None, to)
    }

    /// [`Self::rename_path`] of the path of `direction` (one side of a clash).
    pub fn rename_path_in(&mut self, name: &str, direction: Option<IoDirection>, to: &str) -> Result<(), IoError> {
        let to = to.trim();
        if to.is_empty() {
            return Err(IoError::EmptyName);
        }
        if is_system_audio(to) {
            return Err(IoError::Reserved(to.into()));
        }
        let i = self.position(name, direction)?;
        if self.paths.iter().enumerate().any(|(j, p)| j != i && p.name.eq_ignore_ascii_case(to)) {
            return Err(IoError::Duplicate(to.into()));
        }
        let p = &mut self.paths[i];
        if p.direction == IoDirection::Output && self.main.eq_ignore_ascii_case(&p.name) {
            self.main = to.into();
        }
        p.name = to.into();
        Ok(())
    }

    /// Set the per-machine cue padding opt-out, only on a non-Main output.
    pub fn set_live(&mut self, name: &str, live: bool) -> Result<(), IoError> {
        let i = self.paths.iter().position(|p| p.name.eq_ignore_ascii_case(name)).ok_or_else(|| IoError::Missing(name.into()))?;
        let mut next = self.paths[i].clone();
        next.live = live;
        self.validate(&next)?;
        self.paths[i] = next;
        Ok(())
    }

    /// Replace a path's channels (its format follows the new width).
    pub fn set_channels(&mut self, name: &str, channels: Vec<u16>) -> Result<(), IoError> {
        self.set_channels_in(name, None, channels)
    }

    /// [`Self::set_channels`] of the path of `direction` (one side of a clash).
    pub fn set_channels_in(&mut self, name: &str, direction: Option<IoDirection>, channels: Vec<u16>) -> Result<(), IoError> {
        let i = self.position(name, direction)?;
        let mut next = self.paths[i].clone();
        next.format = format_of(channels.len());
        next.channels = channels;
        self.validate(&next)?;
        self.paths[i] = next;
        Ok(())
    }

    pub fn remove_path(&mut self, name: &str) -> Result<(), IoError> {
        self.remove_path_in(name, None)
    }

    /// [`Self::remove_path`] of the path of `direction` (one side of a clash). The measurements
    /// that used it go with it: an output's as a send, an input's as a return.
    pub fn remove_path_in(&mut self, name: &str, direction: Option<IoDirection>) -> Result<(), IoError> {
        let i = self.position(name, direction)?;
        let removed = self.paths[i].direction;
        if removed == IoDirection::Output && self.main.eq_ignore_ascii_case(name) {
            return Err(IoError::MainPath);
        }
        self.paths.remove(i);
        match removed {
            IoDirection::Output => self.hardware_latency.retain(|m| !m.send.eq_ignore_ascii_case(name)),
            // Only the figures that came back through this input: another input's path of the
            // same name keeps its own (amendment for #431, rule 13).
            IoDirection::Input => {
                let kept: Vec<HardwareLatency> = self
                    .hardware_latency
                    .iter()
                    .filter(|m| !(m.ret.eq_ignore_ascii_case(name) && self.returns_through(&m.input_device)))
                    .cloned()
                    .collect();
                self.hardware_latency = kept;
                let kept: Vec<ExternalLatency> = self
                    .external_latency
                    .iter()
                    .filter(|m| !(m.ret.eq_ignore_ascii_case(name) && self.returns_through(&m.input_device)))
                    .cloned()
                    .collect();
                self.external_latency = kept;
            }
        }
        Ok(())
    }

    /// The measured round trip, in frames, of a hardware insert sending on `send` and returning on
    /// `ret` at session rate `rate` through the input open now ([`Self::returns_through`]); `None`
    /// when that pair was not measured at that rate through this input.
    #[must_use]
    pub fn hardware_latency(&self, send: &str, ret: &str, rate: u32) -> Option<u32> {
        self.hardware_latency
            .iter()
            .find(|m| m.rate == rate && m.send.eq_ignore_ascii_case(send) && m.ret.eq_ignore_ascii_case(ret) && self.returns_through(&m.input_device))
            .map(|m| m.round_trip_frames)
    }

    /// Whether the pair has a figure at `rate` that does not apply to the input open now (it came
    /// back through another input, or its input is not established): "measure again".
    #[must_use]
    pub fn hardware_latency_elsewhere(&self, send: &str, ret: &str, rate: u32) -> bool {
        self.hardware_latency(send, ret, rate).is_none()
            && self.hardware_latency.iter().any(|m| m.rate == rate && m.send.eq_ignore_ascii_case(send) && m.ret.eq_ignore_ascii_case(ret))
    }

    /// Store a measurement, replacing the one for the same pair, rate and return input (a figure
    /// through another input stays, for when that input is open again).
    pub fn set_hardware_latency(&mut self, m: HardwareLatency) {
        let input = self.figure_input(&m.input_device).to_string();
        let same = |o: &HardwareLatency| {
            o.rate == m.rate
                && o.send.eq_ignore_ascii_case(&m.send)
                && o.ret.eq_ignore_ascii_case(&m.ret)
                && self.figure_input(&o.input_device) == input
        };
        let kept: Vec<HardwareLatency> = self.hardware_latency.iter().filter(|o| !same(o)).cloned().collect();
        self.hardware_latency = kept;
        self.hardware_latency.push(m);
    }

    /// Forget the measurements of the pair (every rate). Returns whether there were any.
    pub fn clear_hardware_latency(&mut self, send: &str, ret: &str) -> bool {
        let before = self.hardware_latency.len();
        self.hardware_latency.retain(|o| !(o.send.eq_ignore_ascii_case(send) && o.ret.eq_ignore_ascii_case(ret)));
        self.hardware_latency.len() != before
    }

    /// Store an External Instrument measurement, replacing the one with the same key and return
    /// input.
    pub fn set_external_latency(&mut self, m: ExternalLatency) {
        let input = self.figure_input(&m.input_device).to_string();
        let kept: Vec<ExternalLatency> =
            self.external_latency.iter().filter(|o| !(o.same_key(&m) && self.figure_input(&o.input_device) == input)).cloned().collect();
        self.external_latency = kept;
        self.external_latency.push(m);
    }

    /// Forget the External Instrument measurements of `port` returning on `ret` (every rate and
    /// buffer). Returns whether there were any.
    pub fn clear_external_latency(&mut self, port: &MidiOutPort, ret: &str) -> bool {
        let before = self.external_latency.len();
        self.external_latency.retain(|o| !o.chain_is(port, ret));
        self.external_latency.len() != before
    }

    /// The measurement for `port` returning on `ret` at `rate` with the device running `buffer`
    /// frames: the exact key's, else the newest of the same port and return (decision 4, cases 2
    /// and 3). `None` when the chain was never measured on this machine.
    #[must_use]
    pub fn external_measurement(&self, port: &MidiOutPort, ret: &str, rate: u32, buffer: Option<u32>) -> Option<ExternalLookup> {
        // Only a figure whose return came through the input open now (amendment for #431, rule 13).
        let chain = || self.external_latency.iter().filter(|m| m.chain_is(port, ret) && self.returns_through(&m.input_device));
        // A device that does not say its buffer is keyed 0, as it was when measured.
        let buffer = buffer.unwrap_or(0);
        if let Some(m) = chain().rfind(|m| m.rate == rate && m.buffer == buffer) {
            return Some(ExternalLookup { frames: m.frames, source: ExternalSource::Measured(m.clone()) });
        }
        // The newest; on equal times the one stored last.
        let m = chain().fold(None::<&ExternalLatency>, |best, m| match best {
            Some(b) if b.measured_at > m.measured_at => Some(b),
            _ => Some(m),
        })?;
        Some(ExternalLookup { frames: m.frames_at(rate), source: ExternalSource::Stale(m.clone()) })
    }

    /// Whether `port` returning on `ret` was measured, but only through another input than the
    /// one open now (or one not established): the card says "measure again".
    #[must_use]
    pub fn external_latency_elsewhere(&self, port: &MidiOutPort, ret: &str) -> bool {
        self.external_measurement(port, ret, 0, None).is_none() && self.external_latency.iter().any(|m| m.chain_is(port, ret))
    }

    /// The latency C of `t`'s External Instrument at `rate`, by decision 4's lookup: `manual_ms`
    /// when set, else this machine's measurement of the track's port and return (exact key, else
    /// the newest, rescaled to `rate`), else `None` (not measured). `None` too when `t` holds no
    /// External Instrument, or (with no manual value) sends to no external port.
    #[must_use]
    pub fn external_latency_of(&self, t: &Track, rate: u32) -> Option<ExternalLookup> {
        let e = t.external_instrument()?;
        if let Some(frames) = e.manual_frames(rate) {
            return Some(ExternalLookup { frames, source: ExternalSource::Manual });
        }
        let MidiOutput::External { port, .. } = &t.midi_output else { return None };
        if e.ret.is_empty() {
            return None;
        }
        self.external_measurement(port, &e.ret, rate, self.buffer)
    }

    /// Choose the output path Main plays on.
    pub fn set_main(&mut self, name: &str) -> Result<(), IoError> {
        let p = self.path(name).filter(|p| p.direction == IoDirection::Output).ok_or_else(|| IoError::Missing(name.into()))?;
        if p.live {
            return Err(IoError::LiveMain);
        }
        self.main = p.name.clone();
        Ok(())
    }

    /// The device channels Main plays on: its path's, else channels 0 and 1 (0 on a mono device).
    pub fn main_channels(&self) -> Vec<u16> {
        match self.path(&self.main).filter(|p| p.direction == IoDirection::Output) {
            Some(p) => p.channels.clone(),
            None => (0..self.outputs.clamp(1, 2)).collect(),
        }
    }

    /// Adopt the output count a device opened with: output paths that no longer fit are dropped,
    /// and a table left without its Main path gets the default output paths back. The input half
    /// is not touched: it follows the input device ([`crate::adopt_input`], amendment for #431,
    /// rules 8 and 11). Returns whether anything changed.
    pub fn reconcile_outputs(&mut self, outputs: u16) -> bool {
        let outputs = outputs.min(MAX_DEVICE_CHANNELS as u16);
        if outputs == self.outputs {
            return false;
        }
        self.outputs = outputs;
        self.paths.retain(|p| p.direction == IoDirection::Input || p.channels.iter().all(|&c| c < outputs));
        if self.paths_of(IoDirection::Output).all(|p| !p.name.eq_ignore_ascii_case(&self.main)) {
            let fresh = Self::for_device(&self.system, &self.device, 0, outputs);
            let at = self.paths.iter().position(|p| p.direction == IoDirection::Input).unwrap_or(self.paths.len());
            let missing: Vec<IoPath> = fresh.paths.into_iter().filter(|p| self.path(&p.name).is_none()).collect();
            self.paths.splice(at..at, missing);
            self.main = fresh.main;
            // A former cue can become the fallback Main when the device loses output channels.
            self.paths
                .iter_mut()
                .filter(|p| p.direction == IoDirection::Output && p.name.eq_ignore_ascii_case(&self.main))
                .for_each(|p| p.live = false);
        }
        true
    }

    /// Pairs of output paths that share a device channel (they sum there), with the first shared
    /// channel (0-based): "Out 3-4 shares channel 3 with Headphones". Inputs may overlap freely
    /// ("In 1" and "In 1-2" read the same channel), so they are not reported.
    pub fn overlaps(&self) -> Vec<(String, String, u16)> {
        let mut found = Vec::new();
        for (i, a) in self.paths.iter().enumerate() {
            for b in &self.paths[i + 1..] {
                if a.direction == IoDirection::Output
                    && b.direction == IoDirection::Output
                    && let Some(&c) = a.channels.iter().find(|c| b.channels.contains(c))
                {
                    found.push((a.name.clone(), b.name.clone(), c));
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(m: &IoMap, d: IoDirection) -> Vec<&str> {
        m.paths_of(d).map(|p| p.name.as_str()).collect()
    }

    fn out(name: &str, channels: Vec<u16>) -> IoPath {
        IoPath { live: false, name: name.into(), direction: IoDirection::Output, channels, format: ChannelFormat::Stereo }
    }

    #[test]
    fn default_for_two_channels_is_one_main_pair() {
        let m = IoMap::for_device("ASIO", "Scarlett 2i2", 2, 2);
        assert_eq!(names(&m, IoDirection::Output), ["Out 1-2"]);
        assert_eq!(names(&m, IoDirection::Input), ["In 1", "In 2", "In 1-2"]);
        assert_eq!(m.main, "Out 1-2");
        assert_eq!(m.main_channels(), [0, 1]);
        assert_eq!(m.path("Out 1-2").unwrap().format, ChannelFormat::Stereo);
    }

    #[test]
    fn default_for_four_and_ten_channels_adds_a_pair_per_two_channels() {
        let m = IoMap::for_device("ASIO", "x", 4, 4);
        assert_eq!(names(&m, IoDirection::Output), ["Out 1-2", "Out 3-4"]);
        assert_eq!(m.path("Out 3-4").unwrap().channels, [2, 3]);
        assert_eq!(names(&m, IoDirection::Input), ["In 1", "In 2", "In 3", "In 4", "In 1-2", "In 3-4"]);
        let m = IoMap::for_device("ASIO", "TASCAM", 12, 10);
        assert_eq!(names(&m, IoDirection::Output), ["Out 1-2", "Out 3-4", "Out 5-6", "Out 7-8", "Out 9-10"]);
        assert_eq!(m.paths_of(IoDirection::Input).count(), 12 + 6);
    }

    #[test]
    fn default_for_a_mono_device_is_one_mono_main() {
        let m = IoMap::for_device("WASAPI", "x", 0, 1);
        assert_eq!(names(&m, IoDirection::Output), ["Out 1"]);
        assert_eq!(m.main_channels(), [0]);
        assert_eq!(m.path("Out 1").unwrap().format, ChannelFormat::Mono);
    }

    #[test]
    fn default_for_34_channels_applies_the_cap_and_every_path_validates() {
        let m = IoMap::for_device("ASIO", "big", 34, 34);
        assert_eq!((m.inputs, m.outputs), (32, 32));
        assert_eq!(m.paths_of(IoDirection::Output).count(), 16);
        assert_eq!(m.path("Out 31-32").unwrap().channels, [30, 31]);
        assert!(m.path("Out 33-34").is_none() && m.path("In 33").is_none());
        assert!(m.paths.iter().all(|p| m.validate(p).is_ok()));
    }

    #[test]
    fn an_odd_trailing_output_is_mono() {
        let m = IoMap::for_device("ASIO", "x", 0, 5);
        assert_eq!(names(&m, IoDirection::Output), ["Out 1-2", "Out 3-4", "Out 5"]);
    }

    /// #429 review: the engine refuses a path left without channels through `set_channels` too,
    /// and keeps the path as it was.
    #[test]
    fn set_channels_refuses_an_empty_list_and_keeps_the_path() {
        let mut m = IoMap::for_device("ASIO", "x", 4, 4);
        assert_eq!(m.set_channels("Out 3-4", Vec::new()), Err(IoError::Width(0)));
        assert_eq!(m.path("Out 3-4").map(|p| p.channels.clone()), Some(vec![2, 3]));
    }

    #[test]
    fn validation_refuses_width_range_and_duplicate_names() {
        let mut m = IoMap::for_device("ASIO", "x", 4, 4);
        assert_eq!(m.add_path(out("Empty", vec![])), Err(IoError::Width(0)));
        assert_eq!(m.add_path(out("Wide", (0..17).collect())), Err(IoError::Width(17)));
        assert_eq!(m.add_path(out("Far", vec![0, 4])), Err(IoError::OutOfRange { channel: 4, opened: 4 }));
        assert_eq!(m.add_path(out(" ", vec![0])), Err(IoError::EmptyName));
        assert_eq!(m.add_path(out("Twice", vec![1, 1])), Err(IoError::Repeated(1)));
        assert_eq!(m.add_path(out("out 3-4", vec![2])), Err(IoError::Duplicate("out 3-4".into())));
        assert_eq!(m.add_path(out("In 1", vec![2])), Err(IoError::Duplicate("In 1".into())), "one namespace across directions");
        m.add_path(out("Phones", vec![3, 1])).unwrap();
        assert_eq!(m.path("Phones").unwrap().channels, [3, 1], "non-contiguous is allowed");
        let wide = IoMap::for_device("ASIO", "x", 0, 32);
        assert!(wide.validate(&out("All", (0..16).collect())).is_ok());
    }

    #[test]
    fn edits_rename_main_and_refuse_removing_it() {
        let mut m = IoMap::for_device("ASIO", "x", 0, 4);
        assert_eq!(m.remove_path("Out 1-2"), Err(IoError::MainPath));
        m.rename_path("Out 1-2", "Monitors").unwrap();
        assert_eq!(m.main, "Monitors");
        assert_eq!(m.rename_path("Out 3-4", "monitors"), Err(IoError::Duplicate("monitors".into())));
        m.set_channels("Out 3-4", vec![3]).unwrap();
        assert_eq!(m.path("Out 3-4").unwrap().format, ChannelFormat::Mono);
        m.set_main("Out 3-4").unwrap();
        assert_eq!(m.main_channels(), [3]);
        m.remove_path("Monitors").unwrap();
        assert_eq!(m.remove_path("Monitors"), Err(IoError::Missing("Monitors".into())));
    }

    #[test]
    fn overlapping_output_paths_are_reported() {
        let mut m = IoMap::for_device("ASIO", "x", 2, 4);
        assert!(m.overlaps().is_empty());
        m.add_path(out("Headphones", vec![2])).unwrap();
        assert_eq!(m.overlaps(), [("Out 3-4".to_string(), "Headphones".to_string(), 2)]);
    }

    #[test]
    fn a_new_channel_count_keeps_what_fits_and_rebuilds_a_missing_main() {
        let mut m = IoMap::for_device("ASIO", "TASCAM", 12, 10);
        m.add_path(out("Mine", vec![0, 1])).unwrap();
        assert!(!m.reconcile_outputs(10));
        assert!(m.reconcile_outputs(2));
        assert_eq!((m.inputs, m.outputs), (12, 2), "the input half is the input device's, never touched here");
        assert!(m.path("Out 5-6").is_none() && m.path("In 7").is_some());
        assert!(m.path("Mine").is_some() && m.path("Out 1-2").is_some());
        let mut moved = IoMap::for_device("ASIO", "x", 2, 4);
        moved.set_main("Out 3-4").unwrap();
        moved.reconcile_outputs(2);
        assert_eq!(moved.main, "Out 1-2");
        assert_eq!(names(&moved, IoDirection::Output), ["Out 1-2"]);
        assert_eq!(names(&moved, IoDirection::Input), ["In 1", "In 2", "In 1-2"], "a Main repair adds no input paths");
    }

    #[test]
    fn input_names_resolve_through_the_table_and_fall_back_to_the_label() {
        let mut m = IoMap::for_device("ASIO", "x", 8, 2);
        m.add_path(IoPath { live: false, name: "Guitar".into(), direction: IoDirection::Input, channels: vec![4], format: ChannelFormat::Mono })
            .unwrap();
        m.add_path(IoPath { live: false, name: "Pair".into(), direction: IoDirection::Input, channels: vec![6, 1], format: ChannelFormat::Stereo })
            .unwrap();
        assert_eq!(input_channel(Some(&m), "Guitar", 0, 2), Some(4));
        assert_eq!(input_channel(Some(&m), "guitar", 1, 2), Some(4), "a stereo strip on a mono path hears it on both sides (#681)");
        assert_eq!((input_channel(Some(&m), "Pair", 0, 2), input_channel(Some(&m), "Pair", 1, 2)), (Some(6), Some(1)));
        assert_eq!(input_channel(Some(&m), "In 3", 0, 2), Some(2));
        assert_eq!(input_channel(Some(&m), "In 3-4", 1, 2), Some(3));
        // Not in the table: the number in the label, as before.
        assert_eq!(input_channel(Some(&m), "In 12", 0, 2), Some(11));
        assert_eq!(input_channel(Some(&m), "In 12", 1, 2), Some(11), "a mono label feeds both sides too");
        assert_eq!(input_channel(None, "In 3-4", 1, 2), Some(3));
        assert_eq!(input_channel(None, "Guitar", 0, 2), Some(0));
        // An output path is not an input (and no input path has its name): the label rule, as before.
        assert_eq!(input_channel(Some(&m), "Out 1-2", 0, 2), Some(0));
    }

    /// #681: a one-channel input gets its default "In 1" however the table arrives (opened with
    /// it, narrowed from two, or given the count later), and a stereo strip on a mono path reads
    /// that one channel on both sides, on one input and on two.
    #[test]
    fn a_one_channel_input_has_its_default_path_and_a_stereo_strip_hears_it_on_both_sides() {
        let one = IoMap::for_device("test", "x", 1, 2);
        assert_eq!(names(&one, IoDirection::Input), ["In 1"]);
        let (late, _) = crate::adopt_input(&IoMap::for_device("test", "x", 0, 2), &[], &[], "x", 1);
        assert_eq!(names(&late, IoDirection::Input), ["In 1"]);
        let both =
            |m: &IoMap, name: &str, live: usize| (live_input_channel(Some(m), name, 0, 2, live), live_input_channel(Some(m), name, 1, 2, live));
        assert_eq!(both(&one, "In 1", 1), (Some(0), Some(0)));
        let two = IoMap::for_device("test", "x", 2, 2);
        assert_eq!(both(&two, "In 2", 2), (Some(1), Some(1)), "In 2 on both sides, never In 2 and a wrap to In 1");
        assert_eq!(both(&two, "In 1", 2), (Some(0), Some(0)));
        assert_eq!(both(&two, "In 1-2", 2), (Some(0), Some(1)), "a stereo path is unchanged");
        // Wider than stereo: unchanged, a mono path or label continues past its channel.
        let eight = IoMap::for_device("test", "x", 8, 2);
        let all = |name: &str, strip: usize| (0..strip).map(|k| input_channel(Some(&eight), name, k, strip).unwrap()).collect::<Vec<_>>();
        assert_eq!(all("In 3", 4), [2, 3, 4, 5], "a quad track on In 3, as before #681");
        assert_eq!(all("In 12", 4), [11, 12, 13, 14], "a mono label not in the table, as before");
        assert_eq!(all("In 3", 2), [2, 2]);
    }

    #[test]
    fn system_audio_resolves_to_its_pair_after_the_device_inputs_and_is_silent_when_off() {
        let mut m = IoMap::for_device("ASIO", "x", 2, 2);
        assert_eq!(live_input_channel(Some(&m), "System Audio", 0, 2, 2), None, "off: silent, never In 1");
        m.system_audio = Some(2);
        assert_eq!(
            (live_input_channel(Some(&m), "System Audio", 0, 2, 4), live_input_channel(Some(&m), "system audio", 1, 2, 4)),
            (Some(2), Some(3))
        );
        assert_eq!(live_input_channel(Some(&m), "System Audio", 0, 2, 2), None, "the live input lacks the pair (it did not open)");
        assert_eq!(live_input_channel(None, "System Audio", 0, 2, 4), None);
        assert_eq!(live_input_channel(Some(&m), "In 2", 0, 2, 4), Some(1), "device paths resolve as before");
        assert_eq!(live_input_channel(Some(&m), "In 3", 0, 2, 2), Some(0), "and wrap to the live channels as before");
        let path = |name: &str| IoPath {
            live: false,
            name: name.into(),
            direction: IoDirection::Input,
            channels: vec![0, 1],
            format: ChannelFormat::Stereo,
        };
        assert_eq!(m.add_path(path(" system audio ")), Err(IoError::Reserved("system audio".into())));
        assert_eq!(m.rename_path("In 1-2", "System Audio"), Err(IoError::Reserved("System Audio".into())));
        let saved = serde_json::to_string(&m).unwrap();
        assert!(!saved.contains("system_audio"), "a runtime fact, not part of the saved table");
    }

    #[test]
    fn hardware_latency_is_per_pair_and_rate_and_follows_removed_paths() {
        let mut m = IoMap::for_device("ASIO", "x", 4, 4);
        assert_eq!(m.hardware_latency("Out 3-4", "In 3-4", 48_000), None);
        let at = |frames, rate| HardwareLatency {
            send: "Out 3-4".into(),
            ret: "In 3-4".into(),
            round_trip_frames: frames,
            rate,
            measured_at: 7,
            input_device: String::new(),
        };
        m.set_hardware_latency(at(300, 48_000));
        m.set_hardware_latency(at(500, 44_100));
        m.set_hardware_latency(at(310, 48_000));
        assert_eq!(m.hardware_latency("out 3-4", "IN 3-4", 48_000), Some(310), "a new measurement replaces the old");
        assert_eq!(m.hardware_latency("Out 3-4", "In 3-4", 44_100), Some(500));
        assert_eq!(m.hardware_latency("Out 3-4", "In 1-2", 48_000), None);
        let back: IoMap = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        let mut gone = m.clone();
        gone.remove_path("In 3-4").unwrap();
        assert!(gone.hardware_latency.is_empty());
        assert!(m.clear_hardware_latency("Out 3-4", "In 3-4") && !m.clear_hardware_latency("Out 3-4", "In 3-4"));
    }

    fn ext(port: &str, ret: &str, rate: u32, buffer: u32, frames: u32, at: u64) -> ExternalLatency {
        ExternalLatency {
            port_id: String::new(),
            port_name: port.into(),
            ret: ret.into(),
            rate,
            buffer,
            frames,
            spread_frames: 19,
            notes: 8,
            measured_at: at,
            input_device: String::new(),
        }
    }

    fn port(name: &str, id: &str) -> MidiOutPort {
        MidiOutPort { backend: "winmm".into(), name: name.into(), ordinal: 0, id: id.into() }
    }

    /// ADR 0020 decisions 4 and 5: the key is port, return, rate and buffer; another buffer is used
    /// as is and another rate rescaled by time, both labeled; the table is not the hardware one.
    #[test]
    fn external_latency_is_keyed_by_port_return_rate_and_buffer() {
        let mut m = IoMap::for_device("ASIO", "Scarlett", 2, 2);
        let d = port("MODEL D", "");
        assert_eq!(m.external_measurement(&d, "In 1", 48_000, Some(64)), None);
        m.set_external_latency(ext("MODEL D", "In 1", 48_000, 64, 955, 10));
        m.set_external_latency(ext("MODEL D", "In 1", 48_000, 128, 1039, 20));
        m.set_external_latency(ext("model d", "in 1", 48_000, 64, 956, 30));
        assert_eq!(m.external_latency.len(), 2, "the same key replaces");
        let exact = m.external_measurement(&d, "In 1", 48_000, Some(64)).unwrap();
        assert_eq!((exact.frames, matches!(exact.source, ExternalSource::Measured(_))), (956, true));
        let other = m.external_measurement(&d, "In 1", 48_000, Some(128)).unwrap();
        assert_eq!((other.frames, matches!(other.source, ExternalSource::Measured(_))), (1039, true));
        // Another buffer: the newest (956 at 64, measured last), as is, labeled.
        let stale = m.external_measurement(&d, "In 1", 48_000, Some(256)).unwrap();
        assert_eq!(stale.frames, 956);
        assert!(matches!(&stale.source, ExternalSource::Stale(r) if r.buffer == 64));
        // Another rate: rescaled by time (1061 at 48 kHz is 975 at 44.1 kHz).
        m.set_external_latency(ext("MODEL D", "In 1", 48_000, 64, 1061, 40));
        let rescaled = m.external_measurement(&d, "In 1", 44_100, Some(64)).unwrap();
        assert_eq!(rescaled.frames, 975);
        assert!(matches!(rescaled.source, ExternalSource::Stale(_)));
        // Another port or return: nothing.
        assert_eq!(m.external_measurement(&port("Minilogue", ""), "In 1", 48_000, Some(64)), None);
        assert_eq!(m.external_measurement(&d, "In 2", 48_000, Some(64)), None);
        assert!(m.hardware_latency.is_empty(), "a separate table");
        // A port id wins over the name when both have one.
        let mut byid = IoMap::for_device("ASIO", "Scarlett", 2, 2);
        byid.set_external_latency(ExternalLatency { port_id: "usb-1".into(), ..ext("MODEL D", "In 1", 48_000, 64, 900, 1) });
        assert!(byid.external_measurement(&port("Renamed", "usb-1"), "In 1", 48_000, Some(64)).is_some());
        assert!(byid.external_measurement(&port("MODEL D", "usb-2"), "In 1", 48_000, Some(64)).is_none());
        // Saved with the table; the open buffer is not.
        m.buffer = Some(64);
        let back: IoMap = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!((back.external_latency.clone(), back.buffer), (m.external_latency.clone(), None));
        let mut gone = m.clone();
        gone.remove_path("In 1").unwrap();
        assert!(gone.external_latency.is_empty(), "a removed return takes its measurements");
        assert!(m.clear_external_latency(&d, "In 1") && m.external_latency.is_empty());
    }

    /// Decision 4's lookup on a track: manual wins, then the table at the open buffer.
    #[test]
    fn a_tracks_latency_is_manual_then_measured() {
        use crate::{Insert, MidiOutChannel, TrackId, TrackKind};
        let mut t = Track::new(TrackId(1), "Model D", TrackKind::Instrument, ChannelFormat::Stereo, [0; 3]);
        t.instrument = Some(Insert::external("In 1"));
        t.midi_output = MidiOutput::External { port: port("MODEL D", ""), channel: MidiOutChannel::Channel(0), offset_ms: 0.0 };
        let mut m = IoMap::for_device("ASIO", "Scarlett", 2, 2);
        m.buffer = Some(64);
        assert_eq!(m.external_latency_of(&t, 48_000), None, "not measured");
        m.set_external_latency(ext("MODEL D", "In 1", 48_000, 64, 955, 1));
        assert_eq!(m.external_latency_of(&t, 48_000).unwrap().frames, 955);
        m.buffer = Some(128);
        assert!(matches!(m.external_latency_of(&t, 48_000).unwrap().source, ExternalSource::Stale(_)), "the buffer changed");
        if let Some(e) = t.instrument.as_mut().and_then(|i| i.external.as_mut()) {
            e.manual_ms = Some(20.0);
        }
        assert_eq!(m.external_latency_of(&t, 48_000), Some(ExternalLookup { frames: 960, source: ExternalSource::Manual }), "manual wins");
        t.midi_output = MidiOutput::Internal;
        t.instrument = Some(Insert::external("In 1"));
        assert_eq!(m.external_latency_of(&t, 48_000), None, "no port, no measurement");
    }

    #[test]
    fn serde_round_trips_and_old_files_default() {
        let m = IoMap::for_device("ASIO", "TASCAM", 12, 10);
        let back: IoMap = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        let empty: IoMap = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, IoMap::default());
    }
}
