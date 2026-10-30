//! Disk-backed sources in the mix (ADR 0035, #187). The streaming thread is faked: each test calls
//! [`DiskSource::pump`] between blocks itself, so what the rings hold depends on the blocks
//! rendered, never on a clock or a scheduler (E49, E62).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan_tests::{BLOCK, Rig};
use super::*;
use std::path::PathBuf;
use transit_audio_io::AudioBuffer;
use transit_model::disk::{DiskFile, DiskSource, Opener, StreamMap};
use transit_model::{ChannelFormat, ClipId, SourceAudio, SourceId, TrackKind};

/// A file held in memory.
struct Fake(Arc<Vec<Vec<f32>>>);

impl DiskFile for Fake {
    fn read_at(&mut self, at: u64, frames: usize, out: &mut Vec<Vec<f32>>) -> std::io::Result<usize> {
        out.resize_with(self.0.len(), Vec::new);
        let at = usize::try_from(at).unwrap();
        let mut n = 0;
        for (o, d) in out.iter_mut().zip(self.0.iter()) {
            o.clear();
            o.extend_from_slice(d.get(at..d.len().min(at + frames)).unwrap_or_default());
            n = o.len();
        }
        Ok(n)
    }
}

fn content(frames: usize) -> Vec<Vec<f32>> {
    vec![(0..frames).map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5).collect(), (0..frames).map(|i| (i % 300) as f32 / 300.0).collect()]
}

fn fake_source(data: &[Vec<f32>], window: usize) -> Arc<DiskSource> {
    let data = Arc::new(data.to_vec());
    let frames = data[0].len() as u64;
    let opener: Opener = Arc::new(move || Ok(Box::new(Fake(Arc::clone(&data))) as Box<dyn DiskFile>));
    DiskSource::with_window(PathBuf::from("take.wav"), 2, frames, 48_000, window, opener)
}

/// A mono track playing two clips of one stereo source: a plain one with fades, and a looped,
/// stretched one. `audio` is the source.
fn session(audio: Arc<SourceAudio>) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.pool.insert(SourceId(1), audio);
    let mut a = Clip::audio(ClipId(10), "a", SourceId(1), 300, 100, 3_000);
    a.fade_in.len = 200;
    a.fade_out.len = 300;
    let mut b = Clip::audio(ClipId(11), "b", SourceId(1), 50, 3_500, 4_000);
    b.set_loop(Some(ClipLoop { start: 100, end: 1_100 }));
    b.stretch = 1.25;
    let pl = s.track_mut(t).unwrap().playlist_mut().unwrap();
    pl.clips.push(a);
    pl.clips.push(b);
    (s, t)
}

fn memory(data: &[Vec<f32>]) -> Arc<SourceAudio> {
    Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: data.to_vec() }))
}

/// Render `blocks` (positions) through a live plan on this thread, marked as the audio thread,
/// pumping `disk` after each block when given. Returns the concatenated first output channel.
fn live(s: &Session, blocks: &[i64], disk: Option<&DiskSource>, measure: bool) -> Vec<f32> {
    live_edited(s, None, blocks, disk, measure)
}

/// [`live`], publishing `edit.1` (an edit of `s`) before block `edit.0`. With `measure`, every
/// block is measured for allocation, the first one after a warm-up block where nothing plays.
fn live_edited(s: &Session, edit: Option<(usize, &Session)>, blocks: &[i64], disk: Option<&DiskSource>, measure: bool) -> Vec<f32> {
    mark_audio_thread();
    let mut rig = Rig::new();
    assert_eq!(rig.publish(s), Adoption::Adopted);
    // A plan's first block may set things up; the disk-backed path is measured from its first use.
    rig.eng.render_active(-1_000_000, BLOCK, &mut rig.out);
    let mut out = Vec::new();
    for (i, &pos) in blocks.iter().enumerate() {
        if let Some((at, edited)) = edit
            && at == i
        {
            assert_eq!(rig.publish(edited), Adoption::Adopted);
        }
        if measure {
            let (_, stats) = transit_rt::testing::measure(|| rig.eng.render_active(pos, BLOCK, &mut rig.out));
            assert!(stats.is_zero(), "a disk-backed clip allocated on the audio thread at {pos}: {stats:?}");
        } else {
            rig.eng.render_active(pos, BLOCK, &mut rig.out);
        }
        out.extend_from_slice(&rig.out[0][..BLOCK]);
        if let Some(d) = disk {
            d.pump();
        }
    }
    unmark_audio_thread();
    out
}

/// Clip `a` of [`session`] changed by `f`.
fn edited(s: &Session, f: impl Fn(&mut Clip)) -> Session {
    let mut e = s.clone();
    let t = e.tracks[0].id;
    let clip = e.track_mut(t).unwrap().playlist_mut().unwrap().clips.iter_mut().find(|c| c.id == ClipId(10)).unwrap();
    f(clip);
    e
}

#[test]
fn an_edit_of_a_playing_disk_backed_clip_plays_its_new_mapping_from_the_next_block() {
    let data = content(10_000);
    type Edit = fn(&mut Clip);
    let edits: [(&str, Edit); 4] = [
        ("slip", |c| {
            if let ClipContent::Audio { offset, .. } = &mut c.content {
                *offset += 200;
            }
        }),
        ("extend left", |c| {
            c.start -= 64;
            c.length += 64;
            if let ClipContent::Audio { offset, .. } = &mut c.content {
                *offset -= 64;
            }
        }),
        ("loop", |c| c.set_loop(Some(ClipLoop { start: 200, end: 900 }))),
        ("extend end", |c| c.length += 3_000),
    ];
    for (name, f) in edits {
        let disk = fake_source(&data, 2_048);
        let (ds, _) = session(Arc::new(SourceAudio::disk(Arc::clone(&disk))));
        let (ms, _) = session(memory(&data));
        let (ds2, ms2) = (edited(&ds, f), edited(&ms, f));
        assert_eq!(transit_model::disk::prime_session(&ds, 0), 0);
        // The edit lands at block 4 (1024), the old ring then holds up to clip time 2948.
        let blocks: Vec<i64> = (0..24).map(|b| b * BLOCK as i64).collect();
        let got = live_edited(&ds, Some((4, &ds2)), &blocks, Some(&disk), true);
        let want = live_edited(&ms, Some((4, &ms2)), &blocks, None, false);
        assert_eq!(got[..4 * BLOCK], want[..4 * BLOCK], "{name}: before the edit");
        assert!(disk.underruns() <= 2, "{name}: at most the block of the edit (and the extended end's first block): {}", disk.underruns());
        let bits = |x: &[f32]| x.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&got[5 * BLOCK..]), bits(&want[5 * BLOCK..]), "{name}: the new mapping from the block after the edit");
    }
}

#[test]
fn a_disk_backed_clip_plays_what_memory_plays_without_allocating_once_primed() {
    let data = content(10_000);
    let disk = fake_source(&data, 2_048);
    let (ds, _) = session(Arc::new(SourceAudio::disk(Arc::clone(&disk))));
    let (ms, _) = session(memory(&data));
    assert_eq!(transit_model::disk::prime_session(&ds, 0), 0, "primed before play");
    let blocks: Vec<i64> = (0..36).map(|b| b * BLOCK as i64).collect();
    let got = live(&ds, &blocks, Some(&disk), true);
    let want = live(&ms, &blocks, None, false);
    assert!(want.iter().any(|v| v.abs() > 0.01));
    assert_eq!(got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), want.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
    assert_eq!(disk.underruns(), 0, "the clip ahead was cued in time, the playing one kept filled");
}

#[test]
fn the_transport_loop_point_plays_on_without_a_gap() {
    let data = content(10_000);
    let disk = fake_source(&data, 2_048);
    let (mut ds, _) = session(Arc::new(SourceAudio::disk(Arc::clone(&disk))));
    let (mut ms, _) = session(memory(&data));
    // Cycle from 1024 to 4096: inside the plain clip, ending inside the looped one.
    for s in [&mut ds, &mut ms] {
        s.edit.loop_playback = true;
        s.edit.loop_range = Range::new(4 * BLOCK as i64, 16 * BLOCK as i64);
    }
    assert_eq!(transit_model::disk::prime_session(&ds, 0), 0);
    // Play from 0 to the loop's end, then three laps.
    let mut blocks: Vec<i64> = (0..16).map(|b| b * BLOCK as i64).collect();
    for _ in 0..3 {
        blocks.extend((4..16).map(|b| b * BLOCK as i64));
    }
    let got = live(&ds, &blocks, Some(&disk), true);
    let want = live(&ms, &blocks, None, false);
    assert_eq!(got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), want.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
    assert_eq!(disk.underruns(), 0, "the loop's start was cued before the wrap");
}

#[test]
fn a_late_ring_plays_silence_counts_an_underrun_and_recovers() {
    let data = content(10_000);
    let disk = fake_source(&data, 2_048);
    let (ds, _) = session(Arc::new(SourceAudio::disk(Arc::clone(&disk))));
    let (ms, _) = session(memory(&data));
    let before = transit_model::disk::underruns();
    // Not primed, and the streamer runs only after the first block; the miss is measured too.
    let blocks: Vec<i64> = (0..8).map(|b| b * BLOCK as i64).collect();
    let got = live(&ds, &blocks, Some(&disk), true);
    let want = live(&ms, &blocks, None, false);
    // Block 0 holds the clip's first 156 samples: silence and one underrun; from block 1 on, the ring.
    assert!(want[100..BLOCK].iter().any(|v| v.abs() > 0.0));
    assert!(got[..BLOCK].iter().all(|v| *v == 0.0), "a late ring is silence");
    assert_eq!(disk.underruns(), 1);
    assert!(transit_model::disk::underruns() > before, "counted for the diagnostics row");
    assert_eq!(got[BLOCK..], want[BLOCK..], "the next block plays");
}

#[test]
fn a_locate_without_priming_counts_one_underrun_and_resumes() {
    let data = content(10_000);
    let disk = fake_source(&data, 2_048);
    let (ds, _) = session(Arc::new(SourceAudio::disk(Arc::clone(&disk))));
    let (ms, _) = session(memory(&data));
    assert_eq!(transit_model::disk::prime_session(&ds, 0), 0);
    let mut blocks: Vec<i64> = (0..4).map(|b| b * BLOCK as i64).collect();
    blocks.extend((20..26).map(|b| b * BLOCK as i64));
    let got = live(&ds, &blocks, Some(&disk), false);
    let want = live(&ms, &blocks, None, false);
    assert_eq!(disk.underruns(), 1, "the block after the jump");
    assert!(got[4 * BLOCK..5 * BLOCK].iter().all(|v| *v == 0.0));
    assert_eq!(got[..4 * BLOCK], want[..4 * BLOCK]);
    assert_eq!(got[5 * BLOCK..], want[5 * BLOCK..]);
}

/// Unique temp dir per test (E51), removed on drop.
struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn export_of_a_short_file_forced_disk_backed_equals_the_in_memory_render_bit_for_bit() {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = Dir(std::env::temp_dir().join(format!(
        "transit-disk-export-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("Take_01.wav");
    let data = content(9_000);
    let opts = transit_audio_io::EncodeOptions {
        format: transit_audio_io::FileFormat::Wav,
        bit_depth: transit_audio_io::BitDepth::Int24,
        dither: false,
        bwf: None,
    };
    let bytes = transit_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: data }, &opts).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    // The same file, decoded into memory and played from disk.
    let (_, decoded) = transit_audio_io::decode(&bytes, Some("wav")).unwrap();
    let disk = DiskSource::open(&path, bytes.len() as u64).unwrap();
    let (ds, _) = session(Arc::new(SourceAudio::disk(disk)));
    let (ms, _) = session(Arc::new(SourceAudio::new(decoded)));
    assert!(!on_audio_thread(), "an export reads the file on its own thread");
    let range = Range::new(0, 8_000);
    for block in [512, 97] {
        let got = render_range(&ds, range, block);
        let want = render_range(&ms, range, block);
        assert!(want[0].iter().any(|v| v.abs() > 0.01));
        let bits = |x: &Vec<Vec<f32>>| x.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&got), bits(&want), "block {block}");
    }
    // Consolidate reads through the same reader.
    let t = ds.tracks[0].id;
    assert_eq!(render_clips(&ds, t, range), render_clips(&ms, t, range));
}

#[test]
fn a_full_request_table_drops_the_request_without_allocating() {
    let data = content(4_000);
    let disk = fake_source(&data, 512);
    mark_audio_thread();
    let maps: Vec<StreamMap> = (0..40u64).map(|c| StreamMap { clip: c, offset: 0, looping: None, stretch: 1.0, length: 4_000 }).collect();
    let (_, stats) = transit_rt::testing::measure(|| {
        for m in &maps {
            disk.underrun(m, 0);
        }
    });
    unmark_audio_thread();
    assert!(stats.is_zero(), "{stats:?}");
    assert_eq!(disk.underruns(), 40);
    disk.pump();
    let cued = maps.iter().filter(|m| disk.stream(m, 0, 16).is_some()).count();
    assert!(cued >= 1 && cued <= transit_model::disk::SLOTS, "{cued}");
}
