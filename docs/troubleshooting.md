# Troubleshooting and bug reports

When something goes wrong, the log file says what transit was doing. Attach it to the report.

## Find the log

transit writes `transit.log` in the `logs` folder of its settings folder:

- Windows: `%APPDATA%\transit\logs\transit.log`
- macOS: `~/Library/Application Support/transit/logs/transit.log`
- Linux: `~/.config/transit/logs/transit.log` (or `$XDG_CONFIG_HOME/transit/logs/transit.log`)

The first lines of a run name the file, so the path is also at the top of the log itself. Each launch
moves the previous log to `transit.1.log` (and that one to `transit.2.log`), so the log of a run that
crashed survives the next launch. Copy it before you start transit again if you need more than the two
previous runs. `TRANSIT_CONFIG_DIR` moves the whole settings folder.

## What the log holds by default

Without any setting the log has one line for each of these:

- the audio device that opened: backend (`native` or `cpal`), host (`WASAPI`, `ASIO`, `ALSA`, ...),
  device name, mode, the sample rate and buffer you asked for next to what the device gave, and the
  latency the driver reports (output and input);
- each MIDI port opened or closed;
- the plug-in scan (how many plug-ins, how long) and the time each plug-in instance took to create;
- each session opened: path, format version, tracks, clips, length, the plug-ins it needs;
- at Stop, the callbacks that ran over budget during the run, when there were any.

Warnings and errors from every part of the program are always in the log.

## Reporting a problem

Run `transit --report` and attach the file it writes (or press Copy diagnostics in Audio Devices and paste). It holds your system and audio stack versions, the devices transit sees and the end of the log, and nothing from your sessions.

Reading it:

- **ASIO drivers.** Only the driver transit runs is described (opening another one would stop it). Every other installed driver reads `not probed (ASIO drivers are probed one at a time; the open driver is ...)`, or just `not probed` when transit runs none (always under `transit --report`), with no channel count or rates: that is not a broken driver. A driver that was probed reads `0 ch (the driver has no inputs)` for a direction it lacks, and `probe failed` when the probe did not succeed (the Note line under it says why).
- **Backend lines** (under Open devices) show each backend's own counters. WASAPI (render and capture): skipped wakes, `min padding` (output) and `max packets` (input). ASIO: overloads, busy skips, position jumps and resets the driver reported, and, for the output and input of one driver, one line with each direction's own figures as `out / in` (wakes, gap over period, service, faults) and the driver's shared counters once; two lines when the two directions are not running the same callbacks (one stopped). **`gap over period`** (WASAPI render, ASIO and JACK print it under this one name) is how much longer than one period the time since the previous wake or callback was, never negative; the first wake has nothing to be measured against. It is not a measure of the device clock: a driver that delivers in bursts shows up to a period there with nothing late. WASAPI capture keeps its own name, `late`, because it is measured differently: against the end of the first packet waiting at the wake. The `wakes` figure is a rate per second over the last samples of the Audio Devices window; it is left out until two samples exist (in a report taken without the window open, for instance). With ASIO, a USB interface that delivers on 1 ms USB frames shows up to a period in `gap over period` with nothing late (64 frames at 48 kHz: callbacks 1, 1 and 2 ms apart). Overloads, position jumps and over-budget callbacks (under Callback telemetry) are what a dropout looks like.
- **Longest callback** (under Callback telemetry) says what the longest callback since open was doing, and the line under it, `Longest callbacks`, keeps the longest three. The audio callback writes a phase marker (`phase`: service, MIDI dispatch, strip render, mixdown, aux render, master render, output copy, device I/O; `strip`: the track index; `at`: when the phase last changed) with relaxed stores, no allocation and no lock. When a callback ranks among the longest, the report keeps the phase and strip of the **longest stretch between two phase changes** (`longest_gap`), because the marker is back to idle when the callback ends. The report resolves the strip to the track name and the plug-in display name (the instrument, else the active inserts) from the session, so `Longest callback 19.4 ms during Bass: Serum (strip render); plug-in loading: no; editor open: no (preempted)` reads as: the callback spent its longest stretch rendering the track Bass. `plug-in loading` and `editor open` say a plug-in was being created, or an editor was open, during that callback. A track renamed or removed after the callback reads by its current index (`strip 7` when the index is gone). With the render pool on, strips run in parallel and the marker holds whichever strip entered last. **Classification**, printed only for a callback over its nominal budget, from `longest_gap` against the callback's work time and its budget: **`(slow)`** when `longest_gap` is at most half of the work time (the time was spread over several phases or strips: the work is heavy); **`(stalled in one phase)`** when it is more than half of the work time but within the budget (one heavy call, usually a plug-in's process call, dominated); **`(preempted)`** when it is more than half of the work time and exceeds the whole nominal budget (the thread was not running: a DPC, an interrupt, a higher-priority thread, or a call that blocked). The marker cannot tell a descheduled thread from a single call that did not return, so read the strip and plug-in named beside it: a plug-in that blocks names itself repeatedly, a preemption lands on a different place each time. With the render workers on (the default), strips rendered by the render workers are not marked: the marker shows only the strips the audio thread renders itself (inline, or on the callback because they read the live input), and the time the audio thread waits for the workers stays in the phase before it.
- **Render worker figures** (under Callback telemetry, after the over-budget callbacks) appear by default: transit renders mixer strips on several cores (#124). Started with `TRANSIT_RENDER_POOL=0`, it renders every strip on the audio thread alone and the report leaves these rows out, as before. Each is a count since the device opened. Every figure, with what a healthy value is and what to do about it, is defined once in [Callback Telemetry figures](#callback-telemetry-figures) below; the Info panel in the app (Shift+?) shows the same text for the figure under the pointer. Any overrun is an audible gap on that strip; with no overruns and no over-budget callbacks the render kept up.
- **Plug-in host.** One line per call a plug-in host thread is stuck in, naming the plug-in and the call (`Plug-in host: Guitar Rig 7 stuck in a window message (its editor or a dialog it opened) for 240 s`; `(given up ...)` after Give up), or `Every plug-in host thread is responding.` Then how often request servicing waited for a plug-in call to return (normally 0). The log has the same plug-in and call when the call first runs past its budget and every 60 s while it stays stuck ([known issue K90](known-issues/README.md)).
- **MIDI.** The ports transit sees and which are open, the MIDI Learn mappings (the session's and this computer's controller profile), and the port and channel of the last message heard (on a port a track listens to, or on any open port while the MIDI Monitor is on; Clock and Active Sensing do not count). `transit --report` opens no port, so it lists the ports without an open state and no session.

## Callback Telemetry figures

Every figure of the Callback Telemetry window, in plain words. The same text appears in the app: turn on the Info panel (Shift+?) and point at a figure. This section is generated from `docs/help/telemetry.toml`; edit the entries there and run `cargo xtask help --write`.

<!-- help:telemetry -->
- **Callback Telemetry.** How hard the audio thread is working. Every audio callback has a time budget (the buffer length divided by the sample rate), and these figures count how much of it is used and how often it ran out. Healthy is no over-budget callbacks and a load well under 100%.
- **Load readout.** Audio thread time per callback as a share of the callback budget, the highest over the last 1,024 callbacks (about 2.7 s at 128 frames, 11 s at 512), next to the buffer size and sample rate. It turns amber above 80% and red at 100% or after an over-budget callback. Click for the details window; if it stays amber or red, raise the buffer size or freeze heavy tracks.
- **Callback work time.** How many callbacks took each amount of work time, in bins from up to 1 µs, doubling at each step, to over 16 ms. Healthy is nearly all of them in bins far below the buffer length. A growing tail near or past the buffer length is where dropouts come from: find the heavy track or raise the buffer size.
- **Load now.** Audio thread time per callback divided by the callback budget, the highest over the last 1,024 callbacks (about 2.7 s at 128 frames, 11 s at 512). The readout turns amber above 80% and red at 100%, so steady values near those mean dropouts are close. If it climbs, freeze or simplify heavy tracks or raise the buffer size.
- **Over-budget callbacks in that window.** Callbacks among the last 1,024 (about 2.7 s at 128 frames) whose work time was longer than the callback budget. Healthy is 0. Anything else means a dropout may have just happened, so check the load and the longest callback.
- **Peak callback load (since open).** The longest callback's work time as a share of its nominal budget since the device opened, startup included. A single high peak at the start (a plug-in loading) is normal; a peak that keeps coming back is not. Reset peak starts it over.
- **Over-budget callbacks (nominal).** Callbacks since open whose work time was longer than the nominal budget, which is frames divided by the device rate. Healthy is 0, but 0 does not prove glitch-free audio: a callback can miss the hardware deadline without exceeding the nominal budget. If it counts up, raise the buffer size or lighten the session.
- **Over-budget callbacks while a plug-in was loading.** The part of the over-budget callbacks that happened while a plug-in was being created. Loading a plug-in during playback can cost a callback or two; that is expected. If the rest of the over-budget count is higher, loading is not the cause.
- **Nominal budget.** The budget is the number of frames divided by the device rate, not the time left before the hardware deadline. A callback can miss that deadline without exceeding the nominal budget, so zero over-budget callbacks is good but not proof.
- **Render workers.** The worker threads that render mixer strips on other cores next to the audio thread. They are on by default, and the count is fixed when the device opens. Started with TRANSIT_RENDER_POOL=0 the audio thread renders every strip alone and the render worker rows are not shown.
- **Strip deadline.** How much of each block's period the strips have before one that has not finished counts as an overrun, as a percent. The default is 150% for buffers of 1.5 ms or less (up to 72 frames at 48 kHz, 144 at 96 kHz) and 100% for longer ones, and the row reads (default for 64 frames at 48 kHz) with the buffer and rate it was taken from, or (TRANSIT_STRIP_DEADLINE_PERCENT) when that measurement option (25 to 300) is set. Above 100% the audio thread may wait past the period for a slow strip instead of leaving it silent.
- **Strip overruns, busy.** Blocks in which a strip was not rendered because its worker was still busy with an earlier block, so the strip was silent for that block. Healthy is 0. If it counts up, that strip's plug-in is too slow for this buffer size: raise the buffer or freeze the track.
- **Strip overruns, late.** Blocks in which a strip's worker had not finished by the deadline, so the strip was silent for that block and its late render was discarded. Healthy is 0. Look at the strip's cost and worst figures to see which plug-in is slow.
- **Strip overruns, unstarted.** Blocks in which no thread had started a strip by the deadline and the time left could not fit it, so it was silent. Healthy is 0. Many of these mean the workers are all busy or slow to wake: close other programs and check Worker wake.
- **Strip skips, upstream missed.** Blocks in which a strip (or an aux) was skipped because the strip feeding its sidechain key had no result. The cause is counted once, at the strip that overran; a strip that failed, is suspect or was discarded is counted under its own rows, not here. Healthy is 0, and it stays 0 when started with TRANSIT_RENDER_GRAPH=0.
- **Blocks with a strip overrun.** Blocks with at least one busy, late or unstarted strip overrun. Each one is an audible gap on that strip. Healthy is 0; if not, see which of the three overrun counts is rising.
- **Strip deadline overshoots.** Blocks whose wait for the workers ended more than 0.1 ms past the strips' deadline, measured from the start of the device callback. It means the callback itself started late or was held up, which the strip figures do not show. Healthy is 0; if not, close other programs and check power and driver settings.
- **Late strip results.** Renders that finished after their block had already been played and were discarded. Healthy is 0. It rises with Strip overruns, late.
- **Strips rendered inline.** Strips the audio thread rendered itself before the deadline because no worker had started them. A few are normal under load. Many mean the workers wake slowly, so check Worker wake.
- **Inline renders refused, no history.** Strips no worker had started that the audio thread did not render itself, because it has no earlier render to judge them by (a new strip, or a plug-in just loaded, replaced or moved). The audio thread cannot stop a render once begun, so it leaves these to the workers. A few after editing are normal.
- **Inline renders refused, worst case.** Strips the audio thread did not render itself because the strip's slowest recent render would not fit in the time left. They stay with the workers and count as unstarted if none picks them up. A steady count points at a heavy strip.
- **Inline renders refused, recent overrun.** Strips the audio thread did not render itself because the strip had overrun in the last few blocks. They stay with the workers. Healthy is 0; if it counts up, look at the strip rows to find which one keeps overrunning.
- **Strips rendered on the callback.** Strips the audio thread always renders itself because they read the live input or the hardware. Not a problem figure: it only tells you how much work stays on the audio thread.
- **Faulted strip renders.** Renders that failed on a worker, usually a plug-in error. Healthy is 0. If it counts up, note which plug-in is on the strip and report it with Copy diagnostics.
- **Suspect strip blocks.** Blocks in which a strip stayed silent after a fault, until its plug-in is replaced. Healthy is 0. Replace the plug-in on the strip, and it plays again.
- **Deferred MIDI events.** Live MIDI held for a strip whose worker was busy or had not started the block, and delivered at that strip's next block. A few are harmless; many mean the strip is overrunning and the notes arrive late.
- **Deferred MIDI overflows.** Held MIDI events dropped because a strip's buffer was full. Note-ons and ordinary controllers go first; note-offs and pedal releases are kept. Healthy is 0; if not, the strip is badly overloaded.
- **Deferred MIDI silences.** Times a strip's held MIDI overflowed past its releases, so the strip was sent All Sound Off, All Notes Off and sustain off to avoid stuck notes. Healthy is 0.
- **Render worker protocol faults.** Mix-ups between the audio thread and the workers about which block a strip's result belongs to; they would be a bug in transit, not in your session. This should always be 0. If it is not, report it with Copy diagnostics.
- **Strip overruns, top.** The three strips with the most overruns, each with its count, or none when nothing overran. A strip that was only skipped for its sidechain source reads upstream missed. Fix the first strip in the list first.
- **Strip row.** One row for each strip that overran, the most first and at most eight: its busy, late and unstarted overruns, its inline renders, its average render cost and its slowest recent render in µs, which fades over a few hundred renders. Compare cost and worst with the buffer length to see whether the plug-in is simply too heavy. An upstream missed part names the strip the miss came from, and the counts start over when another track moves into that place.
- **Strip lateness.** Late renders by how far past the strips' deadline they finished, counted when the late result is discarded (one per late overrun). Mostly in the first bins means the deadline is missed narrowly, so a larger buffer helps. Mostly in the last bins points at a heavy strip: put fewer heavy plug-ins on it or use a larger buffer.
- **Worker wake.** Worker renders by the time from the audio thread handing out the block's first strip to a worker picking it up. Healthy is mostly the first bins. A long tail means the workers wake slowly: close other programs and check power settings.
- **Callbacks.** Audio callbacks since the device opened. Not a problem figure; it shows the stream is running and gives the other counts a scale.
- **Timed callbacks.** Callbacks whose work time was measured. Healthy is a number close to Callbacks.
- **Callbacks without a timing.** Callbacks for which no work time could be measured. Healthy is 0, or a handful; a large count means the load figures are based on fewer callbacks than ran.
- **Frames.** Audio frames rendered since the device opened. A frame is one sample for every channel; divide by the sample rate for seconds of audio.
- **Total work time (ns).** The audio thread's total work time since open, in nanoseconds. Divided by the real time since open it is the average load, which should be well under 100%.
- **Longest callback (ns).** The longest single callback since open, in nanoseconds. Healthy is well under the buffer length (frames divided by the rate). Reset peak starts it over.
- **Backend errors.** Errors the audio backend reported on the stream since open; the warning icon beside the readout shows the same count. Healthy is 0. If it counts up, read the log, then try Reconnect in Audio Devices.
- **Device-lost errors.** Times the audio device went away, for example unplugged or reset by its driver. These are also counted in Backend errors. Healthy is 0; plug the device back in and press Reconnect.
- **Frames silenced by a render fault.** Frames of audio replaced by silence because rendering failed. Healthy is 0. If not, find the plug-in that faulted in the log and report it.
- **Reset peak.** Starts the since-open peak load and longest callback over. Use it after a change to see whether the peak comes back.
- **Copy diagnostics.** Copies a report of your system, audio devices, these figures and the end of the log to the clipboard. Paste it into a bug report; it holds nothing from your sessions.
<!-- /help:telemetry -->

## Audio Devices figures

Every status line and figure of the Audio Devices window, in plain words. The same text appears in the app's Info panel. This section is generated from `docs/help/audio-devices.toml`; edit the entries there and run `cargo xtask help --write`.

<!-- help:audio-devices -->
- **Audio Devices.** Choose the audio system, devices, sample rate and buffer size, and read what is actually open and how the driver is keeping up. The lines in the card above the settings are live; the settings change only when you press Apply.
- **Output open.** The output device that is running, then its sample rate, channels and buffer, and what the backend reported at open. If it says none, no audio device is open: pick one and press Apply, or press Reconnect.
- **Input open.** The input device that is running, in the same terms as the output. It opens when a track is armed or monitored, so none yet is normal on a fresh start. For ASIO the input is the output driver's own.
- **Device.** The name of the device as the driver reports it. If it is not the interface you meant, choose it in the lists below and press Apply.
- **Sample rate.** The rate the device runs at. It should match the session rate; if it differs, transit converts, which costs a little CPU. A Requested line appears when the device runs at something else than you asked for.
- **Channels.** How many channels of the device are open in this direction. Use the I/O paths section to choose which of them tracks and the main output use.
- **Buffer size.** The device buffer in samples. A smaller buffer means lower latency but less time to render each block; a larger one is safer. Healthy is the smallest size at which the load and the counters below stay quiet while you play.
- **Audio system.** The audio system and mode serving the device. A star (WASAPI* or ASIO*) means transit's own native backend, which reports the counters below; WASAPI also says exclusive or shared. Exclusive and ASIO give the lowest latency.
- **Device sample format.** The sample format the device was opened in, for example i24 for 24-bit integers or f32 for floating point. Nothing to fix: it tells you the converter transit uses between its own float audio and the device.
- **Reported latency.** The latency the driver reports for this direction, in milliseconds. Output plus input is roughly what you hear when monitoring through transit. Lower it with a smaller buffer; the Calibration line shows the measured round trip.
- **Thread priority.** How the audio thread is scheduled: MMCSS Pro Audio is the best case on Windows, MMCSS by driver means the ASIO driver registered its own thread, and raised priority is the fallback. MMCSS refused or pending for long means Windows would not promote the thread, and dropouts are more likely.
- **Requested versus running.** You asked for a buffer size or sample rate and the device runs at another. PipeWire runs a stream at the buffer size the server is set to, and any device can step down to what it offers. Change the value in the lists or in the driver's own settings, then press Reconnect.
- **Backend counters.** The device loop's own figures since the stream opened. Healthy is gap over period and service far below one buffer period, and no skipped wakes, timeouts, faults or driver events. Hover each figure for what it counts.
- **Wakes.** How many times a second the device loop woke to serve the stream. With exclusive WASAPI, ASIO and JACK, healthy is about the sample rate divided by the buffer size (375 a second for 128 samples at 48 kHz), and much lower means the driver is stalling. Shared WASAPI and capture wake on their own schedule, so judge those by the other figures; the rate is left out until two samples exist.
- **Gap over period.** How much longer than one period the time since the previous wake or callback was, never negative: p95 is about the value 95% of wakes stayed under, max the worst since open. Healthy is small next to the period. A driver that delivers in bursts can show up to a period here with nothing late, so only worry when you also hear dropouts.
- **Late (capture).** How late a capture wake was, measured against the end of the first packet waiting. Healthy is under one period. A large max with dropouts in recordings points at the input device or its USB connection.
- **Service time.** How long the loop took per wake to run the audio callback and hand the data to the device, p95 and max in µs. Healthy is well under one period. If max nears the period, the callback is the bottleneck: check Callback Telemetry.
- **Skipped wakes.** WASAPI exclusive wakes whose buffer was not free yet, so the loop skipped them. Healthy is 0. If it counts up while you hear dropouts, raise the buffer size.
- **Timeouts.** Times the device stopped signaling for too long, which ended the stream (for ASIO the watchdog found the driver had stopped calling back). Healthy is 0. Press Reconnect, and if it repeats, check the cable, the driver and USB power saving.
- **Min padding.** The smallest amount of audio, in frames, waiting in the device buffer at a wake in shared mode. Near 0 means the loop is running close to empty. It reads 0 in exclusive mode; raise the buffer if dropouts go with a value of 0.
- **Max packets.** The most capture packets one wake had to drain. A few is normal; a high number means wakes were late and data piled up. Healthy is a small, steady number.
- **Faults.** Failures that ended the stream, such as the device being lost or a failed call. Healthy is 0; single lines show it only when it is not 0, and the ASIO output and input line always shows it. Check the log, then press Reconnect.
- **Overloads.** Overloads the driver reported to transit (ASIO overloads, JACK xruns). Healthy is 0. If it counts up with dropouts, raise the buffer size or reduce the session's load.
- **Busy skips.** Callbacks that found the engine still busy (with the previous callback, or delivering an error) and did nothing, so that buffer kept what it held. Healthy is 0. For ASIO it is the driver calling again before the last buffer switch finished, which usually means a callback ran longer than a period.
- **Position jumps.** Callbacks whose device position did not advance by exactly one buffer. Healthy is 0. It is not proof of an audible glitch, but with dropouts it points at the driver or interface.
- **Resets.** Reset, resync and sample-rate-change requests from the driver. Healthy is 0. Changing the buffer size in the driver's own panel causes one; transit reopens the device by itself.
- **Recovery.** What transit did after the driver asked for a reset or stalled: how it is reopening and the counts of resets, stalls and reopens. Healthy is reconnected or reopened automatically. If it says the device did not reopen, press Reconnect.
- **Calibration.** The measured round-trip latency of this device at this buffer size and rate, so recordings line up with what you heard. Not calibrated means recordings are placed by the driver's reported latency. Measure it with a cable from an output to an input.
- **Apply.** Reopens the devices with the choices in this window. Playback stops first, and it is refused while recording.
- **Revert.** Discards the changes you have made here and goes back to the running choices.
- **Reconnect.** Reopens the saved devices. Use it after plugging a device back in or after a recovery message says the device did not reopen.
- **Rescan Devices.** Lists the audio devices again. Use it after plugging in or switching on an interface that is not in the lists.
- **Driver settings.** Opens the running ASIO driver's own settings panel, where its buffer size and clock are set. Reconnect afterwards.
- **Copy diagnostics.** Copies a report of your system, audio devices, these figures and the end of the log to the clipboard. Paste it into a bug report; it holds nothing from your sessions.
<!-- /help:audio-devices -->

## A plug-in stopped responding

When a plug-in does not return (its editor window turns white and Windows says it is not responding), transit shows a message naming it. Audio keeps playing. Press **Save now** first: the session saves with that plug-in's last stored state. Then **Give up on <plug-in>** stops transit from waiting on it: its window hides when Windows lets it, its plug-ins stay unresponsive until you quit and reopen transit, and plug-ins you add load normally. Do not use End Task on the plug-in's window: it ends transit. Copy diagnostics and the log name the plug-in and what it was doing; attach them to a report for the plug-in's maker or for transit ([K90](known-issues/README.md)).

## A long recording was not placed

Since issue #187 a long take is played from its file instead of from memory, so a take of any length at
the session's sample rate is placed: as one clip, with its waveform shown once its peaks are read (they are
kept beside the file as `<file>.peaks`). A clip played from disk that the disk could not read in time plays
silence for that moment and counts under "Disk underruns" in Callback telemetry. A session saved by an
older build that left takes unplaced places them when it opens, each at the start of a new track named after
the file (nothing already in the session moves), and the session opens modified; move each take where it was
recorded, then save.

What follows still applies to a take that cannot be played from disk (one recorded at a rate other than
the session's). A take longer than about 6.2 hours of mono at 48 kHz (3.1 hours of stereo; half as long at 96 kHz) is too
long for a session to hold in memory. When the recording stops, transit says so in a message that names the
file, how long it is and the limit, writes the same as a warning in the log, and keeps the file where it
recorded: the message ends with "The file is kept at <path>". The session remembers the file: every time
it opens, a note names it, and Copy diagnostics has an "Unplaced takes" section. The recording is
intact (an RF64 WAV past 4 GiB); open it in another editor, or split it and import the pieces. Placing
takes of any length is issue #187 (known issue K91).

## More detail

Three ways, from least to most effort:

1. **Help > Debug Logging.** A checkbox. It raises transit's own messages to debug level for the rest
   of the session, with no restart, and the status line shows the log file. It is not saved: the next
   launch starts normal again. Turn it on, repeat the problem, then attach the log.
2. **`--verbose`.** Start transit with debug detail from the first line:
   `transit --verbose`. Use this for problems at startup, such as a device that does not open.
3. **`--log <directives>` or `RUST_LOG`.** For a single part of the program. The syntax is the usual
   `RUST_LOG` one: a level (`error`, `warn`, `info`, `debug`, `trace`), or `target=level`, separated by
   commas. A name ending in `*` matches every target that starts with it.

   ```
   transit --log "warn,transit_playback=trace"
   RUST_LOG=debug transit
   ```

   Order of precedence: `--log`, then `--verbose`, then `RUST_LOG`, then the built-in setting
   (`warn,soundcraft*=info,transit*=info`). `--verbose` together with `--log` uses the directives of
   `--log` and still raises transit's own messages to debug.

## Watching the log in a terminal

Started from a terminal, transit also prints each log line there as it is written. When it is not
started from a terminal, for example with its output redirected to a file, add `--log-stderr` to
get the same copy:

```
transit --verbose --log-stderr 2> transit-debug.txt
```

On Windows the release build has no console of its own, so a launch from PowerShell shows nothing
unless you redirect standard error as above. The log file always gets every line either way.

## What to paste in a report

- The version (`transit --version`, or Help > About) and the operating system.
- The audio interface and driver, and the line `audio output opened: ...` from the log.
- The log file, or the lines around the problem. Attach the file rather than pasting a long log.
- Steps to repeat it, and whether Help > Debug Logging was on.

The log holds file paths of your sessions and the names of your audio and MIDI devices. Read it
before you post it publicly and remove anything you want to keep private.

Report problems at <https://github.com/transit-audio/transit/issues>.
