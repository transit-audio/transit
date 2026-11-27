//! What the scan's per-class capability probe reads from a created and initialized CLAP plug-in
//! (ADR 0064, decision 2.4): its note ports in both directions with each port's dialects, and its
//! audio output port count. Main-thread calls on an inactive instance; nothing is activated,
//! processed or shown. Runs in a probe child process.

use super::*;

/// Most note ports read per direction (hostile-input cap).
const MAX_NOTE_PORTS: u32 = 64;

impl Instance {
    /// The supported dialects (`CLAP_NOTE_DIALECT_*` bits) of every note port in one direction, in
    /// port order; a port whose info cannot be read reports none. Empty without `clap.note-ports`
    /// ("If the plugin does not implement this extension, it won't have note input or output").
    pub fn note_port_dialects(&self, is_input: bool) -> Vec<u32> {
        // SAFETY: `notes` is null or the plug-in's extension struct, valid for the instance's life.
        let Some(p) = (unsafe { self.notes.as_ref() }) else { return Vec::new() };
        let (Some(count), Some(get)) = (p.count, p.get) else { return Vec::new() };
        // SAFETY: a vtable call with its own initialized instance, on the thread that created it
        // (`count` is a main-thread call).
        let n = unsafe { count(self.plugin, is_input) }.min(MAX_NOTE_PORTS);
        (0..n)
            .map(|i| {
                // SAFETY: plain data; all-zero is valid.
                let mut info: clap_note_port_info = unsafe { std::mem::zeroed() };
                // SAFETY: the index is below the count just read; `info` is a writable
                // `clap_note_port_info`; main thread, as for `count`.
                if unsafe { get(self.plugin, i, is_input, &mut info) } { info.supported_dialects } else { 0 }
            })
            .collect()
    }

    /// How many audio ports the plug-in declares in one direction; 0 without `clap.audio-ports`.
    pub fn audio_port_count(&self, is_input: bool) -> u32 {
        // SAFETY: `ports` is null or the plug-in's extension struct, valid for the instance's life.
        let Some(p) = (unsafe { self.ports.as_ref() }) else { return 0 };
        let Some(count) = p.count else { return 0 };
        // SAFETY: a vtable call with its own initialized instance, on the thread that created it
        // (`count` is a main-thread call).
        unsafe { count(self.plugin, is_input) }
    }
}
