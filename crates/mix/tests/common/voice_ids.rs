use transit_mix::{Message, NoteId};
pub(super) fn on_id(key: u8, id: u32) -> Message {
    Message::NoteOn { key, velocity: 100, id: NoteId(id) }
}

pub(super) fn off_id(key: u8, id: u32) -> Message {
    Message::NoteOff { key, velocity: 0, id: NoteId(id) }
}
