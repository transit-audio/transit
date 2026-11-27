//! Admission reserves one latest block-start value per parameter (#219); timed points share the
//! [`EVENT_CAP`] budget. Note and MIDI events are admitted by the delivery queue (ADR 0059) and only
//! pass through this queue for the call that hands them.
use super::{EVENT_CAP, Event};

fn block_start(e: &Event) -> bool {
    matches!(e, Event::Param { time: 0, .. })
}

/// Caller coalesces time-zero values by valid parameter id before admission, so there can be
/// at most the parameter count. Timed points cannot consume that reserved storage.
pub(super) fn push(queue: &mut Vec<Event>, event: Event) -> bool {
    if !has_room(queue, &event) {
        return false;
    }
    insert(queue, event);
    true
}

/// Inserts `event` after every queued event at or before its time (no allocation within the
/// capacity).
pub(super) fn insert(queue: &mut Vec<Event>, event: Event) {
    if queue.len() < queue.capacity() {
        let pos = queue.iter().rposition(|q| q.time() <= event.time()).map_or(0, |i| i + 1);
        queue.insert(pos, event);
    }
}

pub(super) fn has_room(queue: &Vec<Event>, event: &Event) -> bool {
    if queue.len() >= queue.capacity() {
        return false;
    }
    if block_start(event) {
        return true;
    }
    queue.iter().filter(|e| !block_start(e)).count() < EVENT_CAP
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_parameter_has_room_beside_saturated_timed_points() {
        const PARAMETERS: usize = 4096;
        let mut queue = Vec::with_capacity(EVENT_CAP + PARAMETERS);
        for id in 0..EVENT_CAP as u32 {
            assert!(push(&mut queue, Event::Param { time: 1, id, value: 0.5 }));
        }
        assert!(!push(&mut queue, Event::Param { time: 1, id: 0, value: 0.5 }));
        for id in 0..PARAMETERS as u32 {
            assert!(push(&mut queue, Event::Param { time: 0, id, value: 0.75 }));
        }
        assert_eq!(queue.len(), EVENT_CAP + PARAMETERS);
        assert!(queue[..PARAMETERS].iter().all(block_start));
    }
}
