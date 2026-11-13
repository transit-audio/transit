//! Audio above full scale survives a save, a save copy and a bundle bit for bit (#519): every
//! in-memory source is stored as a 32-bit float WAV, which keeps samples beyond full scale.

use super::*;

/// Six frames per channel: levels above full scale, a subnormal and -0.0.
fn overs() -> Vec<Vec<f32>> {
    let a = vec![1.5f32, -2.0, 0.25, 1.0e3, f32::from_bits(1), -0.0];
    let b = a.iter().map(|s| -s).collect();
    vec![a, b]
}

fn bits(ch: &[Vec<f32>]) -> Vec<Vec<u32>> {
    ch.iter().map(|c| c.iter().map(|s| s.to_bits()).collect()).collect()
}

/// A session with one stereo audio track playing an in-memory source of [`overs`].
fn with_overs() -> Engine {
    let mut e = Engine::default();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let src = add_source(s, "Hot", AudioBuffer { sample_rate: 48_000, channels: overs() }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::audio(id, "Hot", src, 0, 0, 6));
    e
}

fn source_bits(e: &Engine) -> Vec<Vec<u32>> {
    let s = e.session();
    bits(&s.pool.get(s.sources[0].id).unwrap().buffer.channels)
}

fn scratch(name: &str) -> transit_test_support::TestDir {
    transit_test_support::TestDir::new(&format!("float-overs-{name}"))
}

#[test]
fn a_source_above_full_scale_survives_save_and_reopen_bit_for_bit() {
    let dir = scratch("save");
    assert_eq!(source_bits(&with_overs()), bits(&overs()));
    // Save, Save Session Copy In and the overwriting copy (`save_copy`): each writes the source
    // from memory.
    for (name, how) in [("Save/Song.transit", 0), ("Copy/Song.transit", 1), ("Overwrite/Song.transit", 2)] {
        let path = dir.join(name).to_string_lossy().into_owned();
        let mut e = with_overs();
        let written = match how {
            0 => save_session(&mut e, &path),
            1 => save_session_copy(&e, &path),
            _ => save_copy(&e, &path),
        };
        assert_eq!(written.unwrap(), 1, "{name}");
        let mut r = Engine::default();
        assert_eq!(open_session(&mut r, &path).unwrap(), Vec::<String>::new());
        assert_eq!(source_bits(&r), bits(&overs()), "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_source_above_full_scale_survives_a_bundle_bit_for_bit() {
    let (bytes, _) = session_bundle(with_overs().session()).unwrap();
    let mut r = Engine::default();
    assert_eq!(open_session_bytes(&mut r, "Song.transitz", &bytes).unwrap(), Vec::<String>::new());
    assert_eq!(source_bits(&r), bits(&overs()));
}
