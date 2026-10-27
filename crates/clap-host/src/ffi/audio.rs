//! Audio ports (`clap.audio-ports`): planar buffers and their pointer tables.

use super::*;

// ---- audio buffers -------------------------------------------------------------------------

/// Planar f32 buffers for every audio port in one direction, with the pointer tables CLAP needs.
/// Allocated in `new`; `process` only rewrites pointers in place (no allocation).
pub(crate) struct PortBuffers {
    data: Vec<Vec<Vec<f32>>>,
    ptrs: Vec<Vec<*mut f32>>,
    pub(super) bufs: Vec<clap_audio_buffer>,
    pub(super) frames: usize,
    /// Every channel holds a constant (silent) block: `refresh` sets each `constant_mask` to all ones.
    constant: bool,
    /// Per-port override set by [`PortBuffers::set_port_constant`]; wins over `constant` for that port.
    port_constant: Vec<Option<bool>>,
}

// SAFETY: the raw pointers only ever point into `data`, which this struct owns; moving the struct
// to another thread moves that ownership with it.
unsafe impl Send for PortBuffers {}

impl PortBuffers {
    /// One buffer set per port, `channels[p]` channels each (capped), `frames` long.
    pub fn new(channels: &[u32], frames: usize) -> PortBuffers {
        let data: Vec<Vec<Vec<f32>>> =
            channels.iter().take(MAX_PORTS as usize).map(|&c| (0..c.min(MAX_PORT_CHANNELS)).map(|_| vec![0.0; frames]).collect()).collect();
        let ptrs = data.iter().map(|p| vec![std::ptr::null_mut(); p.len()]).collect();
        let bufs = data
            .iter()
            .map(|_| clap_audio_buffer { data32: std::ptr::null_mut(), data64: std::ptr::null_mut(), channel_count: 0, latency: 0, constant_mask: 0 })
            .collect();
        let port_constant = vec![None; data.len()];
        PortBuffers { data, ptrs, bufs, frames, constant: false, port_constant }
    }

    pub fn channels(&self, port: usize) -> usize {
        self.data.get(port).map_or(0, Vec::len)
    }

    pub fn channel(&self, port: usize, ch: usize) -> Option<&[f32]> {
        self.data.get(port).and_then(|p| p.get(ch)).map(Vec::as_slice)
    }

    pub fn channel_mut(&mut self, port: usize, ch: usize) -> Option<&mut [f32]> {
        self.data.get_mut(port).and_then(|p| p.get_mut(ch)).map(Vec::as_mut_slice)
    }

    /// Marks every channel of the next block as constant (CLAP's way to say silent), or clears it.
    /// Also clears every per-port setting of [`PortBuffers::set_port_constant`].
    pub fn set_constant(&mut self, constant: bool) {
        self.constant = constant;
        self.port_constant.iter_mut().for_each(|p| *p = None);
    }

    /// Marks one port constant (silent), or clears it, whatever [`PortBuffers::set_constant`] said.
    pub fn set_port_constant(&mut self, port: usize, constant: bool) {
        if let Some(p) = self.port_constant.get_mut(port) {
            *p = Some(constant);
        }
    }

    /// The planar channels of one port.
    pub fn port_mut(&mut self, port: usize) -> Option<&mut [Vec<f32>]> {
        self.data.get_mut(port).map(Vec::as_mut_slice)
    }

    /// Points the CLAP tables at the current data (no allocation).
    pub(super) fn refresh(&mut self) {
        for (((d, p), b), own) in self.data.iter_mut().zip(self.ptrs.iter_mut()).zip(self.bufs.iter_mut()).zip(&self.port_constant) {
            let constant = own.unwrap_or(self.constant);
            for (dc, pc) in d.iter_mut().zip(p.iter_mut()) {
                *pc = dc.as_mut_ptr();
            }
            b.data32 = p.as_mut_ptr();
            b.data64 = std::ptr::null_mut();
            b.channel_count = u32::try_from(p.len()).unwrap_or(0);
            b.constant_mask = if constant { all_channels_mask(b.channel_count) } else { 0 };
        }
    }
}

/// A mask with one bit set per channel (`channels` low bits; all 64 from 64 channels up).
pub(crate) fn all_channels_mask(channels: u32) -> u64 {
    1u64.checked_shl(channels).map_or(u64::MAX, |b| b.wrapping_sub(1))
}
