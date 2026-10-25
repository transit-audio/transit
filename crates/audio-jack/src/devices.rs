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
    /// The port groups, in the order their first port appeared.
    pub devices: Vec<JackDevice>,
}

impl Server {
    /// The endpoints of the groups with ports in `direction`.
    pub fn endpoints(&self, direction: ab::Direction) -> Vec<ab::Endpoint> {
        self.devices.iter().filter(|d| !d.ports(direction).is_empty()).map(|d| d.endpoint(direction)).collect()
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
                devices.push(JackDevice { name: port.group().to_owned(), capture: Vec::new(), playback: Vec::new(), rate, buffer });
                devices.len() - 1
            }
        };
        let device = &mut devices[index];
        if port.capture { device.capture.push(port.name.clone()) } else { device.playback.push(port.name.clone()) }
    }
    devices
}
