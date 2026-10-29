//! Test seams of the pool (feature `testing`, issue #210): the pending MIDI of a strip for its
//! fingerprint ([`crate::fingerprint`]).

use super::*;
use std::hash::Hasher;

impl StripPool {
    /// The messages parked for strip `i`, in delivery order: its deferred ring, then the live
    /// events taken while it was away.
    pub(crate) fn pending_midi(&self, i: usize, h: &mut dyn Hasher) {
        let side = self.side.get(i * SIDE..(i + 1) * SIDE).unwrap_or(&[]);
        let put = |h: &mut dyn Hasher, e: &Event| {
            if let Some(Some(ev)) = side.get(e.tag as usize) {
                h.write(format!("{:?}", (ev.source, ev.transport, ev.message)).as_bytes());
                h.write_u8(0);
            } else {
                h.write(format!("{:?}", (e.status, e.data1, e.data2)).as_bytes());
                h.write_u8(1);
            }
        };
        if let Some(r) = self.defer.ring(i) {
            r.events().iter().for_each(|e| put(h, e));
            if r.is_silenced() {
                h.write_u16(r.silence_mask());
            }
        }
        let n = self.live_len.get(i).copied().unwrap_or(0);
        self.live.get(i * LIVE..i * LIVE + n).unwrap_or(&[]).iter().for_each(|e| put(h, e));
    }
}
