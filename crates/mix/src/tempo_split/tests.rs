//! Probes the actual Transport and sample offsets delivered to a processor.
use super::*;
use crate::tests::PlanRig;
use std::sync::{Arc, Mutex};
use transit_midi::{Note, Sequence};
use transit_model::{Clip, Insert};

#[test]
fn an_inverse_rounding_error_never_delays_a_meter_or_tempo_change() {
    let mut s = Session::default();
    s.tempo.set_tempo(0, 90.0).unwrap();
    s.tempo.set_meter(65280, 3, 4).unwrap();
    let frame = 2176000;
    assert!(s.tempo.samples_to_ticks_f(frame, s.sample_rate) < 65280.0, "the inverse-rounding repro");
    assert_eq!(s.tempo.render_span(frame - 100, 512, s.sample_rate), 100);
    let before = transport_at(&s, frame - 1, true, false);
    let after = transport_at(&s, frame, true, false);
    assert_eq!((before.numerator, after.numerator), (4, 3));
    assert_eq!(after.bar_start, 68.0);
    assert_eq!(after.quarters, 68.0);
    s.tempo.set_tempo(65280, 150.0).unwrap();
    assert_eq!(transport_at(&s, frame, true, false).tempo, 150.0);
}

#[derive(Default)]
struct Trace {
    blocks: Vec<(i64, usize, f64)>,
    notes: Vec<(i64, u32, u8)>,
    params: Vec<(i64, u32, f32)>,
}

struct Probe {
    trace: Arc<Mutex<Trace>>,
    transport: Transport,
    points: Vec<(u32, f32)>,
}

impl Plugin for Probe {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, value: f32) -> bool {
        self.points.push((0, value));
        true
    }
    fn set_param_at(&mut self, offset: u32, _: &str, value: f32) -> bool {
        self.points.push((offset, value));
        true
    }
    fn sample_accurate_params(&self) -> bool {
        true
    }
    fn param(&self, _: &str) -> Option<f32> {
        Some(0.0)
    }
    fn set_transport(&mut self, t: &Transport) {
        self.transport = *t;
    }
    fn event(&mut self, offset: u32, event: &PluginEvent) -> bool {
        if let Message::NoteOn { key, .. } = event.message {
            self.trace.lock().unwrap().notes.push((self.transport.samples, offset, key));
        }
        true
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let mut trace = self.trace.lock().unwrap();
        trace.blocks.push((self.transport.samples, frames, self.transport.tempo));
        for (at, value) in self.points.drain(..) {
            trace.params.push((self.transport.samples, at, value));
        }
        for c in io {
            c[..frames].fill(0.1);
        }
    }
}

#[test]
fn transport_midi_and_parameter_offsets_restart_at_each_change() {
    let mut s = Session::default();
    s.tempo.set_tempo(64, 150.0).unwrap();
    s.tempo.set_tempo(80, 100.0).unwrap();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let id = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    let mut instrument = Insert::new("macro_oscillator");
    instrument.params.insert("gain".into(), 0.0);
    tr.instrument = Some(instrument);
    let mut seq = Sequence::default();
    for (pitch, start) in [(60, 63), (61, 64), (62, 65), (63, 80)] {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 0, channel: 0, start, length: 1 });
    }
    tr.playlist_mut().unwrap().clips.push(Clip::midi(id, "offsets", 0, 4096, seq));
    tr.mixer.automation_mode = transit_model::AutomationMode::Read;
    let lane = tr.lane_mut(&AutoParam::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: "gain".into() });
    for (at, value) in [(0, 1.0), (1589, 2.0), (1600, 3.0), (1613, 4.0), (1920, 5.0), (4095, 6.0)] {
        lane.set_point(at, value);
    }
    let trace = Arc::new(Mutex::new(Trace::default()));
    let probe = Probe { trace: trace.clone(), transport: Transport::default(), points: Vec::new() };
    let mut rig = PlanRig::new(4096).fake(t, INSTRUMENT_SLOT, probe);
    rig.publish(&s);
    rig.render(0);
    let trace = trace.lock().unwrap();
    assert_eq!(trace.blocks, [(0, 1600, 120.0), (1600, 320, 150.0), (1920, 2176, 100.0)]);
    assert_eq!(trace.notes, [(0, 1575, 60), (1600, 0, 61), (1600, 20, 62), (1920, 0, 63)]);
    for point in [(0, 1589, 2.0), (1600, 0, 3.0), (1600, 13, 4.0), (1920, 0, 5.0), (1920, 2175, 6.0)] {
        assert!(trace.params.contains(&point), "missing {point:?}: {:?}", trace.params);
    }
}
