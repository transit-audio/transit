# ADR 0035: Disk-backed audio sources

- **Status:** proposed 2026-10-30 (issue #187, option 2, decided by the maintainer on 2026-10-29 with option 3 first, which landed as #192). Slice 1 is implemented: the source variant, the streamer, placement of long takes, the mix's ring reader, the "Disk underruns" row, the peaks sidecar and offline reads through the same file.
- **Date:** 2026-10-30
- **Amends:** the engine's "audio is decoded into the session's pool the moment it enters it, so playback and bounce read nothing from disk" (`crates/engine/src/io.rs`, `file_access.rs`): a disk-backed source is read from its file, never on the audio thread ([ADR 0029](0029-strip-job-slots.md) keeps holding: the callback does no IO, takes no lock and allocates nothing).
- **Reopen when:** a disk can not keep a ring ahead at the reference session's track count (the underrun row says so on the reference hardware), compressed formats need to stream, recording must write into a disk-backed source while it records, or editing needs to change a disk-backed source's audio in place.

## Context

Every source lives fully decoded in memory (`SourceAudio::buffer`). The decoder refuses more than 1 Gi samples (`MAX_DECODE_SAMPLES`), which is 6.2 h of mono at 48 kHz, so the eight-hour #130 endurance takes were never placed (#187); #192 made that failure loud and kept the file reachable from the session (K91). The limit is a consequence of holding sources in RAM: an 8 h mono take would need 5.5 GB to place at all.

Convention (transit follows what Ableton and Logic agree on): both stream audio from disk with a RAM cache and draw from waveform overview files kept beside the audio. Ableton Live plays samples from disk unless a clip's "RAM" switch (RAM mode) loads it into memory, shows a "Disk Overload" indicator in the Control Bar when the disk cannot keep up, and writes `.asd` analysis files (waveform overview, warp markers) next to each sample. Logic Pro streams audio regions from disk, says "Disk is too slow" when reads fall behind, and keeps waveform overview files for each audio file. Neither stalls playback for a slow read: the region drops out and the user is told.

## Decision

1. **Two kinds of source.** A source is in memory (today's `AudioBuffer`) or disk-backed: `SourceAudio::disk` holds a `DiskSource` (`crates/model/src/disk.rs`) and the buffer has only the rate and one empty vector per channel. `Source::disk` (serialized, written only when set) marks it so a save never rewrites the file and an open reads it from disk again.
2. **The audio thread only drains rings.** Each disk-backed source has fixed ring slots (`SLOTS` = 8), each a stream of one clip's content from a clip time on: `channels × window` samples (2 s at the source rate, at least 16 Ki frames), allocated the first time the slot is used, off the audio thread. A ring is keyed by clip time and holds the samples already mapped through the clip's loop and stretch, by the same function the in-memory read uses (`StreamMap::sample`), so both produce the same values. The audio thread reads a ring through atomics only (`DiskSource::stream`): no allocation, no lock, no IO. The protocol: the streamer writes clip times past `head` and publishes `head` (Release), and never writes more than a window ahead of the reader's `read`, so it only overwrites what the reader has passed; the reader holds `busy` while it reads, and the streamer re-cues a slot only when it is not busy (both SeqCst).
3. **A streaming thread reads ahead.** One process-wide thread (`transit-disk-stream`) passes over every registered source every 5 ms (`DiskSource::pump`): it takes the audio thread's requests (a fixed table of 16 lock-free entries), gives each a free slot or the one idle longest, and tops every live ring up to a window ahead of its reader. The audio thread asks for a stream when a clip's ring is short, when a disk-backed clip starts within half a window after the block, and, with Cycle on, for each clip under the cycle's start when the block ends within half a window before the cycle's end, so a transport loop finds its start already read.
4. **Primed before play.** Before Play and Locate the control side primes every disk-backed clip the playhead reaches within a window, and the cycle's start (`prime_session`): it reads those rings on the calling thread, the way the transport waits for a plan. A ring that is still short when the callback needs it plays silence for that clip and counts a **Disk underrun** (a new row in Callback telemetry and Copy diagnostics, defined in `docs/help/telemetry.toml`); the callback never waits for the disk.
5. **Which sources are disk-backed.** A recorded take longer than the setting (`disk.stream_minutes` in the session's values, 10 minutes by default; `TRANSIT_DISK_STREAM_MINUTES` overrides it for the process) or over `MAX_DECODE_SAMPLES` is placed as one clip on one disk-backed source (#192's refusal path becomes "placed, disk-backed"). A source saved disk-backed, or too long to decode, opens disk-backed. Takes that an older build left in `Session::unplaced_takes` are placed on open when the file is still there: at the session's start, on the track the file is named after (`<track>_<NN>.wav`) or a new one, with a note saying to move it.
6. **Peaks.** A disk-backed source's waveform is drawn from bucketed min/max peaks (1024 frames a bucket), built once in the background by reading the file and cached beside it in a `<file>.peaks` sidecar that records the file's length, modification time and a hash of its first 64 KiB; a sidecar that does not match is rebuilt. The waveform shows nothing until the peaks are there.
7. **Offline reads through the same file.** Bounce, export and consolidate render off the audio thread and read the file synchronously through the same reader (`DiskSource::with_sync`), with the same sampling function, so an export of a disk-backed clip equals the in-memory render bit for bit.
8. **Slice 1 limits.** Only native PCM (WAV, BWF, RF64, AIFF, read through #211's `BlockReader`) at the session's rate streams; a compressed or other-rate file stays in memory, and one too long to decode is refused as before (#192's message, now saying why it cannot stream). A take becomes disk-backed when recording stops. Operations that rewrite audio (reverse, other AudioSuite renders, sampler loads) do not read disk-backed sources yet.

## Consequences

- A take of any length is placed; RAM no longer bounds a session's length. An 8 h mono take costs a few rings (384 KB a channel each at 48 kHz) and its peaks (11 MB a channel), not 5.5 GB.
- The audio thread's contract is unchanged: the ring read is allocation-free and lock-free, proven by the no-alloc callback test with a disk-backed clip playing.
- A slow disk is a counted, audible dropout of that clip, never a late callback. The counter is process-wide.
- The streamer is a background thread per process and a peaks thread per newly registered source; tests do not start them and drive `DiskSource::pump` per block instead (E49, E62).
- K91 is resolved for PCM takes at the session's rate.

## Alternatives considered

- **Split long takes over several in-memory sources (#187 option 1).** Keeps the RAM cost, which is the real limit.
- **Memory-map the file.** Page faults on the audio thread are disk IO by another name.
- **Read on the audio thread with a deadline.** Violates ADR 0029's contract; one slow read is a late callback for every track.
- **Stall the callback until the ring is filled.** A slow disk would then stop the whole mix; both Ableton and Logic drop the region instead.

## Slice 2 and later

Compressed formats streamed (a decoder thread per stream), writing a recording directly into a disk-backed source, AudioSuite and sampler reads through the reader, the stream setting in the UI, prefetch sized from the device period, and a benchmark of underruns at the reference track count.
