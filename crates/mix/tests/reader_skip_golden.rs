//! #276: no-pedal skip passage captured on 93efae7f before changing the skip release.
#![allow(clippy::unwrap_used)]

#[path = "support/reader_skip.rs"]
mod support;
use support::Rig;
use transit_model::TrackKind;
use transit_rt::testing::CountingAlloc;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

#[test]
fn a_skip_without_a_pedal_matches_the_pre_change_render() {
    let mut rig = Rig::new(0, TrackKind::Instrument);
    let mut bytes = Vec::new();
    for block in 0..16 {
        match block {
            0 | 8 => rig.on(),
            2 => rig.bypass(true),
            5 => rig.bypass(false),
            9 => rig.off(),
            _ => {}
        }
        rig.block();
        if matches!(block, 0 | 8) {
            rig.sounding();
        }
        if block == 15 {
            rig.silent();
        }
        bytes.extend(rig.out.iter().flatten().flat_map(|v| v.to_le_bytes()));
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/reader_skip_no_pedal.f32");
    if std::env::var_os("CAPTURE_276_GOLDEN").is_some() {
        std::fs::write(&path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).unwrap();
    assert_eq!(bytes.len(), golden.len());
    for (i, (got, want)) in bytes.as_chunks::<4>().0.iter().zip(golden.as_chunks::<4>().0).enumerate() {
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            assert_eq!(got, want, "pre-change sample {i}");
        } else {
            // The existing plan goldens allow platform libm rounding differences.
            let (got, want) = (f32::from_le_bytes(*got), f32::from_le_bytes(*want));
            assert!((got - want).abs() <= 3e-4 + 2e-3 * want.abs(), "pre-change sample {i}");
        }
    }
}
