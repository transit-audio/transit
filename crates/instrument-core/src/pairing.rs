//! Note-on and note-off pairing (#292): a note-off closes the oldest open note-on of its key and
//! channel, the convention Ableton and Logic agree on. An instrument that keeps one flag per key
//! forgets that a key was struck twice: a re-strike arriving before the earlier note's off, then
//! that off, ended the re-struck note at once. This table counts the open note-ons per key and
//! channel, so the instrument can tell an off that closes an earlier, superseded note from the off
//! that ends the newest one.
//!
//! Each note-on gets a sequence number (wrapping, per key and channel); each note-off returns the
//! number of the note-on it closes. An instrument with one voice per key (a re-strike restarts the
//! voice) acts on an off only when no newer note-on of the key stays open; one with a voice per
//! note-on releases the voices of the closed number.
//!
//! Self-healing: a note-on for a key the instrument no longer holds forgets any older note-ons
//! still counted open (an off lost upstream, or a voice stolen or ended under a held key), so a
//! missing off can never keep a later note from ending. Fixed-size, no allocation: real-time safe.

/// MIDI channels.
pub const CHANNELS: usize = 16;
/// MIDI keys.
pub const KEYS: usize = 128;

/// Open note-ons per key and channel.
#[derive(Debug, Clone)]
pub struct NotePairing {
    /// The next note-on's sequence number.
    ons: [[u8; KEYS]; CHANNELS],
    /// The sequence number of the oldest open note-on (the next off closes it).
    offs: [[u8; KEYS]; CHANNELS],
}

impl Default for NotePairing {
    fn default() -> Self {
        Self::new()
    }
}

impl NotePairing {
    #[must_use]
    pub const fn new() -> Self {
        Self { ons: [[0; KEYS]; CHANNELS], offs: [[0; KEYS]; CHANNELS] }
    }

    fn index(channel: u8, key: u8) -> (usize, usize) {
        (usize::from(channel) % CHANNELS, usize::from(key.min(127)))
    }

    /// The note-ons of `key` on `channel` no note-off has closed yet.
    #[must_use]
    pub fn open(&self, channel: u8, key: u8) -> u8 {
        let (c, k) = Self::index(channel, key);
        let on = self.ons.get(c).and_then(|r| r.get(k)).copied().unwrap_or(0);
        let off = self.offs.get(c).and_then(|r| r.get(k)).copied().unwrap_or(0);
        on.wrapping_sub(off)
    }

    /// A note-on of `key` on `channel`: its sequence number. `held` says whether the instrument
    /// still holds the key down from an earlier note-on (not only by the pedal); when it does not,
    /// earlier note-ons still counted open are forgotten. At 255 open note-ons the oldest is
    /// dropped.
    pub fn on(&mut self, channel: u8, key: u8, held: bool) -> u8 {
        let open = self.open(channel, key);
        let (c, k) = Self::index(channel, key);
        let (Some(on), Some(off)) = (self.ons.get_mut(c).and_then(|r| r.get_mut(k)), self.offs.get_mut(c).and_then(|r| r.get_mut(k))) else {
            return 0;
        };
        if !held {
            *off = *on;
        } else if open == u8::MAX {
            *off = off.wrapping_add(1);
        }
        let seq = *on;
        *on = on.wrapping_add(1);
        seq
    }

    /// A note-off of `key` on `channel`: the sequence number of the note-on it closes (the
    /// oldest open one), or `None` when none is open.
    pub fn off(&mut self, channel: u8, key: u8) -> Option<u8> {
        if self.open(channel, key) == 0 {
            return None;
        }
        let (c, k) = Self::index(channel, key);
        let off = self.offs.get_mut(c).and_then(|r| r.get_mut(k))?;
        let seq = *off;
        *off = off.wrapping_add(1);
        Some(seq)
    }

    /// Every note closed (All Notes Off, All Sound Off, a reset).
    pub fn clear(&mut self) {
        self.offs = self.ons;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_off_closes_the_oldest_open_note_on() {
        let mut p = NotePairing::new();
        assert_eq!(p.on(0, 60, false), 0);
        assert_eq!(p.on(0, 60, true), 1);
        assert_eq!(p.open(0, 60), 2);
        assert_eq!(p.off(0, 60), Some(0));
        assert_eq!(p.open(0, 60), 1);
        assert_eq!(p.off(0, 60), Some(1));
        assert_eq!(p.off(0, 60), None);
        assert_eq!(p.open(0, 60), 0);
    }

    #[test]
    fn keys_and_channels_pair_separately() {
        let mut p = NotePairing::new();
        p.on(0, 60, false);
        p.on(1, 60, false);
        p.on(0, 61, false);
        assert_eq!(p.off(1, 60), Some(0));
        assert_eq!(p.open(0, 60), 1);
        assert_eq!(p.open(0, 61), 1);
        assert_eq!(p.open(1, 60), 0);
    }

    #[test]
    fn a_note_on_for_an_unheld_key_forgets_older_open_notes() {
        let mut p = NotePairing::new();
        p.on(0, 60, false);
        p.on(0, 60, true);
        // The instrument no longer holds the key (an off lost upstream): start afresh.
        let seq = p.on(0, 60, false);
        assert_eq!(p.open(0, 60), 1);
        assert_eq!(p.off(0, 60), Some(seq));
    }

    #[test]
    fn sequence_numbers_wrap_and_open_counts_saturate() {
        let mut p = NotePairing::new();
        for i in 0..300u32 {
            p.on(3, 7, i > 0);
        }
        assert_eq!(p.open(3, 7), u8::MAX);
        let first = p.off(3, 7);
        assert_eq!(first, Some((300 - 255) as u8));
        p.clear();
        assert_eq!(p.open(3, 7), 0);
        assert_eq!(p.off(3, 7), None);
    }
}
