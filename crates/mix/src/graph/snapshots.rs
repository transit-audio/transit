//! Plan-owned inputs. Each reader has its own storage; a job never borrows a live bus.
use super::*;

#[derive(Default)]
pub(crate) struct Snapshots {
    entries: Vec<(usize, Option<(BusId, BusBuf)>)>,
}

impl Snapshots {
    pub(crate) fn new(rows: &[tail::Row], phase1: usize, block: usize) -> Self {
        Self {
            entries: rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.live)
                .filter_map(|(k, r)| {
                    let (id, fmt) = r.input?;
                    Some((
                        phase1 + k,
                        Some((
                            id,
                            BusBuf {
                                fmt,
                                buf: vec![vec![0.0; block]; fmt.channels().clamp(1, MAX_CHANNELS)],
                                host_buf: Vec::new(),
                                arrival: 0,
                                reader: usize::MAX,
                            },
                        )),
                    ))
                })
                .collect(),
        }
    }

    pub(crate) fn copy(&mut self, slot: usize, busses: &BusTable, frames: usize) {
        if let Some((id, dst)) = self.entries.iter_mut().find(|(i, _)| *i == slot).and_then(|(_, e)| e.as_mut()) {
            let src = busses.bus(*id);
            for (c, out) in dst.buf.iter_mut().enumerate() {
                for (i, x) in out.iter_mut().take(frames).enumerate() {
                    *x = src.and_then(|b| b.buf.get(c)).and_then(|c| c.get(i)).copied().unwrap_or(0.0);
                }
            }
        }
    }

    pub(crate) fn lend(&mut self, slot: usize, to: &mut BusTable) -> bool {
        if !to.0.is_empty() || to.0.capacity() == 0 {
            return false;
        }
        if let Some((_, entry)) = self.entries.iter_mut().find(|(i, _)| *i == slot) {
            let Some(value) = entry.take() else {
                return false;
            };
            to.0.push(value);
        }
        true
    }

    pub(crate) fn take_back(&mut self, slot: usize, from: &mut BusTable) {
        if let Some((_, entry)) = self.entries.iter_mut().find(|(i, _)| *i == slot)
            && entry.is_none()
        {
            *entry = from.0.pop();
        }
    }
}
