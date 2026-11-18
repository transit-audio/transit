//! The synths' Level default (#753's second stated break, pinned): a synth stored with no `level`
//! (added from Add Track or an import before every creation path stored its table, and never
//! edited) plays at the -12 dB default; a stored `level` is what plays, so a session that stored
//! the old -6 dB default keeps it. No migration writes a value.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use transit_midi::{Note, Sequence};
use transit_model::Insert;

const SYNTHS: [&str; 4] = ["macro_oscillator", "modal_voice", "resonator", "texture_synth"];

/// One instrument track playing a held note through `plugin` with `level` stored (`None`: nothing
/// stored, the shape of an untouched instrument from an older session).
fn session(plugin: &str, level: Option<f32>) -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut seq = Sequence::default();
    seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 });
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 24_000, seq));
    let mut ins = Insert::new(plugin);
    if let Some(db) = level {
        ins.params.insert("level".into(), db);
    }
    tr.instrument = Some(ins);
    s
}

fn render(plugin: &str, level: Option<f32>) -> Vec<f32> {
    render_range(&session(plugin, level), Range::new(0, 12_000), 512).swap_remove(0)
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn a_synth_with_no_stored_level_plays_at_minus_12_db() {
    for plugin in SYNTHS {
        let none = render(plugin, None);
        assert!(peak(&none) > 0.0, "{plugin} sounds");
        let at_default = render(plugin, Some(-12.0));
        assert!(none.iter().zip(&at_default).all(|(a, b)| a.to_bits() == b.to_bits()), "{plugin}: no stored level plays as -12 dB, bit for bit");
        assert!(none.iter().zip(&render(plugin, Some(-6.0))).any(|(a, b)| a.to_bits() != b.to_bits()), "{plugin}: not as the old -6 dB");
    }
    // The Macro Oscillator's Level is a plain gain after its voices: 12 dB under unity.
    let ratio = peak(&render("macro_oscillator", None)) / peak(&render("macro_oscillator", Some(0.0)));
    assert!((ratio - transit_dsp::db_to_gain(-12.0)).abs() < 1.0e-5, "-12 dB: {ratio}");
}

#[test]
fn a_stored_level_is_what_plays_and_the_old_default_is_kept() {
    // A session that stored the -6 dB default (every value written, as `mix.instrument` and the
    // demo always did) plays at -6 dB, 6 dB over a new instance.
    let ratio = peak(&render("macro_oscillator", Some(-6.0))) / peak(&render("macro_oscillator", None));
    assert!((ratio - transit_dsp::db_to_gain(6.0)).abs() < 1.0e-5, "+6 dB over the default: {ratio}");
}
