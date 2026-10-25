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
   transit --log "warn,soundcraft_playback=trace"
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
