//! Admission reserves one latest block-start value per parameter (#219).
use super::{EVENT_CAP, Event, RELEASE_RESERVE};

fn block_start(e: &Event) -> bool {
    matches!(e, Event::Param { time: 0, .. })
}

/// Caller coalesces time-zero values by valid parameter id before admission, so there can be
/// at most the parameter count. MIDI and timed points cannot consume that reserved storage.
pub(super) fn push(queue: &mut Vec<Event>, event: Event, release: bool) -> bool {
    if !has_room(queue, &event, release) {
        return false;
    }
    let pos = queue.iter().rposition(|q| q.time() <= event.time()).map_or(0, |i| i + 1);
    queue.insert(pos, event);
    true
}

pub(super) fn has_room(queue: &Vec<Event>, event: &Event, release: bool) -> bool {
    let ordinary = queue.iter().filter(|e| !block_start(e)).count();
    let room = if release { EVENT_CAP } else { EVENT_CAP - RELEASE_RESERVE };
    queue.len() < queue.capacity() && (block_start(event) || ordinary < room)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_parameter_has_room_beside_saturated_midi_and_its_releases() {
        const PARAMETERS: usize = 4096;
        let mut queue = Vec::with_capacity(EVENT_CAP + PARAMETERS);
        for _ in 0..EVENT_CAP - RELEASE_RESERVE {
            assert!(push(&mut queue, Event::Midi { time: 1, data: [0xa0, 60, 1] }, false));
        }
        assert!(!push(&mut queue, Event::Midi { time: 1, data: [0xa0, 60, 1] }, false));
        for id in 0..PARAMETERS as u32 {
            assert!(push(&mut queue, Event::Param { time: 0, id, value: 0.75 }, false));
        }
        for _ in 0..RELEASE_RESERVE {
            assert!(push(&mut queue, Event::Midi { time: 2, data: [0x80, 60, 0] }, true));
        }
        assert_eq!(queue.len(), EVENT_CAP + PARAMETERS);
        assert!(queue[..PARAMETERS].iter().all(block_start));
    }

    #[test]
    fn reset_cannot_use_the_parameter_reserve_for_an_extra_note_off() {
        let mut queue = Vec::with_capacity(EVENT_CAP + 1);
        for _ in 0..EVENT_CAP {
            assert!(push(&mut queue, Event::Midi { time: 2, data: [0xb0, 64, 0] }, true));
        }
        let mut held = [[false; 128]; 16];
        held[0][60] = true;
        super::super::keep_releases_on_reset(&mut queue, &mut held, super::super::NoteMode::Clap);
        assert!(push(&mut queue, Event::Param { time: 0, id: 7, value: 0.75 }, false));
    }
}
