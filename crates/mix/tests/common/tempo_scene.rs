use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, Insert, Session, TrackKind};

pub fn scene() -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("macro_oscillator"));
    let mut f = Insert::new("filter");
    for (k, v) in [
        ("slope", 1.0),
        ("cutoff", 300.0),
        ("resonance", 60.0),
        ("env_amount", 70.0),
        ("env_decay", 150.0),
        ("lfo_amount", 15.0),
        ("lfo_sync", 1.0),
        ("lfo_shape", 1.0),
        ("key_track", 40.0),
        ("follow_amount", 10.0),
        ("amp_on", 1.0),
        ("amp_sustain", 60.0),
        ("amp_release", 120.0),
    ] {
        f.params.insert(k.into(), v);
    }
    tr.mixer.inserts[0] = Some(f);
    let mut seq = Sequence::default();
    // Odd frames: 25 frames a tick.
    for (pitch, start, length) in [(48, 1, 300), (55, 157, 301), (60, 523, 99), (43, 901, 777)] {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 0, channel: 0, start, length });
    }
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "phrase", 0, 96_000, seq));
    s
}
