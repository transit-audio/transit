//! Unit tests of `plugin.rs`.

use super::*;

#[test]
fn only_values_the_host_set_are_re_sent() {
    assert_eq!(resend_indices(&[false, false, false], 8).count(), 0, "readings from the plugin stay put");
    assert_eq!(resend_indices(&[false, true, false, true], 8).collect::<Vec<_>>(), vec![1, 3]);
    assert_eq!(resend_indices(&[true, true, true], 2).collect::<Vec<_>>(), vec![0, 1], "bounded by the queue");
}

#[test]
fn a_table_published_while_it_is_read_is_not_taken() {
    let m = MidiMap::new();
    let a: Vec<Option<u32>> = (0..MAP_ENTRIES as u32).map(Some).collect();
    let b: Vec<Option<u32>> = vec![Some(7); MAP_ENTRIES];
    m.publish(&a);
    let mut out = vec![None; MAP_ENTRIES];
    let g = m.read(&mut out).expect("a published table");
    assert_eq!(out.to_vec(), a);
    // A publication that lands after the entries were copied: the copy may mix both tables,
    // so the sequence check must refuse it (review mutant V1).
    assert_eq!(m.read_with(&mut out, || m.publish(&b)), None);
    assert_eq!(m.read(&mut out), Some(g + 2));
    assert_eq!(out.to_vec(), b);
    // A write in progress (odd sequence) is refused before anything is copied.
    m.seq.fetch_add(1, Ordering::Relaxed);
    assert_eq!(m.read(&mut out), None);
}

#[test]
fn process_status_follows_the_tail() {
    assert_eq!(status_from_tail(u32::MAX), ProcessStatus::Continue);
    assert_eq!(status_from_tail(0), ProcessStatus::Tail);
    assert_eq!(status_from_tail(48_000), ProcessStatus::Tail);
}

#[test]
fn reset_keeps_the_releases_it_owes() {
    let mut notes = Vec::with_capacity(EVENT_CAP);
    notes.push(NoteEvent::On { time: 3, key: 72, velocity: 1.0, channel: 0, id: -1 });
    notes.push(NoteEvent::Off { time: 5, key: 67, velocity: 0.0, channel: 0, id: -1 });
    notes.push(NoteEvent::Pressure { time: 6, key: 67, pressure: 0.5, channel: 0, id: -1 });
    notes.push(NoteEvent::On { time: 7, key: 50, velocity: 0.0, channel: 2, id: 4 });
    let mut held = [[false; 128]; MIDI_CHANNELS];
    held[0][60] = true;
    held[9][64] = true;
    keep_releases_on_reset(&mut notes, &mut held);
    assert!(notes.contains(&NoteEvent::On { time: 7, key: 50, velocity: 0.0, channel: 2, id: 4 }), "a velocity-0 note-on is a release");
    notes.retain(|e| !matches!(e, NoteEvent::On { velocity: 0.0, .. }));
    let mut keys: Vec<u8> = notes.iter().filter_map(|e| if let NoteEvent::Off { key, .. } = e { Some(*key) } else { None }).collect();
    assert_eq!(keys.len(), notes.len(), "reset left a non-release queued");
    keys.sort_unstable();
    assert_eq!(keys, [60, 64, 67], "reset must queue an off for every held or already-released key");
    assert!(held.iter().flatten().all(|h| !h), "reset must clear held");
    assert!(notes.iter().any(|e| matches!(e, NoteEvent::Off { key: 64, channel: 9, .. })), "on its channel");
}
