//! An offline render of a session with a hosted stage gives the instrument the same events at the
//! same frames as playback (ADR 0064 HM2, 4.7 and 10.3), through the plan compiler and the engine on
//! both sides, with the fake note plug-in in the slot (the offline render's factory is the test's).

use super::hosted_fake::{Fake, out};
use super::*;
use crate::offline::RangeRender;
use crate::{Adoption, DeviceGeneration, InstanceSpec, MixEngine, NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, SlotGen};
use std::sync::{Arc, Mutex};
use transit_dsp::hosted_out::OutKind;
use transit_model::{ChannelFormat, Insert, Session, TrackKind};
use transit_time::Range;

type Heard = Arc<Mutex<Vec<(u64, PluginEvent)>>>;

struct Recorder {
    frame: u64,
    heard: Heard,
}

impl Plugin for Recorder {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], frames: usize) {
        self.frame += frames as u64;
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        self.heard.lock().unwrap().push((self.frame + u64::from(offset), *ev));
        true
    }
}

fn session() -> Session {
    let mut s = Session::default();
    s.edit.click = false;
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("macro_oscillator"));
    tr.midi_effects.push(MidiEffectSlot::new("clap:test.notes"));
    s
}

fn factory(heard: Heard, block: usize) -> impl FnMut(&InstanceSpec, SlotGen) -> Option<NewInstance> {
    move |spec: &InstanceSpec, _| {
        let plugin: Box<dyn Plugin> = if crate::midi_fx_of_slot(spec.slot).is_some() {
            let (mut f, h) = Fake::new(block);
            f.prepare(48_000.0, block, spec.ch);
            h.lock().at = vec![
                (10, out(OutKind::NoteOn, 0, 60, 7, 0.5)),
                (300, out(OutKind::NoteOn, 1, 64, -1, 1.0)),
                (700, out(OutKind::NoteOff, 0, 60, 7, 0.0)),
                (1500, out(OutKind::NoteOff, 1, 64, -1, 0.0)),
                (1900, out(OutKind::NoteOn, 2, 67, 3, 1.0)),
            ];
            Box::new(f)
        } else {
            Box::new(Recorder { frame: 0, heard: Arc::clone(&heard) })
        };
        Some(NewInstance { plugin, refused_state: false })
    }
}

fn notes(h: &Heard) -> Vec<(u64, &'static str, u8, u16, i32)> {
    h.lock()
        .unwrap()
        .iter()
        .filter_map(|(f, e)| match e.message {
            Message::NoteOn { key, .. } => Some((*f, "on", e.channel, u16::from(key), e.note_id)),
            Message::NoteOff { key, .. } => Some((*f, "off", e.channel, u16::from(key), e.note_id)),
            _ => None,
        })
        .collect()
}

#[test]
fn an_offline_render_gives_the_same_frames_as_playback() {
    let want = vec![(10, "on", 0, 60, 0), (300, "on", 1, 64, 1), (700, "off", 0, 60, 0), (1500, "off", 1, 64, 1), (1900, "on", 2, 67, 2)];
    let s = session();
    // Offline: the render's own blocks of 512.
    let offline: Heard = Arc::default();
    let mut make = factory(Arc::clone(&offline), 512);
    let mut render = RangeRender::build_with(&s, None, Range::new(0, 2048), 512, None, &mut make).unwrap();
    while render.next_block().is_some() {}
    assert_eq!(notes(&offline), want, "offline");
    // Playback at host blocks of 1, 7, 64 and 512 (an engine whose largest block is 512).
    for size in [1usize, 7, 64, 512] {
        let live: Heard = Arc::default();
        let target = RenderTarget { device: DeviceGeneration(4), sample_rate: 48_000, max_block: 512, out_channels: 0, in_channels: 0 };
        let mut mix = MixEngine::realtime(target);
        let mut comp = PlanCompiler::new(target, PlanLimits::default());
        let mut make = factory(Arc::clone(&live), 512);
        let mut env = comp.compile(Arc::new(s.clone()), PlanGeneration::EMPTY, PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
        assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
        comp.reclaim(env);
        let mut io = vec![vec![0.0; 512]; 2];
        let mut pos = 0i64;
        while pos < 2048 {
            let n = size.min(usize::try_from(2048 - pos).unwrap());
            mix.render_active(pos, n, &mut io);
            pos += n as i64;
        }
        assert_eq!(notes(&live), notes(&offline), "playback at block {size} equals the offline render");
    }
}
