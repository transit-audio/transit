//! The plug-ins ADR 0064's scan probe is tested with (slice HM0), each set in a build of its own so
//! the default fixture and every test counting its plug-ins are unchanged:
//!
//! - feature `notes`: "Fixture Notes" (feature `instrument`: one stereo audio output port, an
//!   input note port with the CLAP and MIDI dialects, and two output note ports, port 0 MIDI 2
//!   only and port 1 CLAP and MIDI) and "Fixture Notes Silent" (`instrument`: one stereo audio
//!   output port, the same input note port, one output note port that speaks MIDI 2 only: no
//!   readable note output).
//! - feature `note-effects`: three zero-audio note effects, "Fixture Note Effect A", "B" and "C"
//!   (feature `note-effect`: one input and one output note port with the CLAP and MIDI dialects,
//!   no audio port, no GUI).
//! - feature `probe-failures`: "Fixture Notes" first, then a plug-in whose creation returns an
//!   error (the factory returns no instance, "Fixture Probe Error"), one that ends the process on
//!   creation ("Fixture Probe Crash"), one that never returns from creation ("Fixture Probe
//!   Block"), then "Fixture Notes Silent".
//!
//! None of them writes events yet; every output buffer is silent. The blocking plug-in first writes
//! `blocking.mark` into the process's working folder, so a test can see that its probe child is
//! inside the creation before it stops it.

// Each build lists one set, so the ids and descriptors of the other sets go unused in it.
#![allow(dead_code)]

use super::*;

struct Features2([*const c_char; 2]);
// SAFETY: pointers to static C strings, never written.
unsafe impl Sync for Features2 {}

static INSTRUMENT: Features2 = Features2([c"instrument".as_ptr(), std::ptr::null()]);
static NOTE_EFFECT: Features2 = Features2([c"note-effect".as_ptr(), std::ptr::null()]);
static EFFECT: Features2 = Features2([c"audio-effect".as_ptr(), std::ptr::null()]);

const fn desc(id: &'static CStr, name: &'static CStr, features: &'static Features2) -> clap_plugin_descriptor {
    clap_plugin_descriptor {
        clap_version: CLAP_VERSION,
        id: id.as_ptr(),
        name: name.as_ptr(),
        vendor: c"SoundCraft tests".as_ptr(),
        url: std::ptr::null(),
        manual_url: std::ptr::null(),
        support_url: std::ptr::null(),
        version: c"1.0.0".as_ptr(),
        description: std::ptr::null(),
        features: features.0.as_ptr(),
    }
}

static NOTES_DESC: clap_plugin_descriptor = desc(c"org.soundcraft.test.notes", c"Fixture Notes", &INSTRUMENT);
static NOTES_SILENT_DESC: clap_plugin_descriptor = desc(c"org.soundcraft.test.notes-silent", c"Fixture Notes Silent", &INSTRUMENT);
static EFFECT_A_DESC: clap_plugin_descriptor = desc(c"org.soundcraft.test.note-effect-a", c"Fixture Note Effect A", &NOTE_EFFECT);
static EFFECT_B_DESC: clap_plugin_descriptor = desc(c"org.soundcraft.test.note-effect-b", c"Fixture Note Effect B", &NOTE_EFFECT);
static EFFECT_C_DESC: clap_plugin_descriptor = desc(c"org.soundcraft.test.note-effect-c", c"Fixture Note Effect C", &NOTE_EFFECT);
static PROBE_ERROR_DESC: clap_plugin_descriptor = desc(c"org.soundcraft.test.probe-error", c"Fixture Probe Error", &EFFECT);
static PROBE_CRASH_DESC: clap_plugin_descriptor = desc(c"org.soundcraft.test.probe-crash", c"Fixture Probe Crash", &INSTRUMENT);
static PROBE_BLOCK_DESC: clap_plugin_descriptor = desc(c"org.soundcraft.test.probe-block", c"Fixture Probe Block", &EFFECT);

#[cfg(feature = "notes")]
pub(crate) static DESCS: [&clap_plugin_descriptor; 2] = [&NOTES_DESC, &NOTES_SILENT_DESC];
#[cfg(all(feature = "note-effects", not(feature = "notes")))]
pub(crate) static DESCS: [&clap_plugin_descriptor; 3] = [&EFFECT_A_DESC, &EFFECT_B_DESC, &EFFECT_C_DESC];
#[cfg(all(feature = "probe-failures", not(any(feature = "notes", feature = "note-effects"))))]
pub(crate) static DESCS: [&clap_plugin_descriptor; 5] = [&NOTES_DESC, &PROBE_ERROR_DESC, &PROBE_CRASH_DESC, &PROBE_BLOCK_DESC, &NOTES_SILENT_DESC];

/// The ports a plug-in declares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Stereo audio out; note in; note out 0 MIDI 2 only, note out 1 CLAP and MIDI.
    Generator,
    /// Stereo audio out; note in; note out 0 MIDI 2 only.
    Silent,
    /// Note in, note out (CLAP and MIDI); no audio.
    Effect,
}

#[repr(C)]
struct Note {
    plugin: clap_plugin,
    shape: Shape,
}

/// The plug-in for one of this module's ids: `Some(null)` for the one whose creation fails,
/// `None` for an id that is not this module's. The crashing and blocking plug-ins never return.
pub(crate) fn create(id: &CStr) -> Option<*const clap_plugin> {
    let (desc, shape) = if id == c"org.soundcraft.test.notes" {
        (&NOTES_DESC, Shape::Generator)
    } else if id == c"org.soundcraft.test.notes-silent" {
        (&NOTES_SILENT_DESC, Shape::Silent)
    } else if id == c"org.soundcraft.test.note-effect-a" {
        (&EFFECT_A_DESC, Shape::Effect)
    } else if id == c"org.soundcraft.test.note-effect-b" {
        (&EFFECT_B_DESC, Shape::Effect)
    } else if id == c"org.soundcraft.test.note-effect-c" {
        (&EFFECT_C_DESC, Shape::Effect)
    } else if id == c"org.soundcraft.test.probe-error" {
        return Some(std::ptr::null());
    } else if id == c"org.soundcraft.test.probe-crash" {
        crash()
    } else if id == c"org.soundcraft.test.probe-block" {
        block()
    } else {
        return None;
    };
    let b = Box::new(Note {
        plugin: clap_plugin {
            desc: desc as *const _,
            plugin_data: std::ptr::null_mut(),
            init: Some(init),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(deactivate),
            start_processing: Some(start_processing),
            stop_processing: Some(stop_processing),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(get_extension),
            on_main_thread: Some(on_main_thread),
        },
        shape,
    });
    let raw = Box::into_raw(b);
    // SAFETY: `raw` is the live box just leaked; it is freed in `destroy`.
    unsafe {
        (*raw).plugin.plugin_data = raw.cast::<c_void>();
        Some(&(*raw).plugin)
    }
}

/// Ends the process at once, as a plug-in that crashes in its constructor does. Windows: an exit
/// with the access-violation status, not an abort, so no error report or dialog is raised.
fn crash() -> ! {
    #[cfg(windows)]
    {
        unsafe extern "C" {
            fn _exit(status: i32) -> !;
        }
        // SAFETY: the C library's `_exit`; ends the process without running anything else.
        unsafe { _exit(0xC000_0005_u32 as i32) }
    }
    #[cfg(not(windows))]
    std::process::abort()
}

/// Never returns, as a plug-in waiting on a dialog or a licence server does; says so first in
/// `blocking.mark` in the working folder.
fn block() -> ! {
    let _ = std::fs::write("blocking.mark", b"blocking in create_plugin");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// # Safety
/// `p` is a plug-in this module made and has not destroyed.
unsafe fn note<'a>(p: *const clap_plugin) -> &'a Note {
    // SAFETY: caller contract; `plugin_data` is the plug-in's own box.
    unsafe { &*((*p).plugin_data as *const Note) }
}

unsafe extern "C" fn init(_p: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn destroy(p: *const clap_plugin) {
    // SAFETY: the host destroys a live plug-in once; its box is freed here.
    drop(unsafe { Box::from_raw((*p).plugin_data as *mut Note) });
}
unsafe extern "C" fn activate(_p: *const clap_plugin, _sr: f64, _min: u32, _max: u32) -> bool {
    true
}
unsafe extern "C" fn deactivate(_p: *const clap_plugin) {}
unsafe extern "C" fn start_processing(_p: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn stop_processing(_p: *const clap_plugin) {}
unsafe extern "C" fn reset(_p: *const clap_plugin) {}
unsafe extern "C" fn on_main_thread(_p: *const clap_plugin) {}

unsafe extern "C" fn process(_p: *const clap_plugin, pr: *const clap_process) -> clap_process_status {
    // SAFETY: the host passes a valid process struct for this call.
    let pr = unsafe { &*pr };
    for i in 0..pr.audio_outputs_count as usize {
        // SAFETY: `audio_outputs_count` buffers, each with `channel_count` channels of
        // `frames_count` samples, owned by the host for this call.
        unsafe {
            let out = &*pr.audio_outputs.add(i);
            for c in 0..out.channel_count as usize {
                std::slice::from_raw_parts_mut(*out.data32.add(c), pr.frames_count as usize).fill(0.0);
            }
        }
    }
    CLAP_PROCESS_CONTINUE
}

unsafe extern "C" fn audio_count(p: *const clap_plugin, is_input: bool) -> u32 {
    // SAFETY: the host passes a live plug-in of this module.
    u32::from(!is_input && unsafe { note(p) }.shape != Shape::Effect)
}

unsafe extern "C" fn audio_get(p: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool {
    // SAFETY: forwarded from the host's own call.
    if index >= unsafe { audio_count(p, is_input) } {
        return false;
    }
    // SAFETY: the host passes a writable clap_audio_port_info.
    let info = unsafe { &mut *info };
    info.id = 1;
    write_name(&mut info.name, c"out");
    info.flags = CLAP_AUDIO_PORT_IS_MAIN;
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = clap_sys::id::CLAP_INVALID_ID;
    true
}

static AUDIO: clap_plugin_audio_ports = clap_plugin_audio_ports { count: Some(audio_count), get: Some(audio_get) };

/// The dialects of note port `index` in one direction; `None` past the last port.
fn dialects(shape: Shape, index: u32, is_input: bool) -> Option<u32> {
    let readable = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    match (shape, is_input, index) {
        (_, true, 0) => Some(readable),
        (Shape::Effect, false, 0) => Some(readable),
        (Shape::Generator | Shape::Silent, false, 0) => Some(CLAP_NOTE_DIALECT_MIDI2),
        (Shape::Generator, false, 1) => Some(readable),
        _ => None,
    }
}

unsafe extern "C" fn note_count(p: *const clap_plugin, is_input: bool) -> u32 {
    // SAFETY: the host passes a live plug-in of this module.
    let shape = unsafe { note(p) }.shape;
    (0..).take_while(|&i| dialects(shape, i, is_input).is_some()).count() as u32
}

unsafe extern "C" fn note_get(p: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_note_port_info) -> bool {
    // SAFETY: the host passes a live plug-in of this module.
    let Some(supported) = dialects(unsafe { note(p) }.shape, index, is_input) else { return false };
    // SAFETY: the host passes a writable clap_note_port_info.
    let info = unsafe { &mut *info };
    info.id = index + if is_input { 0 } else { 10 };
    info.supported_dialects = supported;
    info.preferred_dialect = if supported & CLAP_NOTE_DIALECT_CLAP != 0 { CLAP_NOTE_DIALECT_CLAP } else { supported };
    write_name(&mut info.name, if is_input { c"notes in" } else { c"notes out" });
    true
}

static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports { count: Some(note_count), get: Some(note_get) };

unsafe extern "C" fn get_extension(_p: *const clap_plugin, id: *const c_char) -> *const c_void {
    // SAFETY: the host passes a NUL-terminated extension id.
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_AUDIO_PORTS {
        &AUDIO as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS {
        &NOTE_PORTS as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}
