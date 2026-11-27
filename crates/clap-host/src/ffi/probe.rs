//! What the scan's per-class capability probe reads from a created and initialized CLAP plug-in
//! (ADR 0064, decision 2.4): its note ports in both directions with each port's dialects, and its
//! audio output port count. Main-thread calls on an initialized, inactive instance (the probe
//! child's main thread made it and never activates it); nothing is activated, processed or shown.
//! Runs in a probe child process.

use super::*;

/// Most note ports read per direction: a guard against a hostile count, far above any real
/// plug-in. A plug-in declaring more gets an error (retried by another build), never a partial
/// answer.
pub(crate) const MAX_NOTE_PORTS: u32 = 4096;

impl Instance {
    /// The supported dialects (`CLAP_NOTE_DIALECT_*` bits) of every note port in one direction, in
    /// port order. Empty without `clap.note-ports` ("If the plugin does not implement this
    /// extension, it won't have note input or output"). Err when the extension lacks `count` or
    /// `get`, when the plug-in declares more than [`MAX_NOTE_PORTS`] ports, or when a port's
    /// information cannot be read: the answer would not be complete.
    pub fn note_port_dialects(&self, is_input: bool) -> Result<Vec<u32>, String> {
        let dir = if is_input { "input" } else { "output" };
        // SAFETY: `notes` is null or the plug-in's extension struct, valid for the instance's life.
        let Some(p) = (unsafe { self.notes.as_ref() }) else { return Ok(Vec::new()) };
        let (Some(count), Some(get)) = (p.count, p.get) else { return Err("clap.note-ports without count or get".into()) };
        // SAFETY: a vtable call with its own initialized, inactive instance, on the probe child's
        // main thread, which created it (`count` is a main-thread call).
        let n = unsafe { count(self.plugin, is_input) };
        if n > MAX_NOTE_PORTS {
            return Err(format!("declares {n} note {dir} ports, more than the {MAX_NOTE_PORTS} this host reads"));
        }
        (0..n)
            .map(|i| {
                // SAFETY: plain data; all-zero is valid.
                let mut info: clap_note_port_info = unsafe { std::mem::zeroed() };
                // SAFETY: the index is below the count just read; `info` is a writable
                // `clap_note_port_info`; main thread and inactive instance, as for `count`.
                if unsafe { get(self.plugin, i, is_input, &mut info) } {
                    Ok(info.supported_dialects)
                } else {
                    Err(format!("note {dir} port {i} of {n} could not be read"))
                }
            })
            .collect()
    }

    /// How many audio ports the plug-in declares in one direction; 0 without `clap.audio-ports`.
    /// Err when the extension lacks `count`.
    pub fn audio_port_count(&self, is_input: bool) -> Result<u32, String> {
        // SAFETY: `ports` is null or the plug-in's extension struct, valid for the instance's life.
        let Some(p) = (unsafe { self.ports.as_ref() }) else { return Ok(0) };
        let Some(count) = p.count else { return Err("clap.audio-ports without count".into()) };
        // SAFETY: a vtable call with its own initialized, inactive instance, on the probe child's
        // main thread, which created it (`count` is a main-thread call).
        Ok(unsafe { count(self.plugin, is_input) })
    }
}
