//! #290: reconciling colliding note starts/releases and shaper switches allocates nothing.

use transit_dsp::create;
use transit_rt::testing::{AllocStats, CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

#[test]
fn colliding_releases_and_switches_allocate_nothing() {
    let mut p = create("macro_oscillator").unwrap();
    p.prepare(48_000.0, 256, 2);
    let mut io = [vec![0.0; 256], vec![0.0; 256]];
    let ((), stats) = measure(|| {
        for round in 0..96 {
            assert!(p.set_param("shaper", if round % 3 == 0 { 0.0 } else { 1.0 }));
            p.note_on(0, 55, 100);
            p.process(&mut io, 240);
            p.note_on(5, 55, 91);
            p.note_off(6, 55);
            p.process(&mut io, 240);
        }
        // End on the shaped path: enough sample frames for the default amp release to rest.
        for _ in 0..100 {
            p.process(&mut io, 256);
        }
    });
    assert_eq!(stats, AllocStats::default());
    assert_eq!(p.voice_activity(), 0, "the final shaped release rests");
}
