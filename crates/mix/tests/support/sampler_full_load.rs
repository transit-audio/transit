//! Serialized full-load fixture shared by the mix and command-query tests.
#![allow(dead_code)]

use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_midi::{Note, Sequence};
use transit_model::sampler_patch::*;
use transit_model::{ChannelFormat, Clip, Insert, SamplerPatch, Session, SourceAudio, SourceId, TrackId, TrackKind};

pub const FRAMES: usize = 16384;

pub fn patch(shaped: bool) -> PatchV1 {
    let mut p = PatchV1 { seed: 0x1357_2468, ..PatchV1::default() };
    p.sources = (0..256).map(|i| SourceRef { source: SourceId(i + 1000), ..SourceRef::default() }).collect();
    p.groups = vec![GroupEntry { id: 0, ..GroupEntry::default() }, GroupEntry { id: 1, round_robin: RoundRobin::Random, ..GroupEntry::default() }];
    p.slicing = Some(SliceLoop {
        source: 128,
        end: 4096,
        mode: SliceMode::Manual,
        manual: Some(Markers {
            markers: (0..16).map(|i| Marker { id: i + 1, frame: u64::from(i) * 256, ..Marker::default() }).collect(),
            next_id: 17,
            ..Markers::default()
        }),
        ..SliceLoop::default()
    });
    for id in 0..256u16 {
        let mut z = ZoneEntry::whole(id, id, 4096);
        z.key_tracking = false;
        z.attack_ms = 0.0;
        z.decay_ms = 0.0;
        z.release_ms = 32.0 * 1000.0 / 48000.0;
        z.velocity_sensitivity = 1.0;
        z.reverse = id % 5 == 0;
        if id < 128 {
            let group = (id / 64) as u8;
            z.keys = [group * 8, group * 8 + 7];
            z.membership = Some(Membership { group, layer: (id % 64 / 8) as u8, alternative: (id % 8) as u8, ..Membership::default() });
            z.fades = Fades::of([2, 2, 8, 8]);
        } else if id < 144 {
            let slice = id - 128;
            z.source = 128;
            z.start = u64::from(slice) * 256;
            z.end = z.start + 256;
            z.loop_start = z.start;
            z.loop_end = z.end;
            z.mode = if slice < 2 { PlayMode::Gate } else { PlayMode::OneShot };
            z.loop_mode = match slice {
                0 => LoopMode::Forward,
                1 => LoopMode::PingPong,
                _ => LoopMode::Off,
            };
            z.slice = Some(SliceBinding { set: SliceSet::Manual, id: u32::from(slice) + 1, ..SliceBinding::default() });
            p.set_pad(PadEntry { index: slice as u8, zone: Some(id), choke: if (2..6).contains(&slice) { 1 } else { 0 }, ..PadEntry::default() });
        } else {
            let key = 64 + ((id - 144) / 2) as u8;
            z.keys = [key, key];
            z.velocities = if id % 2 == 0 { [1, 100] } else { [28, 127] };
            z.fades = Fades::of([0, 0, 8, 8]);
        }
        if !(128..144).contains(&id) {
            z.loop_mode = if id % 3 == 0 { LoopMode::PingPong } else { LoopMode::Forward };
            z.loop_start = 64;
            z.loop_end = 448;
            z.crossfade = if id % 3 == 0 { 0 } else { 64 };
        }
        p.zones.push(z);
    }
    if shaped {
        for z in 1..=4 {
            for (field, value) in [("shaper", 1.0), ("cutoff", 1024.0), ("resonance", 25.0)] {
                p.controls.insert(format!("zone{z:03}_{field}"), value);
            }
            for row in 1..=16 {
                for (field, value) in [("source", (row % 12 + 1) as f32), ("dest", (row % 8 + 6) as f32), ("amount", 2.5)] {
                    p.controls.insert(format!("zone{z:03}_mod{row}_{field}"), value);
                }
            }
        }
    }
    p.validate().unwrap();
    p
}

pub fn install(s: &mut Session, p: PatchV1) -> TrackId {
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut ins = Insert::new("sampler");
    ins.sampler_patch = Some(SamplerPatch::from_value(serde_json::to_value(p).unwrap()));
    s.track_mut(t).unwrap().instrument = Some(ins);
    t
}

pub fn media(s: &mut Session) {
    for id in 0..256u32 {
        let mut x = id + 1;
        let data = (0..4096)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 16) as f32 / 65536.0 - 0.5
            })
            .collect();
        s.pool.insert(SourceId(u64::from(id) + 1000), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48000, channels: vec![data] })));
    }
}

pub fn session(shaped: bool) -> Session {
    let mut s = Session::default();
    s.edit.click = false;
    media(&mut s);
    let t = install(&mut s, patch(shaped));
    let mut seq = Sequence::default();
    for i in 0..192 {
        let pitch = if i < 32 {
            (i % 16) as u8
        } else if i < 144 {
            64 + (i % 56) as u8
        } else {
            36 + (i % 16) as u8
        };
        seq.notes.push(Note { pitch, velocity: if i % 2 == 0 { 35 } else { 95 }, release_velocity: 64, channel: 0, start: i * 3, length: 300 });
    }
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "Full load", 0, FRAMES as i64, seq));
    s
}

pub fn bits(audio: &[Vec<f32>]) -> Vec<u32> {
    assert!(audio.iter().flatten().all(|x| x.is_finite()));
    audio.iter().flatten().map(|x| x.to_bits()).collect()
}
