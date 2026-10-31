//! Shared real-device fixture for the mix and engine offline latency tests.
use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_model::{AutoParam, AutomationLane, ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackId, TrackKind};

pub const LEN: usize = 10_003;
pub const IMPULSES: [usize; 3] = [1024, 4096, 8192];

/// Two isolated impulse tracks: the latent insert on the left, its PDC peer on the right.
pub fn session(device: &str, values: &[f32], changes: &[i64]) -> (Session, TrackId) {
    let mut s = Session::default();
    let mut samples = vec![0.0; LEN];
    for at in IMPULSES {
        samples[at] = 0.125;
    }
    s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![samples] })));
    let mut latent = TrackId(0);
    for pan in [-1.0, 1.0] {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let c = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(c, "impulses", SourceId(900), 0, 0, LEN as i64));
        tr.mixer.pan = vec![pan];
        if pan < 0.0 {
            latent = t;
            let mut ins = Insert::new(device);
            let param = if device == "texture" {
                ins.params.insert("dry_wet".into(), 0.0);
                ins.params.insert("input_gain".into(), 1.0);
                "quality"
            } else {
                ins.params.insert("ratio".into(), 1.0);
                "lookahead"
            };
            ins.params.insert(param.into(), values[0]);
            tr.mixer.inserts[0] = Some(ins);
            let mut lane = AutomationLane::new(AutoParam::Plugin { slot: 0, param: param.into() });
            lane.set_point(0, values[0]);
            for (i, &at) in changes.iter().enumerate() {
                lane.set_point(at - 1, values[i]);
                lane.set_point(at, values[i + 1]);
            }
            tr.automation.push(lane);
        }
    }
    (s, latent)
}

pub fn positions(audio: &[Vec<f32>]) -> Vec<usize> {
    audio
        .iter()
        .flat_map(|c| {
            IMPULSES.map(|at| {
                let begin = at - 600;
                let (i, peak) = c[begin..at + 600].iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap();
                assert!(peak.abs() > 0.01, "missing impulse at {at}");
                begin + i
            })
        })
        .collect()
}
