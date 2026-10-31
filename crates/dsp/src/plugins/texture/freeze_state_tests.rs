use super::*;
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn running(sr: f32, quality: f32, mono: bool, delay: bool) -> TextureFx {
    let mut t = TextureFx::new();
    for (id, v) in [
        ("quality", quality),
        ("mono_input", f32::from(mono)),
        ("time", 0.0),
        ("dry_wet", 100.0),
        ("reverb", 30.0),
        ("input_gain", 1.0),
        ("size", if delay { 100.0 } else { 70.0 }),
    ] {
        t.set_param(id, v);
    }
    t.prepare(sr, 250, 2);
    let mut io = vec![vec![0.0; 250]; 2];
    for block in 0..240 {
        for (c, channel) in io.iter_mut().enumerate() {
            for (n, x) in channel.iter_mut().enumerate() {
                *x = ((block * 250 + n) as f32 * (0.017 + c as f32 * 0.003)).sin() * 0.4;
            }
        }
        t.process(&mut io, 250);
    }
    t.set_param("freeze", 1.0);
    if delay {
        t.set_param("time", 80.0);
    }
    for _ in 0..5 {
        t.process(&mut io, 250);
    }
    t
}

#[test]
fn frozen_continuation_is_bit_exact_at_every_quality_and_layout() {
    for sr in [44_100.0, 48_000.0] {
        for quality in 0..4 {
            for mono in [false, true] {
                for delay in [false, true] {
                    let label = format!("{sr} quality {quality} mono {mono} delay {delay}");
                    let mut before = running(sr, quality as f32, mono, delay);
                    let data = before.save_state().expect("frozen state");
                    let r = Recording::parse(&data).expect("metadata");
                    assert_eq!(r.pcm.len(), r.span as usize * r.channels as usize * 4, "{label}");
                    let mut after = TextureFx::new();
                    assert!(after.load_state(&data), "restore {label}");
                    after.prepare(sr, 250, 2);
                    after.reset(); // A newly opened graph may reset before its first block.
                    let mut heard = false;
                    for block in 0..20 {
                        let mut a = vec![vec![0.0; 250]; 2];
                        let mut b = a.clone();
                        before.process(&mut a, 250);
                        after.process(&mut b, 250);
                        heard |= a.iter().flatten().any(|x| *x != 0.0);
                        assert!(a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()), "{label} block {block}");
                    }
                    assert!(heard, "{label}: non-silent evidence");
                }
            }
        }
    }
}

#[test]
fn a_capture_loan_is_filled_and_returned_without_allocation_or_free() {
    let mut t = running(44_100.0, 1.0, true, false);
    t.set_param("buffer", 4.0);
    t.reset();
    let (capture, mut reader) = channel();
    t.capture = Some(capture);
    let loan = reader.buffer.take().unwrap();
    reader.requests.push(loan).unwrap();
    let mut io = vec![vec![0.0; 250]; 2];
    let ((), stats) = measure(|| t.process(&mut io, 250));
    assert!(stats.is_zero(), "capture: {stats:?}");
    let packet = reader.replies.pop().expect("answered at block boundary");
    let r = Recording::parse(&packet).unwrap();
    assert_eq!(r.span, 2_048_000);
    assert_eq!(r.channels, 1);
    assert_eq!(r.rate, 48_000);
}

#[test]
fn freeze_off_has_no_state_and_capture_clears_a_previous_reference() {
    let mut t = TextureFx::new();
    assert!(t.save_state().is_none());
    let (capture, mut reader) = channel();
    t.capture = Some(capture);
    reader.requests.push(reader.buffer.take().unwrap()).unwrap();
    t.process(&mut [vec![0.0; 12], vec![0.0; 12]], 12);
    assert!(reader.replies.pop().unwrap().is_empty());
}

#[test]
fn truncated_and_wrong_rate_state_is_rejected_without_changing_the_device() {
    let mut t = running(48_000.0, 0.0, false, false);
    let packet = t.save_state().unwrap();
    for end in [0, 8, HEADER - 1, HEADER, packet.len() - 1] {
        assert!(!t.load_state(&packet[..end]));
    }
    let mut wrong = packet.clone();
    wrong[12..16].copy_from_slice(&48_000u32.to_le_bytes());
    assert!(!t.load_state(&wrong));
    assert_eq!(t.save_state().unwrap(), packet);
}

#[test]
fn idle_capture_keeps_the_frozen_recording_without_rendering_or_allocating() {
    let mut t = running(48_000.0, 3.0, false, false);
    t.fade = Fade::In(250);
    let before = t.save_state().unwrap();
    let (capture, mut reader) = channel();
    t.capture = Some(capture);
    reader.requests.push(reader.buffer.take().unwrap()).unwrap();
    let ((), stats) = measure(|| t.service_idle_state());
    assert!(stats.is_zero(), "idle capture: {stats:?}");
    assert_eq!(reader.replies.pop().unwrap(), before);
}
