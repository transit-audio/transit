use super::*;

/// Blip length in seconds.
const LEN_S: f64 = 0.03;

/// Sound settings: gain (session level times the transport trim), then the accent and normal
/// (hz, velocity) pairs.
fn sound(s: &Session, trim: f32) -> (f64, (f64, f64), (f64, f64)) {
    let gain = f64::from(db_to_gain(s.edit.value("click.volume_db", -6.0).clamp(-60.0, 12.0) as f32)) * f64::from(trim);
    let accent = (note_hz(s.edit.value("click.accent_note", 84.0)), s.edit.value("click.accent_velocity", 127.0));
    let normal = (note_hz(s.edit.value("click.normal_note", 72.0)), s.edit.value("click.normal_velocity", 100.0));
    (gain, accent, normal)
}

/// The count-in's beats: `elapsed` samples of the count-in precede this block. Allocation-free.
pub fn render_count_in(s: &Session, c: CountInClick, frames: usize, main: &mut [Vec<f32>], trim: f32) {
    let len = (LEN_S * s.sample_rate.as_f64()) as i64;
    let (gain, accent, normal) = sound(s, trim);
    let beat = c.beat.max(1);
    let end = c.elapsed.saturating_add(frames as i64);
    // A blip that began in an earlier block still rings into this one.
    let mut k = (c.elapsed.saturating_sub(len)).max(0) / beat;
    while k.saturating_mul(beat) < end {
        let at = k * beat;
        if at.saturating_add(len) > c.elapsed {
            let first = k % i64::from(c.per_bar.max(1)) == 0;
            let (hz, vel) = if first { accent } else { normal };
            let amp = gain * (vel.clamp(1.0, 127.0) / 127.0);
            blip(main, at - c.elapsed, frames, hz, amp as f32, s.sample_rate.as_f64());
        }
        k += 1;
    }
}

pub fn render(s: &Session, pos: Samples, frames: usize, main: &mut [Vec<f32>], trim: f32) {
    let sr = s.sample_rate;
    let rate = sr.as_f64();
    let len = (LEN_S * rate) as i64;
    let end = pos.saturating_add(frames as i64);
    let (gain, accent, normal) = sound(s, trim);
    // Start one blip length early so a click that began in an earlier block still rings into this one.
    let t0 = s.tempo.samples_to_ticks(pos.saturating_sub(len), sr).max(0).saturating_sub(1);
    let mut tick = t0.saturating_sub(s.tempo.bar_beat_at_tick(t0).tick);
    // Blocks hold a few beats at most; the guard only protects against a malformed tempo map.
    for _ in 0..4096 {
        let at = s.tempo.tick_to_samples(tick, sr);
        if at >= end {
            break;
        }
        if at.saturating_add(len) > pos {
            let (hz, vel) = if s.tempo.bar_beat_at_tick(tick).beat == 1 { accent } else { normal };
            let amp = gain * (vel.clamp(1.0, 127.0) / 127.0);
            blip(main, at - pos, frames, hz, amp as f32, rate);
        }
        tick = tick.saturating_add(s.tempo.meter_at_tick(tick).ticks_per_beat());
    }
}

/// Adds one decaying sine blip whose first sample sits `rel` frames into the block (negative
/// when it began before the block) to every channel.
fn blip(main: &mut [Vec<f32>], rel: i64, frames: usize, hz: f64, amp: f32, rate: f64) {
    let len = (LEN_S * rate) as usize;
    let w = std::f64::consts::TAU * hz / rate;
    let first = usize::try_from(-rel).unwrap_or(0);
    for i in first..len {
        let Ok(at) = usize::try_from(rel + i as i64) else { continue };
        if at >= frames {
            break;
        }
        let env = (-6.0 * i as f64 / len as f64).exp() as f32;
        let v = ((w * i as f64).sin() as f32) * env * amp;
        for ch in main.iter_mut() {
            if let Some(d) = ch.get_mut(at) {
                *d += v;
            }
        }
    }
}

/// Click pitch for a MIDI note setting (clamped to the MIDI range).
fn note_hz(note: f64) -> f64 {
    transit_dsp::midi_to_hz(note.clamp(0.0, 127.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::PlanRig;

    /// Renders `len` frames from zero the way playback does (metronome on), in blocks of `block`.
    fn render_playback(s: &Session, len: usize, block: usize, recording: bool) -> Vec<Vec<f32>> {
        let mut rig = PlanRig::new(block);
        rig.publish(s);
        rig.eng.metronome = true;
        rig.eng.recording = recording;
        let mut out = vec![vec![0.0f32; len]; 2];
        let mut done = 0usize;
        while done < len {
            let n = (len - done).min(block);
            let tmp = rig.render(done as i64);
            for (o, t) in out.iter_mut().zip(tmp.iter()) {
                if let (Some(d), Some(src)) = (o.get_mut(done..done + n), t.get(..n)) {
                    d.copy_from_slice(src);
                }
            }
            done += n;
        }
        out
    }

    #[test]
    fn click_sounds_on_every_beat_only_in_playback() {
        let mut s = Session::default();
        let beat = s.sample_rate.as_f64() as usize * 60 / 120; // 120 BPM quarter notes
        let len = beat * 8;
        s.edit.click = true;
        // Offline renders (bounces, stems) never print the metronome.
        let offline = render_range(&s, Range::new(0, len as i64), 512);
        assert!(offline.iter().all(|c| c.iter().all(|x| *x == 0.0)));

        let out = render_playback(&s, len, 512, false);
        for b in 0..8usize {
            let at = b * beat;
            let on: f32 = out[0][at..at + 200].iter().map(|x| x.abs()).fold(0.0, f32::max);
            let off: f32 = out[0][at + beat / 2..at + beat / 2 + 200].iter().map(|x| x.abs()).fold(0.0, f32::max);
            assert!(on > 0.1, "beat {b} should click, peak {on}");
            assert_eq!(off, 0.0, "between beats must stay silent");
            assert_eq!(out[0][at], out[1][at], "both channels get the click");
        }
        // Blocks that start mid-beat render the same clicks as a single pass.
        let split = render_playback(&s, len, 97, false);
        assert_eq!(out, split);
    }

    #[test]
    fn click_only_during_record_follows_recording() {
        let mut s = Session::default();
        s.edit.click = true;
        s.edit.set_flag("click.only_during_record", true);
        let idle = render_playback(&s, 48_000, 512, false);
        assert!(idle.iter().all(|c| c.iter().all(|x| *x == 0.0)));
        let rec = render_playback(&s, 48_000, 512, true);
        assert!(rec.iter().any(|c| c.iter().any(|x| x.abs() > 0.1)));
    }
}
