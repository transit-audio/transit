//! The device model (ADR 0021, decision 3): JACK has no devices. The server is the system, a port
//! group is the device (the owner of a set of physical ports, by the client part of their names:
//! `system` under JACK 2, one entry per device node under PipeWire), and its ports are the channels.

use crate::api::{ClientError, PhysicalPort};
use std::sync::Arc;
use transit_audio_backend as ab;

/// The smallest and largest quantum the buffer size list offers (the server decides which it takes).
pub const MIN_QUANTUM: u32 = 64;
pub const MAX_QUANTUM: u32 = 2048;

/// One port group: the device an endpoint carries as its handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JackDevice {
    /// The client part of the ports' names ("system", "Scarlett 2i2 USB Pro").
    pub name: String,
    /// Full names of its capture ports (audio from the hardware), in the server's order.
    pub capture: Vec<String>,
    /// Full names of its playback ports (audio to the hardware), in the server's order.
    pub playback: Vec<String>,
    /// The server's rate and quantum when it was listed.
    pub rate: u32,
    pub buffer: u32,
    /// A virtual device (ADR 0015, amendment for #431, rule 1): output ports the server does not
    /// mark physical, of a client with no physical output port, offered as an input only. Its
    /// ports can change between opens (rule 5), so its ordered port names guard its paths.
    pub virtual_device: bool,
}

impl JackDevice {
    /// The ports of `direction`: capture for input, playback for output.
    pub fn ports(&self, direction: ab::Direction) -> &[String] {
        match direction {
            ab::Direction::Input => &self.capture,
            ab::Direction::Output => &self.playback,
        }
    }

    /// The channel names of `direction`: the ports' short names ("capture_1", "playback_AUX0").
    pub fn channel_names(&self, direction: ab::Direction) -> Vec<String> {
        self.ports(direction).iter().map(|p| p.split_once(':').map_or(p.as_str(), |(_, short)| short).to_owned()).collect()
    }

    /// The endpoint of this group. One format: `f32` at the server's rate, every port; the quantum may be asked for.
    pub fn endpoint(&self, direction: ab::Direction) -> ab::Endpoint {
        let channels = clamp(self.ports(direction).len());
        let format = ab::FormatRange {
            sample: ab::SampleFormat::F32,
            channels,
            min_rate: self.rate,
            max_rate: self.rate,
            // The standard sizes the server can be asked for, and the one it runs if outside them.
            period: Some((MIN_QUANTUM.min(self.buffer), MAX_QUANTUM.max(self.buffer))),
        };
        ab::Endpoint::new(
            self.name.clone(),
            self.name.clone(),
            crate::SYSTEM.into(),
            clamp(self.capture.len()),
            clamp(self.playback.len()),
            vec![format],
            Arc::new(self.clone()),
        )
    }
}

fn clamp(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// A running server as a probe client saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub rate: u32,
    pub buffer: u32,
    /// The port groups of the physical ports, in the order their first port appeared.
    pub devices: Vec<JackDevice>,
    /// The eligible virtual devices ([`virtual_groups`]), capture ports only, in the order their
    /// first port appeared. Listed whatever the preference: it filters only what the Input list
    /// offers, and a saved name resolves among them always (rule 2).
    pub virtual_devices: Vec<JackDevice>,
}

impl Server {
    /// The endpoints of the physical groups with ports in `direction` (today's lists).
    pub fn endpoints(&self, direction: ab::Direction) -> Vec<ab::Endpoint> {
        self.devices.iter().filter(|d| !d.ports(direction).is_empty()).map(|d| d.endpoint(direction)).collect()
    }

    /// The input endpoints of the virtual devices.
    pub fn virtual_endpoints(&self) -> Vec<ab::Endpoint> {
        self.virtual_devices.iter().map(|d| d.endpoint(ab::Direction::Input)).collect()
    }

    /// The input named `name` among every eligible group (rules 1 and 2): a physical group with
    /// capture ports, else a virtual device. A name that is neither, such as a client transit owns
    /// or a group that only plays, is not an input.
    pub fn input(&self, name: &str) -> Option<&JackDevice> {
        let physical = self.devices.iter().find(|d| d.name == name && !d.capture.is_empty());
        physical.or_else(|| self.virtual_devices.iter().find(|d| d.name == name))
    }
}

/// What enumeration found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerState {
    /// `libjack.so.0` did not load (the loader's text, for the log).
    LibraryMissing(String),
    /// No server answered; none was started.
    NotRunning,
    /// The server refused the probe client for another reason.
    Failed(String),
    Running(Server),
}

impl ServerState {
    /// What the Audio Devices panel says about the system, when it is not usable.
    pub fn diagnostic(&self) -> Option<String> {
        match self {
            ServerState::LibraryMissing(_) => Some(crate::LIBRARY_MISSING.into()),
            ServerState::NotRunning => Some(crate::SERVER_NOT_RUNNING.into()),
            ServerState::Failed(text) => Some(format!("JACK: {text}")),
            ServerState::Running(_) => None,
        }
    }

    pub(crate) fn from_error(error: ClientError) -> Self {
        match error {
            ClientError::LibraryMissing(text) => ServerState::LibraryMissing(text),
            ClientError::ServerNotRunning => ServerState::NotRunning,
            ClientError::Failed(text) => ServerState::Failed(text),
        }
    }
}

/// Groups physical ports into devices by the client part of their names, in order of first
/// appearance; each group keeps its capture and playback ports in the server's order.
pub(crate) fn group(ports: &[PhysicalPort], rate: u32, buffer: u32) -> Vec<JackDevice> {
    let mut devices: Vec<JackDevice> = Vec::new();
    for port in ports {
        let index = match devices.iter().position(|d| d.name == port.group()) {
            Some(index) => index,
            None => {
                devices.push(JackDevice {
                    name: port.group().to_owned(),
                    capture: Vec::new(),
                    playback: Vec::new(),
                    rate,
                    buffer,
                    virtual_device: false,
                });
                devices.len() - 1
            }
        };
        let device = &mut devices[index];
        if port.capture { device.capture.push(port.name.clone()) } else { device.playback.push(port.name.clone()) }
    }
    devices
}

/// The client part of a full port name: before the first `:`.
fn client_of(port: &str) -> &str {
    port.split_once(':').map_or(port, |(client, _)| client)
}

/// The eligible virtual devices (ADR 0015, amendment for #431, rule 1's one eligibility rule): the
/// audio output ports in `outputs` that are not in `physical`, grouped by client in order of first
/// appearance, keeping the server's port order. A client with any physical output port gets no
/// virtual device (physical and non-physical ports are never mixed), and a client whose name is in
/// `owned` (transit's session and probe clients, by the names the server assigned) gets none.
/// MIDI ports are not in `outputs` (the listing asks for the audio type).
pub(crate) fn virtual_groups(outputs: &[String], physical: &[PhysicalPort], owned: &[String], rate: u32, buffer: u32) -> Vec<JackDevice> {
    let captures: Vec<&str> = physical.iter().filter(|p| p.capture).map(|p| p.name.as_str()).collect();
    let mut devices: Vec<JackDevice> = Vec::new();
    for port in outputs.iter().filter(|p| !captures.contains(&p.as_str())) {
        let client = client_of(port);
        if captures.iter().any(|c| client_of(c) == client) || owned.iter().any(|o| o == client) {
            continue;
        }
        match devices.iter_mut().find(|d| d.name == client) {
            Some(device) => device.capture.push(port.clone()),
            None => devices.push(JackDevice {
                name: client.to_owned(),
                capture: vec![port.clone()],
                playback: Vec::new(),
                rate,
                buffer,
                virtual_device: true,
            }),
        }
    }
    devices
}
