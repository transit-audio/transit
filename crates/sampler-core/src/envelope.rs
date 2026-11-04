//! The new-patch amp envelope and the fixed ramps (ADR 0041 "Playback, controls and modulation").
//!
//! Durations are whole output frames, `round(ms * rate / 1000)`, worked out once when the settings
//! change ([`EnvelopeFrames::from_ms`]). Linear attack reaches 1 on its `A`th frame (`k / A` on frame
//! `k = 1..=A`); zero attack starts at 1. Linear decay then reaches sustain on its `D`th frame; zero
//! decay selects sustain at once. Release captures the current gain `G` and emits
//! `G * (1 - (j + 1) / R)` for `j = 0..R`, then the voice retires (its last frame is 0); zero release
//! retires at the event. The choke fade is the same rule with `R` = [`crate::CHOKE_FRAMES`]
//! ([`Ramp`]), applied after shaping. These are not the legacy sampler's exponential ADSR, which
//! stays with the legacy renderer in dsp.

use crate::math;

/// An envelope's durations in output frames and its sustain level.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EnvelopeFrames {
    pub attack: u32,
    pub decay: u32,
    /// 0..1.
    pub sustain: f32,
    pub release: u32,
}

/// Milliseconds as whole frames at `rate`: `round(ms * rate / 1000)`, never negative.
#[must_use]
pub fn ms_to_frames(ms: f32, rate: f32) -> u32 {
    let f = math::round(f64::from(ms) * f64::from(rate) / 1000.0);
    if f.is_finite() && f > 0.0 { f.min(f64::from(u32::MAX)) as u32 } else { 0 }
}

impl EnvelopeFrames {
    /// The frames of attack, decay and release times in ms at `rate`, sustain 0..1.
    #[must_use]
    pub fn from_ms(attack_ms: f32, decay_ms: f32, sustain: f32, release_ms: f32, rate: f32) -> EnvelopeFrames {
        EnvelopeFrames {
            attack: ms_to_frames(attack_ms, rate),
            decay: ms_to_frames(decay_ms, rate),
            sustain: if sustain.is_finite() { sustain.clamp(0.0, 1.0) } else { 1.0 },
            release: ms_to_frames(release_ms, rate),
        }
    }
}

/// The envelope's stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// `from * (1 - j / len)`: frame `j` (1..=len) of a linear ramp from `from` to 0.
#[inline]
fn fall(from: f32, j: u32, len: u32) -> f32 {
    from * (1.0 - j as f32 / len as f32)
}

/// A voice's amp envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Envelope {
    stage: Stage,
    /// Frames emitted in the stage.
    pos: u32,
    level: f32,
    /// The gain the release started from.
    from: f32,
}

impl Default for Envelope {
    fn default() -> Self {
        Envelope::new()
    }
}

impl Envelope {
    /// An idle envelope.
    #[must_use]
    pub const fn new() -> Envelope {
        Envelope { stage: Stage::Idle, pos: 0, level: 0.0, from: 0.0 }
    }

    /// A new note: attack from 0, or (zero attack) decay from 1, or (zero decay too) sustain.
    pub fn start(&mut self, f: &EnvelopeFrames) {
        *self = if f.attack > 0 {
            Envelope { stage: Stage::Attack, pos: 0, level: 0.0, from: 0.0 }
        } else if f.decay > 0 {
            Envelope { stage: Stage::Decay, pos: 0, level: 1.0, from: 0.0 }
        } else {
            Envelope { stage: Stage::Sustain, pos: 0, level: f.sustain, from: 0.0 }
        };
    }

    /// The key let go: release from the current gain (zero release: idle at once).
    pub fn release(&mut self, f: &EnvelopeFrames) {
        if matches!(self.stage, Stage::Idle | Stage::Release) {
            return;
        }
        if f.release == 0 {
            *self = Envelope::new();
        } else {
            *self = Envelope { stage: Stage::Release, pos: 0, level: self.level, from: self.level };
        }
    }

    /// The next frame's gain.
    #[inline]
    pub fn next(&mut self, f: &EnvelopeFrames) -> f32 {
        match self.stage {
            Stage::Idle => self.level = 0.0,
            Stage::Attack => {
                self.pos += 1;
                self.level = self.pos as f32 / f.attack.max(1) as f32;
                if self.pos >= f.attack {
                    self.level = 1.0;
                    self.pos = 0;
                    self.stage = if f.decay > 0 { Stage::Decay } else { Stage::Sustain };
                }
            }
            Stage::Decay => {
                self.pos += 1;
                self.level = 1.0 + (f.sustain - 1.0) * (self.pos as f32 / f.decay.max(1) as f32);
                if self.pos >= f.decay {
                    self.level = f.sustain;
                    self.pos = 0;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.level = f.sustain,
            Stage::Release => {
                self.pos += 1;
                self.level = fall(self.from, self.pos, f.release.max(1));
                if self.pos >= f.release {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }

    /// The gain of the last frame (the matrix's Amp envelope source).
    #[must_use]
    pub fn level(&self) -> f32 {
        self.level
    }

    #[must_use]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// The envelope finished (or never started).
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// Whether the envelope is releasing.
    #[must_use]
    pub fn is_releasing(&self) -> bool {
        self.stage == Stage::Release
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn bits(&self) -> (u8, u32, u32, u32) {
        (self.stage as u8, self.pos, self.level.to_bits(), self.from.to_bits())
    }
}

/// A linear fade to silence over a fixed number of frames: frame `j` (1..=len) is
/// `from * (1 - j / len)`; the choke fade is `Ramp::new(CHOKE_FRAMES)` from 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ramp {
    from: f32,
    pos: u32,
    len: u32,
}

impl Ramp {
    /// A ramp over `len` (>= 1) frames from 1.
    #[must_use]
    pub fn new(len: u32) -> Ramp {
        Ramp { from: 1.0, pos: 0, len: len.max(1) }
    }

    /// The next frame's gain.
    #[inline]
    pub fn advance(&mut self) -> f32 {
        self.pos = (self.pos + 1).min(self.len);
        fall(self.from, self.pos, self.len)
    }

    /// The ramp reached 0 (its last frame has been emitted).
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.pos >= self.len
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn bits(&self) -> (u32, u32, u32) {
        (self.from.to_bits(), self.pos, self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(e: &mut Envelope, f: &EnvelopeFrames, n: usize) -> Vec<f32> {
        (0..n).map(|_| e.next(f)).collect()
    }

    #[test]
    fn attack_decay_and_release_follow_the_linear_rules() {
        let f = EnvelopeFrames { attack: 4, decay: 2, sustain: 0.5, release: 4 };
        let mut e = Envelope::new();
        e.start(&f);
        assert_eq!(run(&mut e, &f, 8), [0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.5, 0.5]);
        e.release(&f);
        assert_eq!(run(&mut e, &f, 6), [0.375, 0.25, 0.125, 0.0, 0.0, 0.0]);
        assert!(e.is_idle());
    }

    #[test]
    fn zero_times_start_at_one_select_sustain_and_retire_at_the_release() {
        let f = EnvelopeFrames { attack: 0, decay: 0, sustain: 0.8, release: 0 };
        let mut e = Envelope::new();
        e.start(&f);
        assert_eq!(run(&mut e, &f, 2), [0.8, 0.8]);
        e.release(&f);
        assert!(e.is_idle());
        let f = EnvelopeFrames { attack: 0, decay: 4, sustain: 0.0, release: 2 };
        e.start(&f);
        assert_eq!(run(&mut e, &f, 2), [0.75, 0.5]);
        e.release(&f);
        assert_eq!(run(&mut e, &f, 3), [0.25, 0.0, 0.0]);
    }

    #[test]
    fn a_release_in_the_attack_falls_from_the_current_gain() {
        let f = EnvelopeFrames { attack: 8, decay: 0, sustain: 1.0, release: 2 };
        let mut e = Envelope::new();
        e.start(&f);
        run(&mut e, &f, 4);
        e.release(&f);
        assert_eq!(run(&mut e, &f, 2), [0.25, 0.0]);
    }

    #[test]
    fn the_choke_ramp_runs_31_32_down_to_zero() {
        let mut r = Ramp::new(32);
        let g: Vec<f32> = (0..32).map(|_| r.advance()).collect();
        for (j, x) in g.iter().enumerate() {
            assert_eq!(*x, (31 - j) as f32 / 32.0, "frame {j}");
        }
        assert!(r.is_done());
    }

    #[test]
    fn durations_round_to_whole_frames() {
        assert_eq!(ms_to_frames(1.0, 48_000.0), 48);
        assert_eq!(ms_to_frames(0.0104, 48_000.0), 0);
        assert_eq!(ms_to_frames(0.0105, 48_000.0), 1);
        assert_eq!(ms_to_frames(-3.0, 48_000.0), 0);
        assert_eq!(ms_to_frames(f32::NAN, 48_000.0), 0);
    }
}
