//! A print only reports newly silenced pre-count content (#702).

use serde_json::json;
use transit_engine::Engine;
use transit_model::{ChannelFormat, Clip, TrackKind};

fn check(command: &str, track_muted: bool, clip_muted: bool) {
    for source in ["mute", "mute_deactivate", "hide", "leave"] {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Source"));
        let audio = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]] };
        let src = transit_engine::io::add_source(s, "Tone", audio, None, transit_audio_io::FileFormat::Wav);
        let id = s.new_clip_id();
        let mut c = Clip::audio(id, "Zone", src, 0, -2400, 4800);
        c.muted = clip_muted;
        let tr = s.track_mut(t).unwrap();
        tr.mixer.mute = track_muted;
        tr.playlist_mut().unwrap().clips.push(c);
        // Audible material after bar 1 must not make a muted zone clip trigger the notice.
        let later = Clip::audio(s.new_clip_id(), "Later", src, 0, 4800, 4800);
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(later);
        let reply = e.execute(command, &json!({"tracks": [t.0], "clips": [id.0], "source": source})).unwrap();
        assert_eq!(reply["tracks"].as_array().unwrap().len(), 1);
        let expected = !track_muted && !clip_muted && source != "leave";
        assert_eq!(
            reply.get("notice").is_some(),
            expected,
            "{command}, source={source}, track_muted={track_muted}, clip_muted={clip_muted}: {reply}"
        );
        if expected {
            assert!(reply["notice"].as_str().unwrap().contains("no longer plays"));
        }
    }
}

#[test]
fn clip_bounce_skips_notice_for_an_already_muted_track() {
    check("clip.bounce", true, false);
}

#[test]
fn clip_bounce_skips_notice_for_an_already_muted_clip() {
    check("clip.bounce", false, true);
}

#[test]
fn track_bounce_skips_notice_for_an_already_muted_track() {
    check("track.bounce", true, false);
}

#[test]
fn track_bounce_skips_notice_for_an_already_muted_clip() {
    check("track.bounce", false, true);
}

#[test]
fn commit_skips_notice_for_an_already_muted_track() {
    check("track.commit", true, false);
}

#[test]
fn commit_skips_notice_for_an_already_muted_clip() {
    check("track.commit", false, true);
}

#[test]
fn every_print_reports_newly_silenced_zone_content() {
    for command in ["clip.bounce", "track.bounce", "track.commit"] {
        check(command, false, false);
    }
}

#[test]
fn bouncing_later_clips_reports_the_unselected_pickup_it_silences() {
    for source in ["mute", "mute_deactivate", "hide", "leave"] {
        for (track_muted, clip_muted) in [(false, false), (true, false), (false, true)] {
            let mut e = Engine::default();
            let s = e.session_mut();
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Source"));
            let audio = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]] };
            let src = transit_engine::io::add_source(s, "Tone", audio, None, transit_audio_io::FileFormat::Wav);
            let mut pickup = Clip::audio(s.new_clip_id(), "Unselected pickup", src, 0, -4800, 2400);
            pickup.muted = clip_muted;
            let later = Clip::audio(s.new_clip_id(), "Selected later", src, 0, 4800, 4800);
            let id = later.id;
            let tr = s.track_mut(t).unwrap();
            tr.mixer.mute = track_muted;
            tr.playlist_mut().unwrap().clips.extend([pickup, later]);
            let render = |e: &Engine| transit_mix::render_range(e.session(), transit_time::Range::new(-4800, -2400), 256);
            let audible = !track_muted && !clip_muted;
            assert_eq!(render(&e).iter().flatten().any(|v| *v != 0.0), audible);
            let reply = e.execute("clip.bounce", &json!({"clips": [id.0], "source": source})).unwrap();
            assert_eq!(reply["tracks"].as_array().unwrap().len(), 1);
            assert_eq!(render(&e).iter().flatten().any(|v| *v != 0.0), audible && source == "leave");
            if audible && source != "leave" {
                assert_eq!(
                    reply["notice"], "The source track's content before bar 1 no longer plays; choose Leave as the source to keep it sounding.",
                    "{source}: {reply}"
                );
            } else {
                assert!(reply.get("notice").is_none(), "{source}: {reply}");
            }
        }
    }
}

fn mixed_zone_clips(command: &str) {
    for source in ["mute", "mute_deactivate", "hide", "leave"] {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Mixed pickups"));
        let audio = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]] };
        let src = transit_engine::io::add_source(s, "Tone", audio, None, transit_audio_io::FileFormat::Wav);
        let mut muted = Clip::audio(s.new_clip_id(), "Muted pickup", src, 0, -4800, 1200);
        muted.muted = true;
        let audible = Clip::audio(s.new_clip_id(), "Audible pickup", src, 0, -2400, 4800);
        let id = audible.id;
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.extend([muted, audible]);
        let reply = e.execute(command, &json!({"tracks": [t.0], "clips": [id.0], "source": source})).unwrap();
        assert_eq!(reply["tracks"].as_array().unwrap().len(), 1);
        assert_eq!(reply.get("notice").is_some(), source != "leave", "{command}, {source}: {reply}");
    }
}

#[test]
fn clip_bounce_reports_a_mix_of_muted_and_unmuted_zone_clips() {
    mixed_zone_clips("clip.bounce");
}

#[test]
fn track_bounce_reports_a_mix_of_muted_and_unmuted_zone_clips() {
    mixed_zone_clips("track.bounce");
}

#[test]
fn commit_reports_a_mix_of_muted_and_unmuted_zone_clips() {
    mixed_zone_clips("track.commit");
}
