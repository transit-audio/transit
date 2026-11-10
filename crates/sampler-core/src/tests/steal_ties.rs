//! Inject equal serials: ordinary event dispatch gives each start/release its own serial.

use super::*;

#[test]
fn voice_slot_index_breaks_equal_age_ties() {
    let mut c = SamplerCore::new();
    c.prepare(48_000.0, 1);
    for v in &mut c.voices {
        v.active = true;
        v.serial = 1;
    }
    assert_eq!(c.allocate(), 0, "equal sounding ages choose the first slot");
    // Refill the stolen slot; only two higher-index slots are ending, with equal release ages.
    c.voices[0].active = true;
    c.voices[31].choke(2);
    c.voices[17].choke(2);
    assert_eq!(c.allocate(), 17, "equal release ages choose the first ending slot");
    c.voices[17].active = true;
    c.voices[9].active = false;
    assert_eq!(c.allocate(), 9, "free slots precede even an ending voice");
}
