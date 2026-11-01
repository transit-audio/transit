use super::{EVENT_CAP, Event, MidiVoice};

/// Convert in stable sample order, absorbing an off while another identity holds the key.
/// Entries are reserved at admission, including future ons, and reclaimed after this pass.
/// EVENT_CAP bounds storage; F10 has at most 224 sounding notes + 767 queued non-releases.
pub(super) fn midi_events(queue: &mut Vec<Event>, voices: &mut [Option<MidiVoice>; EVENT_CAP], held: &mut [[bool; 128]; 16], deferred: &mut u64) {
    queue.retain_mut(|event| {
        match *event {
            Event::NoteOn { time, key, velocity, channel, id } => {
                if let Some(v) = voices.iter_mut().flatten().find(|v| v.matches(channel, key, id)) {
                    v.sounding = true;
                }
                *event = Event::Midi { time, data: [0x90 | channel, key, (velocity * 127.0).round().clamp(1.0, 127.0) as u8] };
            }
            Event::NoteOff { time, key, velocity, channel, id } => {
                if let Some(v) = voices.iter_mut().flatten().find(|v| v.matches(channel, key, id)) {
                    v.sounding = false;
                }
                if voices.iter().flatten().any(|v| v.sounding && v.channel == channel && v.key == key) {
                    *deferred = deferred.saturating_add(1);
                    return false;
                }
                *event = Event::Midi { time, data: [0x80 | channel, key, (velocity * 127.0).round().clamp(0.0, 127.0) as u8] };
            }
            Event::Midi { data: [status, key, value], .. } => {
                // Global/raw releases, including all_notes_off and reset, retire their keys.
                let off = status & 0xF0 == 0x80 || (status & 0xF0 == 0x90 && value == 0);
                let all = status & 0xF0 == 0xB0 && matches!(key, 120 | 123);
                if off || all {
                    for v in voices.iter_mut().flatten().filter(|v| v.channel == status & 0x0F && (all || v.key == key)) {
                        v.sounding = false;
                    }
                }
            }
            _ => {}
        }
        true
    });
    *held = [[false; 128]; 16];
    for entry in voices {
        if let Some(v) = entry {
            if v.sounding {
                if let Some(h) = held.get_mut(usize::from(v.channel)).and_then(|h| h.get_mut(usize::from(v.key))) {
                    *h = true;
                }
            } else {
                *entry = None;
            }
        }
    }
}
