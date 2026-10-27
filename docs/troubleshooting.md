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
- **Longest callback** (under Callback telemetry) says what the longest callback since open was doing, and the line under it, `Longest callbacks`, keeps the longest three. The audio callback writes a phase marker (`phase`: service, MIDI dispatch, strip render, mixdown, aux render, master render, output copy, device I/O; `strip`: the track index; `at`: when the phase last changed) with relaxed stores, no allocation and no lock. When a callback ranks among the longest, the report keeps the phase and strip of the **longest stretch between two phase changes** (`longest_gap`), because the marker is back to idle when the callback ends. The report resolves the strip to the track name and the plug-in display name (the instrument, else the active inserts) from the session, so `Longest callback 19.4 ms during Bass: Serum (strip render); plug-in loading: no; editor open: no (preempted)` reads as: the callback spent its longest stretch rendering the track Bass. `plug-in loading` and `editor open` say a plug-in was being created, or an editor was open, during that callback. A track renamed or removed after the callback reads by its current index (`strip 7` when the index is gone). With the render pool on, strips run in parallel and the marker holds whichever strip entered last. **Classification**, printed only for a callback over its nominal budget, from `longest_gap` against the callback's work time and its budget: **`(slow)`** when `longest_gap` is at most half of the work time (the time was spread over several phases or strips: the work is heavy); **`(stalled in one phase)`** when it is more than half of the work time but within the budget (one heavy call, usually a plug-in's process call, dominated); **`(preempted)`** when it is more than half of the work time and exceeds the whole nominal budget (the thread was not running: a DPC, an interrupt, a higher-priority thread, or a call that blocked). The marker cannot tell a descheduled thread from a single call that did not return, so read the strip and plug-in named beside it: a plug-in that blocks names itself repeatedly, a preemption lands on a different place each time.
- **Render worker figures** (under Callback telemetry, after the over-budget callbacks) appear only when transit was started with `TRANSIT_RENDER_POOL=1`, which renders mixer strips on several cores (a test option; the default renders them on the audio thread alone). Each is a count since the device opened. **Render workers**: the worker threads. **Strip overruns, busy**: blocks in which a strip was not rendered because its worker was still busy with an earlier block (the strip is silent for that block and its own time skips it). **Strip overruns, late**: blocks in which a strip's worker had not finished by the deadline (silent for that block; the render finishes later and is discarded). **Strip overruns, unstarted**: blocks in which no thread had started a strip by the deadline and the time left could not fit it (silent; its time skips the block). **Blocks with a strip overrun**: blocks with at least one of those three. **Strip deadline overshoots**: blocks whose wait for the workers ended more than 0.1 ms past the strips' deadline (measured from the start of the device callback): the callback itself started late or was held up, which the strip figures do not show. **Late strip results**: renders that finished after their block was played, discarded. **Strips rendered inline**: strips the audio thread rendered itself before the deadline because no worker had started them. **Strips rendered on the callback**: strips the audio thread always renders itself (they read the live input or the hardware). **Faulted strip renders**: renders that failed on a worker. **Suspect strip blocks**: blocks a strip stayed silent after a fault, until its plug-in was replaced. **Deferred MIDI events**: live MIDI held for a strip whose worker was busy and delivered at its next block. **Deferred MIDI overflows**: held events dropped because a strip's buffer was full (note-ons and ordinary controllers go first, note-offs and pedal releases are kept). **Deferred MIDI silences**: buffers that overflowed past their releases; the strip got All Sound Off, All Notes Off and sustain off. **Render worker protocol faults**: should always be 0; report it if not. Any overrun is an audible gap on that strip; with no overruns and no over-budget callbacks the render kept up.
- **MIDI.** The ports transit sees and which are open, the MIDI Learn mappings (the session's and this computer's controller profile), and the port and channel of the last message heard (on a port a track listens to, or on any open port while the MIDI Monitor is on; Clock and Active Sensing do not count). `transit --report` opens no port, so it lists the ports without an open state and no session.

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
