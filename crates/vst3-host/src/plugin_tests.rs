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
