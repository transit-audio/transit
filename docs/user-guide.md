# User guide: appearance and interface size

**View > Appearance** lists **System**, **Dark**, and **Light**. v0.7.0 is dark only: Dark is
the graphite look of the device design language, and System and Light are shown unavailable
until a designed light theme follows. The choice is saved per machine in `ui.json`.

**View > Appearance > Interface zoom** shows the current zoom and offers **100%**, **125%**,
**150%**, **175%**, and **200%** presets. Zoom changes every widget and painted label, including
the arrangement, browser, mixer, and editor panels. It is saved per machine and restored at
startup; it does not change your session or create an undo step.

Press **Ctrl+=** or **Ctrl++** to enlarge the interface by a factor of 1.1, **Ctrl+-** to
reduce it by the same factor, and **Ctrl+0** to reset to 100%. On macOS, use Cmd instead
of Ctrl. Zoom is limited to 75% through 300%. Each change appears in the status line.
Plain **+**, **=**, and **-** still zoom the arrangement's timeline.

Interface zoom multiplies your operating system's display scale: 150% interface zoom on a
200% display uses three physical pixels per UI point. At 100%, transit follows the display
scale as before. Higher zoom leaves less space for panels; choose a smaller zoom if needed.
Plug-in editor windows follow the display scale, not this zoom.

See the [shortcuts reference](shortcuts.md) and **Help > Keyboard Shortcuts** for key bindings.

## Lower panes: Piano Roll, Mixer, Devices

The tabs under the arrangement toggle: click the **Mixer** or **Devices** tab to show its pane,
and click it again to hide it. The two panes are independent and can both be open at once, the
Devices pane at the bottom and the Mixer above it; **View > Mixer**, **View > Devices** and their
shortcuts (Ctrl+Alt+M, Ctrl+Alt+L; Cmd+Alt on macOS) toggle them the same way. The tab strip
stays when no pane is shown, so a pane is one click away. With both open in a short window the
Mixer pane gives up height first so the arrangement keeps two track rows, down to the Mixer's
smallest height.

## Arrangement: track height

Drag the bottom edge of a track header to resize its main row from 16 to 440 points.
Hold **Alt** (Option on macOS) when starting the drag to resize every track together,
including hidden tracks and tracks inside collapsed folders. Selection does not change the target.
Returning to the starting height before release restores each original height.

Use **Alt+wheel** over a main track header or clip row to resize just the hovered track,
eight points per notch. Automation and take lanes follow their parent proportionally.
Drag an automation lane's bottom edge to change its proportion; collapsing and reopening
lanes keeps that proportion until the session closes.

In the arrangement, **Alt+=** or **Alt+-** steps every track to the next taller or shorter
preset. **View > Track Height** offers **Increase All**, **Decrease All**, and
**Reset Track Heights**. Reset returns every track to 46 points, keeps lane proportions
and horizontal zoom, and can be undone. Each drag or wheel gesture makes one undo step.

Track heights are saved in the session. Sessions from older builds that used the default
height open at **88 points per track**; **Reset Track Heights** returns them to **46**.

## Arrangement: grid over clips

**View > Grid Over Clips** moves the grid above the clips instead of beneath them. The grid is one
structure, drawn once, carrying the tempo, meter and triplets of the project, so it reads the same
over empty lane space and over clips: the same lines at every zoom, with the clips sitting on it, so
timing edits can be judged by eye. It is off by default, which keeps the grid beneath the clips.
Ableton Live draws its grid through clips while Logic Pro draws it behind regions, so transit offers
both. The setting is saved in the session.

## Piano roll

Click the ruler above the notes to set the playhead; drag it up or down to zoom.
Clicking empty grid clears the selection and marks where step entry begins; it never moves the playhead or where Play starts.

**Scale mode.** The Pitch menu's Scale mode highlights a key and keeps new and dragged notes in it: a dragged note sits on the scale row nearest the pointer, moving to the next one as the pointer passes halfway, and a chord keeps its shape, an accidental keeping its offset above its scale tone. Fold to scale, or the F key, shows only the scale's rows and the rows that hold notes; in the folded view every row is a place a note can go. F replaces the old hold-Shift unfold: Shift only adds to the selection and never changes the rows or the scale. To enter or drag notes outside the scale, turn Scale mode off. The rows never move while a mouse button is held; a change waits for the release.

**Several clips.** Select several MIDI clips in the arrangement, then double-click one of them (or press Ctrl+Alt+K): they open together, up to the first 16 by track order and then clip time. The status line says how many were left out. The one you double-clicked is the foreground clip when admitted, named in the header with the count ("2 clips · Piano"). Each open clip has a bar in its color at the foot of the ruler and its notes in that color. Click a note of another clip, or its bar, to bring that clip to the foreground.
- A band selects notes across clips, and a move, resize, velocity drag, delete, transpose or quantize changes them all in one undo step. Notes stay in their own clips.
- A new note goes to the clip whose span you draw it in, the foreground first. Outside every open clip it is refused, and a status line says so.
- N toggles Focus Mode, or hold N for momentary focus: the other clips' notes dim, and only the foreground clip is edited. Double-click a note to focus its clip without deleting it. With one clip open, double-click still deletes the note.
- Step entry, the controller lane and the clip-end handle work on the foreground clip.

## Note names: Middle C

**Options > Middle C** sets the name MIDI note 60 carries in every transit window: **C3** (the
default, as Logic and Live show it) or **C4** (scientific pitch, as a hardware tuner and most
notation show it). One key has one name everywhere: the piano roll's keys, the MIDI Monitor, the
MIDI Mappings table, the Sampler's note names, the Macro Oscillator's wave readout, the Tuner (its
note and its Reference caption, the A above middle C) and every note-named choice, such as the
Resonator's Tonic. A flip renames them in the next frame, including the MIDI Monitor's earlier
lines, so Copy gives one convention.

The setting is saved per computer and only changes names on screen. Sessions, presets, automation
and MIDI mappings keep MIDI numbers and sound the same. Commands from the assistant, MCP, the CLI or
the control protocol read note names as C3 = 60 whatever the setting, and MusicXML export keeps its
own convention (middle C is C4 there). The Random Sequencer's Octave numbers octaves the same way
as the rest of transit now: its default reads 2 under C3 (it read 3 before) and plays the same
notes.

## Cycle

Cycle (C, the transport button or Transport > Cycle On) only turns looping on and off; it never
moves the cycle region. Turning Cycle on reuses the region you already have, whether you dragged
it on the loop bar or set it earlier. Only when there is no region does Cycle On make one: from
the span of the selected clips (earliest start to latest end), else the selected time range, else
four bars from the bar holding the playhead, in one undo step with the switch. Cycle Off keeps
the region. To move the region to the current selection, use Transport > Set Cycle to Selection
(Ctrl+L, Cmd+L on macOS), which does not toggle Cycle.

## Transport from MIDI devices

Connected MIDI inputs honor MMC Play, Stop, Record, Pause, Fast Forward, Rewind and Locate,
plus MIDI Start, Stop, Continue and Song Position, without a mapping or an armed track.
Start and Play start at the edit cursor; Continue resumes the retained position. Song Position
and Locate move without starting. Clock messages do not change tempo.

In **Audio > Audio Devices > MIDI**, **Transport buttons** turns this off for one device.
**MMC device ID** selects an addressed ID from 0 to 126; broadcast 127 is always accepted.
Record still needs an armed track and uses the ordinary count-in and pre-roll.
**MMC Record Exit ends recording and stops playback; it does not punch out and continue playing.**

For a controller that sends notes or CCs, choose **Learn Play**, **Learn Stop**, or another
transport target in that MIDI section, then press the controller button. You can also right-click
a transport button and choose **MIDI Learn for this controller (all sessions)**. Release before
pressing again: a held button fires once. Transport mappings take priority over overlapping
session mappings, which remain stored and are shown as overridden.

The switch, ID and learned assignments are saved by exact device name in this machine's
`ui.json`, under `midi_transport`. They carry across sessions and reconnects; they are never
saved in a session. Devices with the same name share these preferences.

## Notes played during the count-in

Notes you play on an armed MIDI or instrument track during the count-in are kept, as in Logic Pro:
the clip starts at the first of them instead of at the record start, and every note stays where
you played it. Recording already starts at the pre-roll start, so notes in the pre-roll are kept
too. Nothing lands before the start of the session, and with Cycle on the count-in notes belong to
the first take. Count-in notes need room before the record start: recording from the very start
of the session, a note you released during the count-in is left out of the take (the status line
says how many), and a note still held when recording starts begins at the session start.

## Takes and comping

With Cycle on, Record keeps a take per lap until Stop; recording over clips keeps the old clips as
the previous take. The takes of one stretch of time form a stack, and the track plays one of them:
the newest when you stop. Takes work the same for audio and MIDI.

- **Take lanes.** After cycle recording the track's take lanes open under it: one lane per take,
  the newest on top, labeled **Take 1**, **Take 2** and so on. A take's clips are highlighted
  where it plays and dimmed elsewhere. Click **T** in the track header (or **Show Take Lanes** in
  the track's right-click menu) to show or hide the lanes; a clip with stacked takes also carries
  an "n takes" chip that does the same. Showing lanes only changes the view.
  Lane headers show the span each take plays in bars.beats (hover for the full list of multiple ranges), and arrangement clip labels include the take number without changing the stored clip name.
- **Choose a take.** Click a dimmed clip in a lane: that take plays over the clip's range, and the
  rest of the track keeps playing. Click a lane's label to play that take wherever it was
  recorded. A shorter take plays over its own range, with the longer take around it; choose the
  longer take back and it is one clip again, with its notes and fades whole. Each choice is one
  undo step.
- **Flatten Takes** (the lane menu at the label's right end, the track menu, or the clip menu for
  that stack) keeps what plays and removes the other takes. One undo brings them back.
- **Delete Take** (the lane menu) removes a take that is not playing, after asking: the
  question's **Delete Take** button, or Enter, confirms.
- A frozen track's takes cannot be changed: unfreeze it first.

Swipe comping (picking a range from one take and the next range from another, with crossfades),
renaming or coloring takes and auditioning a lane are not in this version. Logic's Take Folder and
Live's Take Lanes work the same way for everything above.

## DAWproject interchange

**File > Open** opens a `.dawproject` as a new, untitled session with its tracks, mixer settings,
note clips, MIDI expression, tempo and meter, and markers. Anything the importer does not support is
counted in the load report.

**File > Export DAWproject…** writes those editable parts to a new `.dawproject`. Audible audio and
instrument tracks also include full-length 32-bit float WAV stems, so built-in and hosted device
processing survives even though the devices themselves, routing, groups and parameter automation are
not exported separately. Track and master volume/pan automation is omitted in favor of the static
mixer values. Muted and empty tracks have no stem; inactive tracks are omitted. External Instrument
audio cannot render offline; those tracks keep their MIDI and are named in the result. The confirmation
dialog states these limits before choosing a location.

## Bounce

File > Bounce… (Ctrl+Alt+B) writes the main mix or a bus over the selection, the cycle range or
the whole song, or one file per output path (**Stems by output path**). **Format** is WAV, AIFF or
FLAC, lossless at the bit depth you pick (16-bit, 24-bit or 32-bit float), or **Opus**, transit's
lossy format for sharing: small files that play in every browser, phone and messaging app.

With Opus the bit depth row gives way to **Bitrate**: 96, 128, 160 (the default), 192 or 256 kb/s
for stereo; a mono bus offers 96 (the default), 128 or 160 kb/s.
The encoder is the reference libopus at its highest quality setting, with a variable bitrate that
stays near the rate you pick, so a three-minute song at 160 kb/s is about 3.6 MB. Opus files are
always 48 kHz: a session at another rate is converted as the file is written, and the file is
exactly as long as the range you bounced. Opus is mono or stereo; a surround mix or path is refused
(fold it down to stereo, or bounce WAV or FLAC). The destination's extension follows the format
(`mix.wav` becomes `mix.opus`). transit does not export MP3: Opus sounds better at every bitrate,
and its encoder is BSD-licensed where MP3's is LGPL. Importing MP3 still works, and `.opus` files
import too.

The control channel's `file.bounce_mix`, `file.bounce_stems`, `file.bounce_paths` and
`file.export_clips` take `format: "opus"` and `bitrate` in kb/s (6 to 510; the dialog offers the
five above); a `path` ending in `.opus` picks Opus on its own.

## Bounce in Place

Choose **Track > Bounce in Place** (Commit, **Alt+Shift+C**) to print the focused track through
its inserts, before its fader. The print starts at the session start and ends two seconds after
the last clip on the active playlist. The source's clips and take stacks are always preserved.
Every print (Bounce in Place, Commit, Freeze and File > Bounce) starts once every insert has already
been running for a few seconds, as live play from a stop does, so a plug-in's start-up does not
land in a print.

- **Destination:** **New track** creates **<name> bounce** directly under the source and copies its
  volume, output and sends. **Existing track** adds the print to the active playlist of a visible,
  unfrozen audio track other than the source. It overwrites material inside the printed range,
  preserving material outside it, just like pasting over clips in Slip mode.
- **Source:** **Leave** keeps it unchanged. **Mute** (the default) mutes it only: it stays active
  and its plug-ins keep running; Mute means mute, as in Logic's Bounce in Place.
  **Mute and deactivate** mutes it and deactivates it, stopping its plug-ins while keeping it
  visible. **Hide** deactivates and hides it. To play a deactivated source again, choose
  **Track > Make Active**, **Make Active** in the track's header menu, or click the **inactive**
  tag on its header or mixer strip, and unmute it if needed. Inactive tracks have dimmed names and
  the tag in both views; in the arrangement their color bar turns gray, their clips become gray
  outlines, and the track is striped across its header and lane. A source that is already inactive is refused with "<track> is inactive:
  Track > Make Active first", and nothing changes.

**Enter** bounces; **Esc** cancels. Choices are remembered for the current session. The destination
is focused after the bounce; one Undo restores the whole operation.

**Track > Show Hidden Tracks…** lists hidden tracks with **Show** beside each and **Show All**.
Showing restores visibility, including access to the takes, without changing mute or inactive
settings. The entry is disabled when no tracks are hidden.

To print only some clips, select them and choose **Bounce in Place** in the clip's right-click menu
or the **Edit** menu (Logic's Bounce Regions in Place). The form's second line gives the span, for
example "2 clips on 1 track, 1.1.1 to 9.1.1". On each track the print runs from the earliest
selected clip's start to the latest one's end, plus a two-second tail, and starts at the span's
start; clips outside the span are untouched. With clips on several tracks the print is one new
track per source track, placed under each source, so the existing-track choice is unavailable, and
one Undo restores them all. The source choice applies to the whole source track, not just the
span: Mute mutes the track, as in Logic. The Edit item is disabled with no clip selected.

The control channel offers `track.bounce` (alias `track.commit`) with
`{tracks?, destination?: "new" | <track id>, source?: "leave" | "mute" | "mute_deactivate" | "hide", range?: {start, length}}`;
defaults are `"new"`, `"mute"` and the whole track. `range` (session samples) prints only that span, placed at
`start`. `clip.bounce {clips?, destination?, source?}` prints the selected clips' spans as described above. `track.show_hidden {tracks?}` shows the given tracks, or all hidden tracks
when omitted. `track.hide {tracks, hidden: false}` also shows them.

## Audio setup

### Audio devices on Windows

On Windows the audio system decides how transit talks to the device: **WASAPI** opens through
transit's native WASAPI backend and **ASIO** through its native ASIO backend. There is no Backend
choice in **Audio > Audio Devices**. On WASAPI, **Take exclusive control of the device** opens the
device exclusively (other apps go silent while transit runs); shared is the default.

The ASIO list comes from the registry: transit lists every installed driver by name without loading
any of them. A driver is loaded only when transit opens it (on **Apply**, **Reconnect** or at startup
with ASIO saved), so its channel counts, rates and buffer sizes show once it is open; until then the
lists offer the standard choices. Loading a driver another app is using can take the interface away
from that app, so transit never does it just to list. With `--no-audio` no audio device is listed or opened
at all, so a second copy started that way never touches the driver the first one holds (a second
copy with audio on opens its saved device like the first). When transit quits it releases the driver
it holds before the process ends.

If the device does not open, transit says why and leaves it closed, with no other backend standing
in: the status line and Audio Devices read "The audio device did not open: <reason>." Fix the cause
(close the driver's settings panel, plug the interface in, free it from another app), then change
a choice and press **Apply**, or press **Reconnect**. If a saved output or input is not connected,
the system's default opens instead and transit says so at startup and on Apply, for example
"Output “X” is not available; using “Y”."

### Low Latency mode

Turn on **Low Latency** in the transport bar or **Options** when playing an instrument or monitoring an input through a long effect chain. It temporarily bypasses eligible software audio inserts on armed or monitored tracks, their downstream buses, and the master. The master is included even when no track is armed. Inserts with the largest reported latency are bypassed first; equal latencies use stable track and insert identities.

Set **Low Latency limit** in **Audio > Audio Devices > System**. The limit is the cumulative added processing latency of each live route, in milliseconds; 0 ms removes all eligible positive-latency inserts. Instruments, MIDI effects, hardware inserts, and user bypass settings are preserved. Unrelated playback tracks stay compensated and can still determine the shared latency, so the limit is not a guarantee of total playing delay.

The button is lit while enabled and says pending until the prepared plan applies. Dimmed insert chips and device cards show **bypassed by Low Latency** only after adoption. The status line reports the number of bypasses and the reduction in shared latency once per toggle. Manual bypass remains in effect when the mode is turned off, and toggling does not restart recording or change recorded input samples or placement.

The button tooltip and diagnostics show **Path latency**, the adopted shared/Main engine latency plus the device's reported output latency. This is not a physical round-trip measurement or a latency promise for an unpadded cue. Unknown device output latency is shown as unknown; with Delay Compensation off the figure is unavailable because paths are not aligned. Copy diagnostics includes the same readout.

The mode and limit are saved per machine in ui.json, never in the session or undo history. Offline bounce, freeze, commit, stems and Bounce in Place use the full user-enabled chain. A real-time render through the open Player temporarily requests the full chain and restores the machine preference afterward.

In **Audio > Audio Devices > I/O paths**, enable **Live (no shared padding)** on a named output other than Main to hear a cue as soon as its own sources align, without waiting for slower outputs.
The checkbox is saved on this machine, leaves inserts active, and is ignored by offline renders.

## Sampling what the computer plays

To sample a browser video or another app, record it as you would a microphone. In
**Audio > Audio Devices**, open **I/O paths**, tick **Record what the computer plays** under
**System Audio**, choose the output to capture (the system default unless you pick one) and press
**Apply**. Then set an audio track's input to **System Audio**, arm it and record while the sound
plays. Dragging the recording into a sampler comes in a later release; until then the take is a
file in the project's Audio Files folder.

On Windows transit captures the output itself (WASAPI loopback), whatever driver the session uses,
at that output's own rate, converted to the session's. The capture follows that output's clock, not
your interface's: Audio Devices shows the latency it adds and the small drift transit measures and
corrects. A 5.1 or 7.1 output is folded to its front pair (the center channel, the dialogue, is
dropped, so a stereo output is the better pick), and the clip is not lined up with the rest of the
session, so move it into place by ear. If transit (or another app) has that output open in
exclusive mode, Windows refuses the capture and Audio Devices says so; pick another output or open
the device shared. The input itself opens with the track's input, so System Audio needs an input
device to ride on. On Linux the PipeWire or PulseAudio monitor of an output arrives as an input
already and is listed as **System Audio (monitor of ...)** where transit can tell; if none is
listed, route the monitor to transit's input in pavucontrol's Recording tab or in qpwgraph. macOS
needs a virtual audio device for now.

System Audio records what the computer plays, as a loopback cable would. You are responsible for
the rights to what you record.

## Plug-in windows

On Windows, plug-in editor windows stay with transit's window: minimizing transit hides them and
restoring it brings them back where they were; returning to transit (Alt+Tab, its taskbar button
or a click on its window) brings them in front of it. They have no taskbar buttons of their own and
are not listed in Alt+Tab. Clicking a plug-in that already has its window open (its mixer insert,
its device card's title, or Open Instrument Editor) brings that window to the front instead of
opening a second one. To keep each plug-in window apart, with its own taskbar button, turn off
**Window > Plug-in Windows Follow transit**; the choice is saved per machine. A plug-in that stops
responding does not hold transit up: its window simply stops following until it answers again.
Windows a plug-in opens by itself (its own preset browser or a licensing dialog) are not grouped.
Returning to transit by clicking a plug-in window brings up only that window; the main window and
the other plug-in windows come up on your next click on the main window.

On Linux, VST3 and CLAP editors open in an X11 window of transit's, from the track header, the
device card and the mixer slot, as on Windows. On a Wayland desktop they open through XWayland,
the X11 compatibility layer GNOME and KDE start by themselves: the plug-in sees X11 either way, so
nothing needs setting up. Windows plug-ins run through [yabridge](https://github.com/robbert-vdh/yabridge)
open their editors the same way (yabridge embeds the Windows editor in that X11 window). The
editor window takes the size the plug-in asks for and follows it when the plug-in resizes itself;
its scale is the desktop's (`Xft.dpi`, which GNOME and KDE set from their scaling setting). With
**Window > Plug-in Windows Follow transit** on, minimizing transit hides the editors and restoring
it brings them back, and they have no entry of their own in the taskbar. What does not work yet on
Linux: the editor window has no header strip (presets, Save, Bypass and MIDI Learn stay in the
device card and the mixer), keyboard focus can stay with the main window until you click into the
plug-in, and an editor keeps one scale on every monitor ([known issues K92 to K94](known-issues/README.md)).

## FX channels and sends

+ FX Channel (the mixer button, Track > Add FX Channel, or the arrangement quick-add) adds an FX channel and
its bus and nothing else: no track gets a send, whatever is selected. Sends come from a strip's
**+ Send** slot. Its **New FX Channel...** entry creates the channel and a send to it from that
strip at -12 dB, in one undo step, the way Logic and Live do it. Escape in the arrangement, or a
click on empty space below the tracks or in the mixer, deselects the clips and tracks: no mixer
strip or track header is highlighted. The edit cursor keeps its track, which stays the target of
cursor commands (Paste, Split, New MIDI Clip) and of the Devices pane until you click a track
again. Clicking a track header selects the track and clears the clip selection; the last click
decides what Delete removes. Deleting a group, several tracks or a track that holds clips asks
first, and the button says what goes (**Delete Track**, **Delete 3 Tracks**, **Delete Group**).
In every such question Enter presses the highlighted button and Escape cancels: in the
save-changes question Enter is **Save**.

## Main bus monitoring

The main bus strip (the rightmost strip in the mixer) has a monitoring set beside its M: **Mono**,
**Dim** and **Ref**. They change only what you hear from your speakers or headphones, after the
master fader. A bounce, a freeze, a stem, a send and the meters never have them: a bounce made
with Mono or Dim on is identical to one made with them off. They are settings of this computer,
kept with the audio settings, not part of the session, and never an undo step.

- **Mono** sums the left and right sides to mono, each at -3 dB, to check how the mix holds up on
  one speaker.
- **Dim** turns what you hear down 20 dB until you press it again.
- **Ref** plays at your stored reference listening level while it is on. **Alt+click** on Ref
  stores the current monitor level as the reference. The monitor level itself (0 dB, the default,
  down to silence) is set with the `mix.monitor` command for now; the master fader stays the mix
  level.

The line under the main bus's readouts, **INT**, is the integrated loudness of the mix in LUFS
(ITU-R BS.1770-4, gated, as streaming services measure it), running since the last reset. It
measures the mix after the master fader, not what Dim or Ref make of it. Click it to reset it;
starting the transport from a stop resets it too. A dash means nothing loud enough has played
yet. Under the strip's name, the main bus shows the output path it plays on and the audio device
that path feeds ("Out 1-2 · Speakers"; the path alone while no device is open).

## A plug-in that stops responding

If a plug-in stops responding (its editor turns white), transit names it in a message: audio keeps playing, **Save now** saves the session with the plug-in's last stored state, and **Give up on <plug-in>** lets transit carry on without it until you restart; End Task on the plug-in's window would end transit ([troubleshooting](troubleshooting.md#a-plug-in-stopped-responding)).

## Multicore mixer render

transit renders mixer strips on several CPU cores (one render worker per core, minus one for the
audio thread, at most eight). Each block gives the strips a deadline: a strip that misses it is
silent for that block and is counted under **Render worker figures** in Copy diagnostics
([troubleshooting](troubleshooting.md)). Sessions with sidechain keys render on the workers too,
through a dependency graph: a keyed strip starts once its key source has rendered. Strips that read
a live input or hardware (monitored audio tracks, External Instruments, hardware inserts) render on
the audio thread as before.

To render every strip on the audio thread instead, start transit with the environment variable
`TRANSIT_RENDER_POOL=0` (for example `set TRANSIT_RENDER_POOL=0` in a Windows command prompt
before `transit`, or `TRANSIT_RENDER_POOL=0 transit` on Linux and macOS). Do this when a plug-in
misbehaves on a worker thread: its track drops out, or keeps going silent while Strip overruns
(busy) keeps rising, as when a plug-in hangs ([known issue K89](known-issues/README.md)). Unset,
or `1`, uses the workers. An app opened from the Start menu, a desktop shortcut or Finder does not
see a shell variable, so set `TRANSIT_RENDER_POOL` as a user environment variable instead (Windows:
System Properties > Environment Variables; macOS: a launchd user environment or a shell launcher)
and restart transit. If only sessions with sidechain keys misbehave, set `TRANSIT_RENDER_GRAPH=0` the
same way instead: those sessions then render on the audio thread, and sessions without keys keep the
workers.

Set `TRANSIT_RENDER_TAIL=1` the same way to opt into rendering the aux and bus tail through the
graph (#174 slice 2). Unset or `0` keeps the serial tail, the default until the paired measurement;
other values are logged once and treated as unset. `TRANSIT_RENDER_GRAPH=0` disables the tail graph
even with this opt-in. Masters still render on the audio thread after the tail.

The tail graph requires a render pool, an audio-thread block without forced serial rendering,
both graph switches on, a worker slot for every task strip, and at least four phase-1 strips plus
live auxes combined. Live auxes count toward that threshold. Only an away tail strip forces the
tail recovery path regardless of the switches or counts; away phase-1 strips alone use the
existing recovery path.

## Macro Oscillator

The Macro Oscillator is a built-in instrument (choose it from a track's instrument slot): up to 16
voices, or one mono voice, with 24 synthesis models, from virtual analog, waveshaping and FM through grain, additive,
wavetable, chords and speech to swarms, noise, string and modal physical models and three drums.
It is a port of Émilie Gillet's open-source macro oscillator firmware (MIT license), by way of
Oliver Rockstedt's Rust port (MIT): the digital synthesis is the original's, checked sample by
sample against her C++ code, but it is a port, not an emulation of the hardware module or its
analog stages. Their license notices are in **Help > About transit**.

- **Model** picks the synthesis model: the picker under the display lists all 24, numbered,
  Pitched first and Noise · Perc after, each button printing a short name (hover it for the full
  name); click one, then the arrow keys step through them (left and right one, up and down a row).
  The model card above the display shows the model's name and a one-line description.
  **Harmonics**, **Timbre** and **Morph** are the model's three tone controls (the large knobs under
  Tone); what each does depends on the model, and the knob labels, like the pad's axis names, change
  with it (the table below lists them; hover a control with the Info panel on, Shift+?, for a
  summary). The square pad beside the display sets Timbre (across)
  and Morph (up) together: drag it, hold Shift to move finely, double-click to return both to the
  middle. A route to Timbre or Morph shows on the pad as a dashed amber trail.
- **Playing**: a note opens the voice's low-pass gate at the note's velocity; releasing it closes
  the gate over **Decay**, and **LPG Color** sets how it closes (filtering as it fades at 0%, a plain
  fade and a longer release at 100%). The drum models ring out by themselves over **Decay**; the Chiptune model holds while its key is
  held and fades over **Decay** after it.
- **Voices** sets how many notes sound at once, 1 to 16 (8 by default); the lights under it show
  each voice (lit: a key holds it; half lit: its release is fading; dark: free), and the display
  above counts the voices sounding. A new note takes a
  free voice, else the quietest voice whose release is still fading, else the oldest held note, so
  the newest note always sounds. A released note fades over **Decay** on its own voice while you
  play on. Each voice has its own noise, so a chord on a noise model is not one noise in unison, and
  an export plays the same every time. Voices add, so wide chords are louder than one note: lower
  **Level** if they clip.
- **Mono, last note, legato** (**Voices** at 1): one note sounds at a time. A note played while another is held takes
  over without restarting the sound (legato); releasing it returns to the note still held. Notes
  played under the sustain pedal are legato too. A legato note takes its own velocity, so the level
  steps mid-phrase, as the original's level input does (Ableton's and Logic's legato modes keep the
  running envelope instead).
- **Pitch bend** spans **Bend Range** semitones (2 by default), **Tune** transposes, and the **mod
  wheel** adds up to 100% to the control **Mod Wheel** names (Timbre by default; Harmonics, Morph or
  Off).
- **Aux Mix** crossfades to the model's auxiliary output, a variation of the sound. **Level** is
  the output level; at 0 dB the instrument peaks at full scale (one LSB, 0.0003 dB, under 0 dBFS),
  and the -6 dB default leaves headroom on the track.
- **Tuning**: the port plays at concert pitch, the A above middle C (MIDI 69) at 440 Hz. The original firmware's arithmetic is
  kept bit-exact in transit's code for its tests; the module itself plays in tune because its audio
  clock runs at the rate that arithmetic assumes.
- The voices run at 44.1, 48, 88.2 or 96 kHz. At other project rates they run at the nearest of
  these, still in tune, with their envelopes slightly faster or slower. Changing the model while a note
  sounds can click, as on the original.

### The 24 models

The models in the order of the picker (1 to 16 pitched, 17 to 24 noise and percussion). The last
three columns are what the Tone knobs are called while the model is selected.

| # | Model | Picker | What it is | Harmonics | Timbre | Morph |
|---|---|---|---|---|---|---|
| 1 | Filtered analog | Filtered | Analog waves through a resonant low-pass filter | Resonance | Cutoff | Wave and sub |
| 2 | Phase distortion | Phase | A sine bent by an asymmetric triangle modulator | Bend freq | Bend amount | Skew |
| 3 | Six-op FM 1 | 6-op 1 | Six-operator FM, first bank of 32 patches | Patch | Mod level | Env stretch |
| 4 | Six-op FM 2 | 6-op 2 | Six-operator FM, second bank of 32 patches | Patch | Mod level | Env stretch |
| 5 | Six-op FM 3 | 6-op 3 | Six-operator FM, third bank of 32 patches | Patch | Mod level | Env stretch |
| 6 | Wave terrain | Terrain | A 2D surface read along an elliptical path | Terrain | Path radius | Path offset |
| 7 | String machine | Strings | Divide-down string-machine chords with filter and chorus | Chord | Filter, chorus | Waveform |
| 8 | Chiptune | Chiptune | Square-wave chords with an arpeggiator and a triangle voice | Chord | Arp, inversion | Pulse or sync |
| 9 | Virtual analog | Analog | Two classic waves with sync and crossfade | Detune | Square shape | Saw shape |
| 10 | Waveshaper | Shaper | A triangle through a waveshaper and a wavefolder | Shaper wave | Fold amount | Asymmetry |
| 11 | FM | FM | Two sine operators modulating each other's phase | Ratio | FM index | Feedback |
| 12 | Grain | Grain | A formant oscillator built from windowed sine segments | Formant ratio | Formant pitch | Formant width |
| 13 | Additive | Additive | Sine harmonics shaped into a movable spectral bump | Bump count | Peak harmonic | Bump width |
| 14 | Wavetable | Wavetbl | Four banks of 8 by 8 waveforms, scanned by row and column | Bank | Row | Column |
| 15 | Chord | Chord | Four-note chords from analog or wavetable oscillators | Chord type | Inversion | Waveform |
| 16 | Speech | Speech | Vowel and word synthesis in several speech styles | Speech style | Formant shift | Phoneme, word |
| 17 | Swarm | Swarm | Eight enveloped saw grains in a drifting cloud | Pitch scatter | Density | Grain length |
| 18 | Filtered noise | Noise | Clocked noise through a resonant filter | Filter type | Clock rate | Resonance |
| 19 | Particle noise | Particle | Random dust clicks through filter networks | Pitch scatter | Density | Filter type |
| 20 | Inharmonic string | Pluck | A struck or plucked string, from tuned to inharmonic | Material | Brightness | Decay |
| 21 | Modal | Modal | A modal resonator struck by noise or dust | Material | Brightness | Decay |
| 22 | Bass drum | Kick | Analog-style kick: a punchy attack and a tuned body | Attack, drive | Brightness | Decay |
| 23 | Snare | Snare | Analog-style snare: shell modes plus filtered noise | Noise balance | Mode balance | Decay |
| 24 | Hi-hat | Hi-hat | Metallic square oscillators with a noise blend | Metal or noise | High-pass | Decay |

The three Six-op FM models are the same engine with a different bank of 32 patches; Harmonics picks
the patch, Timbre sets how hard the modulators drive, and Morph stretches the envelopes. Models 17
to 24 are noise and percussion: Material (models 20 and 21) runs from a tuned sound toward an
inharmonic one, and on the drums Morph is always the decay.

### Shaper and modulation matrix

Below the module's own controls, two section bars, **Shaper** and **Modulation**, carry the
TRANSIT EXTENSION tag: they are not part of the original module, and both are off by default (with
the shaper off and every matrix row Off, the instrument plays exactly as the module does). Click a
bar's title to show or hide its section (the choice is remembered per machine); until you choose,
a section shows while it is in use. The switch at the right of the Shaper bar turns the shaper on
and off, and a disabled Shaper that is shown dims and stays editable.

- **Shaper** switches on a section after each voice's low-pass gate, per voice: an **amp envelope**
  (Attack, Decay, Sustain, Release, and **Velocity**: how much velocity sets the level) that becomes
  the voice's envelope (the gate stays open and the note ends when the amp envelope's release
  does), and a **filter** (low, band or high pass; 12 or 24 dB; **Cutoff**, **Resonance**, **Drive**,
  **Key Track**) with its own envelope (**Env Amount**, up to six octaves, and its ADSR). The
  filter is the Filter effect's own, one per voice, so a chord's notes each get their own sweep.
  Turning the shaper on or off applies to held notes at once, with a short crossfade and no new
  attack (on, a held note goes through the filter at its envelopes' current level); notes already
  released finish the way they started, so the switch does not click.
- **LFO 1** and **LFO 2**, per voice: Sine, Triangle, Saw, Square, Sample & hold or Smooth random;
  a **Rate** or, with **Sync**, a **Division** of the tempo (the divisions run in duration order, with triplets and dotted values where they fall); **Retrigger** (on: each new note starts
  the cycle; off: notes pick up the instrument's running LFO, so held notes move together);
  **Fade In** after each note; **Amount**. A synced LFO with Retrigger off follows the song position
  while the transport plays, so it lines up with the bars wherever playback starts. The envelopes
  and LFOs run whether or not the shaper is on: they reach the sound through the matrix. Known
  limit: the song position sets a synced LFO's phase, but Sample & hold and Smooth random values
  depend on the cycles before, so playback from bar 5 and an export from bar 1 can differ in those
  two shapes (the other four match).
- The **matrix** has 16 rows; the Modulation section shows the rows in use, one under another,
  and **Add route** shows the next free one. Drag a source chip (LFO 1, Velocity and the others)
  onto a knob to route it there in the first free row, then drag the row's amount bar; a knob with
  a route draws an amber ring from its value to its reach, and the x at a row's end clears it. Each
  row has a **Source**, a **Destination**, a **Via**, a **Curve** and an **Amount** (-100% to +100%): every 12 frames, for every voice,
  it adds amount × curve(source) × via to its destination. Sources: LFO 1 and 2, the amp and filter
  envelopes, velocity, the key (centered on middle C, MIDI 60), release velocity, aftertouch, poly
  aftertouch, the mod wheel, pitch bend, a random value per note, and the rack's eight macros when
  the track is in a rack. Destinations, at 100% and a full source: Pitch (24 semitones), Harmonics,
  Timbre, Morph and Decay (their whole travel), Level (silence to double), Cutoff (six octaves),
  Resonance, Filter env amount, LFO 1 and 2 rate (four octaves; a synced LFO follows the tempo),
  LFO 1 and 2 amount, and Pan (left to right). The curve (Linear, Exponential, S-curve, Equal power,
  Logarithmic: the clip fades' shapes) bends the source the same way on both sides of zero. A via
  scales the amount by a second source: an LFO 1 to Pitch row with the mod wheel as its via is
  vibrato that the wheel brings in.
- Sessions and presets saved before the matrix had a **Note to Macro** grid (velocity and key
  depths with a curve each); they open with those depths as the first matrix rows (Velocity or Key
  to Harmonics, Timbre, Morph or Decay) and sound exactly as before. The **Key Tracked Timbre**
  preset is one such row: Key to Timbre at +50%. A MIDI controller mapped to an old curve picks the
  same curves as before but now drives only the first row of its source; the load note says so.

The **Filter** insert is paraphonic: one filter after the voices are summed, with one envelope for
all held notes, after any instrument, built-in or hosted. This section is per voice and belongs to
the Macro Oscillator only: every note gets its own filter, envelopes and LFOs.

Ableton's Wavetable and Logic's ES2 and Alchemy route modulation through a matrix too; the via
column follows ES2's modulation router (Source, Via, Target), and Alchemy's modulation has one too.

Every control can be automated (lanes `plugin:instrument:<id>`, for example
`plugin:instrument:timbre`, or `plugin:instrument:mod1_amount` for the first matrix row's amount)
and mapped to a MIDI controller (right-click it). Presets: Init, Analog Bass (mono), Glass Bell,
Plucked String, Key Tracked Timbre, and your own: the title bar's arrows step through them, the
name field lists them (with Save As and Delete), and the save button writes over your own preset
(on a factory preset it opens the list to Save As). The editor is a floating window inside
transit's main window, so it minimizes and returns with it. Drag its title bar to move it, and double-click an empty part of the bar to fold the window down to the bar and back (Ableton Live folds a device the same way; the bar's controls keep working, and **Window > Collapse All Editors** and **Expand All Editors** do every window at once). Device windows size themselves to their content, with no resize handle, and re-fit when a section opens or closes; one that would run off the bottom of the screen moves up, and past that its body scrolls. It opens where you last left it (a second Macro Oscillator's window opens
beside the first). The body scrolls with the mouse wheel; to nudge a knob with the wheel, hold Ctrl
(Cmd on macOS). **Escape** closes the editor you last clicked in, and **Window > Close All
Editors** closes every editor, built-in and hosted. The link button in the title bar
(**Follow Selection**) makes the window show the Macro Oscillator of whichever track you focus,
as Logic's Link does; off, the window stays on its own instrument.

To give a Sampler pad its sample, drag a sample from the Files browser or from your file manager
onto a pad: the file dropped on a pad becomes that pad's sample, in one undo step. While you drag
from the browser, the pad under the pointer
is outlined. On the Slices view's empty bench or the Instrument view's empty key map, a file dragged
from the browser does what that view's Load… does (the slice loop, or a zone in the first free
slot), in one undo step, with the dashed well outlined while you hold it there; once the view has
content it takes no drag. A file or plug-in dropped elsewhere on a device window is ignored; the window takes the
drop, so nothing behind it is reached. Anywhere else, a browser drag lands on the track lane,
header, mixer strip or device chain where you let go, and a file from the file manager is imported
into the arrangement at the cursor (a `.transit` or `.dawproject` file opens as the session, a MIDI
file imports as MIDI). On Windows a file manager drop lands where the pointer is when you let go; on other
platforms it lands as soon as the pointer moves after you let go (see the known issues).

Right-click a pad and choose **Clear pad** to empty it and reset its name, choke group, mute,
and solo. Delete or Backspace does the same for the selected pad while the Sampler window has
the keyboard and shows its pads; while a menu is open, the keys do nothing. Its unused zone is removed with its level, pan, envelope, modulation and shaper settings, so the next sample loaded there starts from the defaults; audio files and other pads stay intact. One Undo
restores the whole edit. The trigger note stays as it was. **Reset note** appears in the same
menu only when the pad stores its own note; it returns that pad to the kit's start note plus
its position. A refused reset shows the engine's reason under the pads and changes nothing.

Newly imported Sampler samples keep their native sample rate and PCM. Playback converts the
rate and pitch together; start, end, slice, cue and loop positions count the source's own frames.
A 44.1 kHz sample therefore keeps 44,100 frames per second in a 48 kHz session. Envelope times
still follow the session rate. Older kits are unchanged: audio converted by an earlier alpha
build keeps that stored PCM and rate and plays as before. There is no migration or restoration
of the original unconverted audio; reimport a file to use its native PCM. Timeline audio imports
keep their existing conversion to the session rate.

The Sampler's **Instrument · Pads · Slices** switch changes only the view. It remembers the
choice for each instance while the app is open, including closing and reopening the editor.
Each view keeps its own selection when you return: Pads its pad and bank, Instrument its zone.

The **Instrument** view shows every zone on the key map: a rectangle over the keys and velocities
it plays, its root key and name in the corner and its root as a dot on its foot. A zone on a pad
shows as one key at full velocity with a dashed edge and a lock: it plays on the pad's note, so
its edges do not drag. Click a zone to select it. Drag a side edge to change its key range, the
top edge to change its highest velocity, or the dot to move its root; letting go is one undo step,
and Escape cancels the drag. A drag that the kit cannot take is outlined in red before you let go,
then snaps back with the reason under the key map. A group member's edge moves the whole group's
key range, and a round-robin layer's top edge moves all of its alternatives. A zone's edge fades
show as its fill fading toward the edge. With the Sampler window focused, the arrow keys select the
neighboring zone, Alt with the arrows nudges the waveform handle you last pressed (Shift for ten
frames), and Delete asks before removing the selected zone.

The **Zone** panel under the key map names the selected zone, its file, sample rate and length,
and its keys, velocities, root and edge fades: click `none` (or a width) to type the four fade
widths, key low, key high, velocity low and velocity high; Enter sends them as one undo step and
Escape gives the typing up. Its waveform shows the region between **S** and
**E** (drag either to move it, one undo step) and the loop in magenta. **Loop** sets Off, Forward
or Ping-pong, and **Crossfade** takes milliseconds at the sample's own rate (the tooltip shows the
frames); a crossfade works on forward loops only. When a shorter loop leaves less room than the
stored crossfade, a warning line says how much is used. A zone that plays a slice follows its
slice: its handles are locked and edited in the Slices view. Playback and Amp Envelope edit the
selected zone.

The **Group** panel beside it shows the selected zone's group: one row per velocity layer with its
round-robin alternatives as chips (click one to select that zone) and a **+** chip. On a layer the
**+** chip loads a sample you choose as the layer's next alternative; on a zone in no group it
makes the zone a group of its own; on a pad's zone it first asks, because the zone leaves its pad
(its notes still play the group): nothing changes until **Make group**. Alt-dragging a pad's zone
on the key map asks the same. **Round robin** picks Sequential or Random; while Random, the
**Seed** field and its dice button set the kit's random seed (0 is stored as 1); while Sequential
it shows a dash and the stored seed is kept. **Vel. sens.** sets the selected zone's velocity
sensitivity. While the panel asks about a pad's zone, Enter answers **Make group** and Escape
cancels. A layer with more alternatives than fit on one line continues on the next. Adding zones
from the view bar, SFZ import, and the Shaper and Modulation sections are not available yet.

With no zone, the key map is a drop target: drop an audio file anywhere on the view or choose
**Load…** to load it into the first empty zone (one undo step); a file the Sampler cannot take
shows the reason under the key map and nothing changes. While a sample's waveform is still being
read, the zone waveform says `Analyzing…` and everything stays editable. When a zone's audio file
is missing, its waveform is hatched with a card naming the file; the zone keeps its place on the
map and in its group (relinking is not available yet). When the kit is full (256 zones, or eight
alternatives in a layer), the **+** chip still asks for a sample and the reason shows under the
key map.

The **Slices** view shows the kit's slice loop on the bench: the loop's waveform under a strip of
numbered flags, one per slice start (`S 1` is the loop start, `E` the loop's end), the set in use
(Auto or Manual) and its count, and the time from the loop start under it. Click the waveform to
select the slice under the pointer and hear it while you hold the button (its pad's note at the
piano roll's velocity; the song does not start), or click a slice's cell in the flag strip,
between two flags, to select it without playing; the line under the bench names it, its times,
its pad and that pad's note. Press Space or Enter to hear the selected slice while the Sampler
window has the keyboard. **Zoom** shows the loop at 1×, 2×, 4× or 8× around the selected slice,
with a scroll bar inside the bench past 1×; hover the bench to read the position in milliseconds
from the loop start. Times count the source's own frames at its own sample rate. Selecting,
playing, zooming and scrolling never change the kit. While the loop plays, whether from the bench,
Space, a pad, a MIDI keyboard or the song, a thin line moves through the waveform at the point you
hear, one line per sounding note, so you can see where a hit sits and place or nudge a marker on
it. It follows pitch, loops and reverse; a note outside the zoomed range shows no line, and the
bench does not scroll to follow.

Without a loop, drop an audio file on the bench or choose **Load…**. In **Manual**, double-click
the waveform to add a marker (a dashed line shows where it lands, and says `too close` where it
cannot), drag a marker's handle to move it (a click that does not move leaves it where it is and
plays nothing; Escape, or switching to another window, cancels the drag), click a flag to remove
its marker, and
double-click a slice's cell to name it. Left and Right select slices, Alt+Left and Alt+Right
nudge the selected marker a frame (Shift for ten), and Delete removes it. **SNAP 0×** snaps edits
to the nearest zero crossing. Each edit is one undo step. An edit in **Auto** first copies Auto's
markers to Manual in the same step, and a notice offers **Undo**.

In Auto, the **Detect** panel sets the source (cue points, transients or a grid), the sensitivity
and the grid. A change detects again after a moment and previews the result as dashed lines;
**Replace markers** takes it, **Keep current** drops it. **Use slices** puts the slices on pads
(**Slices to pads**, with a preview of the bank it fills) or writes them as a MIDI clip (**Create
clip**) at the selection's start on the track, else the edit cursor, else the playhead. A refusal
is shown in the engine's words; when the slices' keys no longer run in order, **Map slices to
keys** repairs them.

Under the panels, **Playback** and **Amp Envelope** edit the selected slice's pad zone, as on the
Pads view. **Off · Loop · Ping-pong** loops the selected slice over its whole length; choosing a
loop also sets Gate (it loops while held), so One-shot is greyed while the slice loops. Ping-pong
sets the crossfade to 0 in the same step. The crossfade field beside it (forward loops only) takes
milliseconds at the loop file's own sample rate; its tooltip gives the frames. Enter or a click
elsewhere sets it, on the slice you typed it for even when the click selects another slice. In any
of the view's fields, Escape gives the typing up and leaves the window open. When a later marker
edit makes the slice too short for its crossfade, the field keeps your value and a line says how
much is used. On the bench a looping slice shows a magenta band under its flag, `↻` or `⇄` after
its number, and its crossfade as a hatch at its end.

To fill a kit from a folder, choose **Load kit…** in the Pads bar. Pick a folder, review the
file count and pad range, then choose **Load kit**. Cancel leaves the kit unchanged. When the kit
already has loaded pads, transit asks what to do before anything changes: **Replace** makes the
folder the kit from pad A1 on, as it would load into an empty kit: each pad it fills is cleared
first (its name, choke group, mute, solo and settings go; its trigger note stays), a pad bound to
a slice is replaced like any other, and pads after the last file, the slice markers and the slice
loop stay as they are. A replaced sample leaves the session when nothing else uses it.
**Fill free pads** (the default, Enter) puts the files on the free pads and leaves the loaded pads
alone; it is unavailable, with the reason, when the folder has more files than free pads.
**Cancel** (Escape) changes nothing. The question previews the files and pads of each choice and
says how many loaded pads, and how many slices among them, Replace would replace. Either choice is
one undo step. **Expand to rack…** beside it is not available yet. A refusal
appears under the pads in the engine's words. Loading currently waits for the folder to finish
decoding, and transit does not respond until it has (a load with progress and cancel is planned
for the next release, #450). Through the control channel, use `mix.sampler_load_kit` with the track and `folder`,
and `preview: true` to see the assignments. WAV, AIFF and FLAC files
are ordered by filename without regard to case and placed on the first free pads, A1 through
D16. Occupied pads, including slices, stay as they are, unless you choose Replace (`replace: true`). Subfolders, hidden files, links and
other files are ignored; SFZ mappings are listed but never executed. Files stay where they are,
referenced by path. The whole kit loads in one undo step. If it cannot fit, a file cannot be
read, or an audio limit is exceeded, nothing loads and the result names the reason and counts.
Folder loading currently accepts PCM WAV/AIFF and
FLAC files whose headers declare their length; preview checks headers, while loading also
checks the audio data.

Each Sampler kit has a **pad start note**: the note of pad A1, with every other pad on the next
note up (bank B starts 16 notes above A, C 32 and D 48). A new kit starts at MIDI note 36, the
General MIDI kick, where other DAWs put the first pad of their drum instruments, so drum
clips made elsewhere land on bank A; a kit saved before this setting existed keeps its pads on
MIDI note 60 and up. To match a pad controller whose first pad sends another note, set the start
note to that note instead of remapping the controller: use the **Start note** field before
Load kit in the bar above the pads (drag it up or down a semitone at a time, with Shift by octaves, or turn
Ctrl+wheel over it; double-click it to type a note name such as `C1` or `c#1`, or a MIDI number
from 0 to 64), or the `mix.sampler_pad_start` command. A change the kit cannot take, such as one
that would put a pad on a note another pad keeps, is refused, and the reason shows under the pads.
A pad whose note you set yourself keeps that note when the start note changes; the others move,
and keep their pitch. Clips are never changed: the notes they hold no longer reach the moved pads,
so transpose the pad clips by the distance you moved the start note (select their notes in the
piano roll and move them by that many semitones).

### Kit presets

The Sampler's presets are **kit files**. A kit file holds the whole kit: every pad (its name, note,
choke group, mute and solo), every zone and its settings, the slice loop with its markers, the
groups and round robin, and the pad start note. It does not hold the audio: each sample is
referenced by its file path, as the session references media. A sample in or below the kit file's
folder is stored relative to it, so the folder moves and copies as one; any other sample is stored
by its full path. Kit files end in `.tkit` and live in the `presets/sampler` folder of the settings
folder (Windows: `%APPDATA%\transit\presets\sampler`; macOS:
`~/Library/Application Support/transit/presets/sampler`; Linux: `~/.config/transit/presets/sampler`).
No factory kits ship.

The Sampler window's title bar works as on the other instruments. Click the name field to list your
kits, recall one, save the kit under a new name (type it in **Save As…** and press Enter or
**Save**) or delete the kit shown. The save button writes over the kit you last recalled or saved
in this window; until you have one, it opens the list for Save As. Save As onto the name of a kit
that already exists asks first (`Replace the kit file "808"?`, **Replace** or **Cancel**), and
deleting a kit asks too (`Delete the kit "808"?`, **Delete Kit** or **Cancel**; Enter does nothing
there, since a deleted kit file cannot be brought back, and Escape cancels). The arrows step to the previous
or next kit, wrapping at either end. A recall replaces the whole kit in one undo step, and a run of
arrow steps is one undo step back to the kit you had before the run. When the kit has loaded pads
(or any zone, or a slice loop) and you changed it since you last recalled or saved it, transit asks
first: `Replace the kit? 12 loaded pads will be replaced by "808".` **Replace** (Enter) or
**Cancel** (Escape, nothing changes). An unchanged kit is replaced without asking, since it is in a
file and undo brings it back. After a recall the view, the bank and the selected pad stay as they
were.

A sample that is not where the kit says, that changed since the kit was saved (another length or
sample rate), or that cannot be read is **missing**: its pad and zones load as saved and play
silence, and the rest of the kit loads. On the Pads view a missing pad is hatched and its tooltip
reads `Kick 01.wav is missing`; the Instrument and Slices views show the card naming the file. The
note under the pads names each missing file and the path transit looked for. To fix it, put the
file back at that path (or move the kit folder so relative paths resolve) and recall the kit again,
or load a sample onto the pad (drop it, or **Load…**), which keeps the pad's settings. Relinking is
not available yet. A kit made on Windows names Windows paths, which are missing on macOS and Linux,
and the reverse; a sample on a network share is found only when the kit file is on that same
share. A kit that cannot be recalled (saved by a newer transit, or not a kit) changes nothing, and
the reason shows under the pads.

Through the control channel, `mix.sampler_kit_save` and `mix.sampler_kit_load` take any path (see
the control protocol's Sampler kit presets section). **Load kit…** in the Pads bar is different:
it fills the free pads from a folder of samples (its undo step is **Load Kit Folder**).

## Resonator

The Resonator is a built-in instrument (choose it from a track's instrument slot, beside the Macro
Oscillator): a physical-modeling resonator with six models. It is a port of Émilie Gillet's
open-source resonator firmware (MIT license), checked sample by sample against her C++ code
([ADR 0033](adr/0033-built-in-instruments-mutable-instruments-port.md)); the notices are in
**Help > About transit**.

- **Model**: Modal (a bank of tuned modes: bars, bells, plates), Sympathetic Strings, Inharmonic
  String, FM Voice, Chord Strings (sympathetic strings tuned to a chord) and String and Reverb.
  **Structure**, **Brightness**, **Damping** and **Position** shape each one; on Chord Strings,
  Structure picks the chord.
- **Playing**: every note strikes the next voice in turn with the module's internal exciter while
  the others ring on (**Polyphony** 1, 2 or 4). The module takes one strike per 24-frame block, so
  the notes of a chord strike 0.5 ms apart, like a fast strum. There is no gate: a note-off or the
  sustain pedal does not stop a ringing voice; **Damping** decides how long it rings.
- **Accent** is how much velocity matters: a soft note darkens **Brightness** from that note on (one
  control for every voice, as on the module). **Tonic** is the note the sympathetic strings drone on
  at low Structure; a new note keeps its own pitch (MIDI 69 at 440 Hz), while voices still ringing shift with Tonic until they are struck again.
- **Tune**, **Bend Range** and the matrix's Pitch move every voice at once. The mod wheel and
  aftertouch work through the matrix.
- **Spread**: at polyphony 2 and 4 the module puts odd voices on one output and even voices on the
  other, so notes alternate between the sides at 100%; lower, each side takes in the other, down to
  the module's mono mix at 0% (50% by default).
- The shaper and the modulation matrix are the Macro Oscillator's (transit's addition, off by
  default), on the instrument's output: the amp envelope opens at each note and closes when no key
  is held (the pedal holds it).

The firmware runs at 48 kHz; in a 48 kHz session nothing is converted, and at other rates its
output is converted at the edge and the delay (35 samples at 44.1 kHz, 65 at 88.2 kHz, 70 at
96 kHz) goes to delay compensation. An instrument cannot take a track's audio as its exciter yet.
Presets: Struck Bell, Sympathetic Harp, Steel Pluck, FM Mallet, Strummed Chords, Ambient Pluck,
and your own.

## Texture Synth

The Texture Synth is a built-in instrument (choose it from a track's instrument slot, after the
Resonator): the synth of the [Texture](#texture) insert played from MIDI, what the module plays
when nothing is patched into it. It is a port of Émilie Gillet's open-source texture synthesizer
firmware (MIT license), checked sample by sample against her C++ code
([ADR 0033](adr/0033-built-in-instruments-mutable-instruments-port.md)); the notices are in
**Help > About transit**.

- **What sounds**: a wavetable oscillator and grains of the same wavetables. **Time** picks where
  in the bank's 16 waves they read, **Bank** one of 12 banks (the module's Feedback knob), **Size**,
  **Shape** and **Pitch** the grains' length, envelope and transposition, each with a **Random**
  amount. **Dry/Wet** balances the oscillator (dry) and the grains (wet); it is 100% wet by
  default, because the oscillator has no gate on the module and plays on at the last note's pitch.
  There is no recording buffer: the grains read the wavetables.
- **Playing**: one pitch at a time, with up to 32 grains inside. A key sets the pitch (MIDI 69 at
  440 Hz with **Pitch** at 0) and holds the module's seed gate while any key is held. **Density**
  decides what the gate does: left of center a steady stream of grains while the key is held (the
  first on the note), at the center one grain per note, right of center a random stream. With
  **Latch** on, the notes become a clock that Density divides. A new key while one is held starts
  again at its pitch; letting it go returns to the newest key still held without restarting. Grains
  already started play out after the gate falls.
- **Freeze** holds the cloud: no new grains, the sounding ones keep their level. Saving with Freeze
  on stores the cloud with the session; it sounds again when the session opens (with the shaper on,
  it waits for a note to open the amp envelope). **Seed** is the module's button: held, grains flow
  whatever the keys do. Freeze and Seed are parameters: automate them or map them to a pedal or a
  pad with MIDI Learn.
- **Quality** sets the resolution of the oscillator and the grains (Dirty Tape 4-bit, Clean Tape a
  16-bit oscillator with 6-bit grains, Vintage Digital 7-bit, Digital 16-bit). **Velocity** sets
  the level from each note (the module has no level input). **Bend Range** and the matrix's Pitch
  move the pitch; the mod wheel and aftertouch work through the matrix.
- The shaper and the modulation matrix are the Macro Oscillator's (transit's addition, off by
  default), on the instrument's output: the amp envelope opens at each note and closes when no key
  is held (the pedal holds it), which also gates the oscillator. The matrix reaches the module's
  controls: Time, Size, Shape, Density, Bank, Dry/Wet, Reverb and Pitch.

The synth runs at 48 kHz in the module's 12-frame blocks; in a 48 kHz session nothing is converted,
and at other rates its output is converted at the edge and the delay (35 samples at 22.05, 32 and
44.1 kHz, 65 at 88.2 kHz, 70 at 96 kHz, 129 at 176.4 kHz, 140 at 192 kHz) goes to delay
compensation. An instrument cannot take a track's audio yet. Presets: Grain Pad, Grain Pulse,
Frozen Cloud, Tape Keys, and your own.

## Modal Voice

The Modal Voice is a built-in instrument (choose it from a track's instrument slot, after the
Texture Synth): one exciter-and-resonator voice that you bow, blow or strike. It is a port of Émilie
Gillet's open-source modal synthesizer firmware (MIT license), checked sample by sample against her
C++ code ([ADR 0033](adr/0033-built-in-instruments-mutable-instruments-port.md)); the notices are in
**Help > About transit**.

- **Playing**: the voice has a real gate. A key strikes and holds it; while it is up the bow and the
  blow keep sounding under **Contour** (percussive at the left, sustained at the right); the last
  key up releases the voice. A new note while one is held strikes again at the new pitch; letting go
  of it returns to the newest key still held without a new strike (mono legato, as in Ableton's and
  Logic's mono modes). The sustain pedal holds the gate as a key does.
- **Exciters**: **Bow** (a scratchy noise that also drives bowed waveguides on Modal) with its
  **Timbre**; **Blow** (breath noise; past two thirds a reed-pipe model overblows) with **Flow** and
  **Timbre**; **Strike** (past 80% the raw hit bleeds into the output) with **Mallet** (nine
  percussion samples, a mallet, a plectrum or bouncing particles) and **Timbre**.
- **Model**: Modal (52 tuned modes: membranes, bars, plates, bells), String (stiffness or a
  buzzing bridge by Geometry) or Chord Strings (five strings, Geometry picking one of eleven chords).
  **Geometry**, **Brightness**, **Damping** and **Position** shape the resonator.
- **Space** opens the stereo spread and then a reverb, which freezes from 175%.
- **Velocity** is the module's strength input: soft notes excite more gently, through the module's
  own accent curve (100% by default; at 0% every note plays at full strength). With the FM Pair the
  instrument fades the strength, and at Space of 10% or less the dry oscillator mix, down over
  100 ms once no key is held, so that voice is silent between notes (the module's own FM voice
  drones with nothing patched).
- **Tune**, **Bend Range** and the matrix's Pitch move the voice. The mod wheel and aftertouch work
  through the matrix; Aftertouch to Strength plays the module's pressure input.
- **FM Pair** (the last model) is the module's hidden voice: two FM oscillators through a filter
  swept by the envelope, slowly circling in the stereo field. The panel keeps its knobs: Blow's
  Level, Flow and Timbre set the first oscillator's level, ratio and FM amount, Strike's the
  second's, Bow Level its detune and Bow Timbre the feedback; Geometry sets the filter's shape and
  resonance, Brightness its cutoff, Damping how much the envelope opens it, Position the rotation.
- The shaper and the modulation matrix are the Macro Oscillator's (transit's addition, off by
  default), on the instrument's output, following the gate.

The firmware runs at 32 kHz; its output is converted to the session rate and the delay (49 samples
at 44.1 kHz, 53 at 48 kHz, 97 at 88.2 kHz, 105 at 96 kHz) goes to delay compensation. An instrument
cannot take a track's audio as its exciter yet. Presets: Bowed Glass, Breath Pipe, Struck Bar,
Temple Bell, Plucked Wire, Noise Pad, and your own.

## Filter

The Filter is a built-in insert effect (the device chain's **+**, or the mixer strip's insert
picker): a resonant multimode filter whose cutoff can follow the track's notes, so a plain
instrument such as the Macro Oscillator gets a classic subtractive sweep. It works after any
instrument, built-in or hosted.

- **Mode** (low pass, high pass, band pass, notch) and **Slope** (12 or 24 dB per octave),
  **Cutoff**, **Resonance** (no peak at 0%, a sharp peak at 100%; it does not self-oscillate),
  **Drive** (gain into a soft clipper before the filter; at 0 dB a signal up to full scale passes
  untouched) and **Output**. The curve at the top shows the response at the knob settings, with
  the range the envelope and the LFO reach shaded behind it.
- **Envelope**: an ADSR started by the track's notes, at the note's exact sample, after the
  track's MIDI effects. **Amount** moves the cutoff up (or, negative, down) by up to six octaves
  at the envelope's peak; **Velocity** sets how much a soft note sweeps less.
- **Paraphonic**: there is one filter and one envelope for the whole instrument, after its voices
  are summed. With **Retrigger** on Every note, each new note restarts the envelope for all the
  notes you hold, the classic paraphonic behavior; on Legato it restarts only when no note was
  held. The envelope releases when the last note (or the sustain pedal) lets go. Each voice does
  not get its own filter; for that the filter has to be inside the instrument.
- **LFO**: sine, triangle, saw, square or random steps, at a free **Rate** or, with **Sync**, one
  cycle per **Division** of the project tempo (bars, straight, triplet T or dotted D notes).
  **Amount** swings the cutoff both ways, up to six octaves at 100%; **Retrigger** restarts the
  cycle with the envelope.
- **Follower**: the input's level moves the cutoff (an auto-wah), with its own attack and release.
- **Key Track** moves the cutoff with the last note played: an octave per octave at 100%, none at
  middle C.
- **Amp**: with **Amp Env** on, a second ADSR from the same notes shapes the level, like a
  synthesizer's amplifier, with its own velocity amount. The track is then silent between notes.
  On an audio track or a rack strip, which has no notes, Amp Env on keeps the output silent.

The Filter hears the notes the track's instrument plays: on an audio track, or a rack's own
strip, the envelopes and key tracking do nothing (the LFO and the follower still work). A
bypassed Filter lets go of the notes it held and forgets them until the next note; "bypass all
EQ" also bypasses the Filter, since it is filed under EQ. Every control can be automated (lanes
`plugin:<insert letter>:<id>`, for example `plugin:a:cutoff` for the first insert) and mapped to a
MIDI controller. Presets:
Pluck, Wah, Paraphonic Brass, and your own from the panel's preset menu. Ableton's Auto Filter
and Logic's AutoFilter offer the follower and the LFO; the note-triggered envelope is transit's
addition ([ADR 0038](adr/0038-inserts-read-the-tracks-note-stream.md)).

## Texture

The Texture is a built-in insert effect (the device chain's **+**, or the mixer strip's insert
picker): a granular texture processor that records what comes into it and plays it back as clouds
of overlapping grains, or, with **Size** at the top, as a tape delay. It is a port of Émilie
Gillet's open-source texture synthesizer firmware (MIT), checked bit for bit against her code
([ADR 0033](adr/0033-built-in-instruments-mutable-instruments-port.md)).

- **Grains**: **Time** (where a grain starts, newest to oldest audio), **Size** (its length:
  shortest a third of the way up, up to about four seconds toward the top, played backward below a
  third), **Shape** (its envelope) and **Pitch** (two octaves either way). Each has a **Random**
  knob under it, the module's attenuverter with nothing patched: right adds smooth random movement,
  left occasional jumps, the center none.
- **Density** starts grains at a steady rate left of center and at random right of it, faster
  toward either end, with none at the center. **Feedback** sends the output back into the
  recording, **Dry/Wet** balances the input and the grains, **Reverb** adds the module's reverb.
- **Play**: **Freeze** stops recording, so the grains keep playing what the buffer holds.
  **Seed** starts a grain while you press it (one per press with Density at the center) and keeps
  them coming while you hold it; **Latch** holds it down for you, so grains flow at the rate
  Density sets (on by default). **Trigger** is the seed input's gate for automation, heard when
  **Trig In** is on: its rises start grains, or with Latch on clock them and the delay.
- **The delay mode**: Size at the very top fades the grains out and a tape delay in. Time sets the
  delay, Density divides the clock that Seed taps (twice or more) or Trigger sends, and Size's
  Random sets the echoes' bandwidth. Frozen, the delay cuts the buffer into slices that Time picks.
- **Buffer**: **Quality** picks how the buffer records, each at the module's own rate: Digital
  (16-bit at 48 kHz), Vintage Digital (12-bit at 32 kHz), Clean Tape (12-bit grains at 24 kHz) and
  Dirty Tape (8-bit at 24 kHz, with wow and flutter); the lower qualities hold longer recordings.
  **Length** sizes the buffer from the module's 4 seconds (of Digital) up to 8. **Mono In** records
  the left input only, doubling the time held; **Mono Grains** plays one grain at a time.
- **Input**: **Gain** on Auto (the default) lets the module's input stage learn the input's level
  over its first seconds and hold it where the processor expects it; Manual uses **Level** (0 to
  32 dB) instead, for material that is already leveled.

The firmware runs at its own rate in every session: the Texture converts from the session rate and
back at its edges, so pitch, grain lengths and buffer times are the module's at 44.1, 88.2 or
96 kHz too. In a 48 kHz session the Digital and Clean Tape qualities need no conversion. The
conversion and the firmware's block delay the signal by a few milliseconds (11 samples at 48 kHz
with Digital, about 3 ms with Dirty Tape), which delay compensation makes up for. Changing
Quality, Mono In or Length restarts the firmware: the output fades out over 10 ms, the recording
starts over, and it fades back in. Every control can be automated (for example
`plugin:a:freeze`) and mapped to a MIDI controller. Presets: Granular Cloud, Frozen Pad (a dense,
slow cloud: press Freeze once it has heard what it should hold), Tape Delay, and your own.

## Time Stretcher

The Time Stretcher is a built-in insert effect (the device chain's **+**, or the mixer strip's
insert picker, under Pitch Shift): the pitch shifter and time stretcher of Émilie Gillet's
open-source texture processor firmware (MIT), checked bit for bit against her code
([ADR 0033](adr/0033-built-in-instruments-mutable-instruments-port.md)). It records what comes into
it; two overlapping windows read the recording back at the pitch you set, each new window placed
where it lines up best with the one fading out, so the pitch moves without the speed and the read
position moves without the pitch. (The inherited two-tap Pitch Shifter was retired in the same release, so the Time Stretcher is
the pitch shifter now.)

- **Shift**: **Pitch** (two octaves either way, in semitones; a big jump glides there an octave per
  window), **Fine** (cents) and **Window** (the windows' length: short is grainy, long is smooth).
- **Color**: **Diffusion** smears the output, **Tone** closes a low-pass to the left of center and
  opens a high-pass to the right, **Feedback** sends the output back into the recording (an octave
  up then climbs), **Reverb** adds the module's reverb.
- **Stretch**: **Position** reads further back in the recording without changing the pitch.
  **Freeze** stops recording, so the windows keep reading what the buffer holds: move Position to
  scrub through it at its own pitch. **Trigger** restarts the read at the oldest audio, which then
  glides back to Position over the time since the last press (steady presses make a repeating
  stretch).
- **Buffer**: **Quality** picks how the buffer records, as on the module: Stereo (16-bit, 1 s),
  Mono (16-bit, 2 s), Lo-Fi Stereo and Lo-Fi Mono (8-bit at half the rate, 4 and 7 s). **Input**
  sets the level going in (-18 to +18 dB); there is no automatic gain, as on the module.
- **Dry/Wet** balances the input and the shifted signal; at 0 % the input passes at its level.

The firmware runs at 32 kHz, its own rate, in every session: the device converts from the session
rate and back at its edges, so the pitch and the buffer times are the module's at 44.1, 48 or
96 kHz. The conversion and the firmware's 32-frame block delay the signal by about 3.1 ms (151
samples at 48 kHz), the same for every quality, which delay compensation makes up for. Changing
Quality restarts the firmware: the output fades out over 10 ms, the recording starts over, and it
fades back in. The recording is not saved with the session. Every control can be automated and
mapped to a MIDI controller. Presets: Octave Shimmer, Sub Doubler, Stretch Pad, and your own.

## Cross Modulator

The Cross Modulator is a built-in insert effect that combines two signals: this track (the
modulator) and a carrier, which is the sidechain key you pick in its key picker (the same route the
compressor uses), or, with no key set, its own oscillator. It is a port of Émilie Gillet's
open-source meta-modulator firmware (MIT), checked bit for bit against her code
([ADR 0033](adr/0033-built-in-instruments-mutable-instruments-port.md)).

- **Algorithm** (0 to 8) chooses how they combine: 0 Crossfade, 1 Cross-fold (their sum and product
  through a wavefolder), 2 Diode Ring (an analog ring modulator), 3 Digital Ring, 4 XOR (the two as
  16-bit words, bit by bit), 5 Comparator; between two positions the sound morphs from one to the
  next. Past 5 it crossfades into a 20-band **Vocoder**: this track's band envelopes shape the
  carrier's spectrum, and from 6 to 8 the release grows until the bands freeze. The line under the
  panel names where the knob sits. **Timbre** is the algorithm's own control (the vocoder's formant
  shift).
- **Carrier** and **Modulator** set the two inputs' levels into a soft saturator. **Carrier**: Auto
  takes the key when one is set, else the oscillator; Oscillator always takes the oscillator, the
  key then moving its pitch or phase. **Swap** makes the key the modulator and this track the
  carrier (for example, put it on a synth and key it from a voice to vocode the synth).
- **Oscillator**: **Shape** (Sine, Triangle, Saw for the algorithms; sawtooth, pulse and noise for
  the vocoder) and **Frequency** (65 Hz to 2.09 kHz).
- **Dry/Wet** and **Output**.

A stereo track runs two firmware instances, left with the key's left and right with its right. The
firmware runs at 96 kHz; the device converts from the session rate and back at its edges (the
Texture's converters), so the oscillator's pitch and the vocoder's bands are the module's at any
session rate. The delay (59 samples at 96 kHz, 97 at 44.1 kHz, 99 at 48 kHz) goes to delay
compensation. Every control can be automated and mapped to a MIDI controller. Presets: Ring Bell,
Vocoder, XOR Grit, and your own.

## Random Sequencer

The Random Sequencer is a built-in MIDI effect: add it with "+" in the MIDI Effects card of an
instrument track (or a rack), and it plays random notes into the instrument while the transport
runs. It is a port of Émilie Gillet's open-source random sampler firmware (MIT), checked bit for
bit against her code ([ADR 0033](adr/0033-built-in-instruments-mutable-instruments-port.md)). Its
controls sit in sections (Rhythm, Pitch, Loop, Velocity, Seed); click a section's header to show or
hide it. Right-click its name for the presets: Drifting Melody, Locked Loop, Sparse Random Bass.

- **Rhythm.** **Rate** is how often the clock ticks, as a note value of the song's tempo (4/1 to
  1/64, with dotted and triplet values); ticks stay on that grid when the tempo changes. **Model**
  decides at each tick which of two gates fire (Coin Toss, Clusters, Drums, Independent, Divider,
  Three States) and **Gate Bias** leans it. **Jitter** moves ticks early or late. A note lasts as
  long as its gate: **Gate Length** and **Length Random**. **Play** picks the voices: gate 1, gate
  2, both, the clock (a note every tick), or all three.
- **Pitch.** **Spread** and **Bias** shape where the pitches fall in the **Range** (2 or 5 octaves up
  from **Root** in **Octave**, or 10 around it); **Steps** keeps them to the **Scale** (every
  semitone at 0, then the scale, then ever fewer of its strongest notes, the root alone at 100%).
  **Pitch Mode** spreads the three voices' settings apart (Same, Bump, Tilt).
- **Loop.** **Deja Vu** repeats the random decisions: new at 0, an exact loop of **Loop Length**
  steps across its middle, shuffled above. **Freeze** locks the loop; **Deja Vu On** picks the gates,
  the pitches or both.
- **Velocity** and **Dynamics** set the notes' velocity and how far it varies.
- **Seed** picks the random sequence. With **Latch** on, every play start, locate and loop starts
  it over, so a passage plays the same notes each time; off, each pass differs. **Thru** lets the
  notes you play through as well.

The notes land on exact samples, the same at any buffer size, and every one ends when you stop.
Every control can be automated and mapped to a MIDI controller.

## Upgrading to 0.7.0

transit 0.7.0 stops writing the names it inherited from the SoundCraft fork into two kinds of file:
**clip group files** (`file.export_clip_groups` and `file.import_clip_groups`) and **track
presets** (`track.save_preset` and `track.load_preset`), which you reach through the assistant, MCP,
the control protocol or the CLI. Your sessions need nothing for this: a `.transit` file opens and
saves as before ([ADR 0049](adr/0049-inherited-file-identifiers-retire.md)).

**The old one-sample Sampler is gone (an alpha break).** Builds before 0.7.0 had a Sampler that
played one sample across the keyboard, with its own panel and a **New Kit** button. Every Sampler
is now a kit of pads. When you open an older session, each one-sample Sampler becomes an empty
Sampler kit: its sample assignment and its settings are dropped, and so are the automation lanes,
MIDI mappings, rack macro targets and user pages that pointed at those settings. Nothing is
converted. The sample file stays in the session (it is in the Clip List), so you can drop it onto
a pad again. The load notes name every affected track, for example:

- `track "Keys": the old one-sample Sampler was removed and is now an empty kit; its sample "Hit.wav" stays in the session's files and can be loaded onto a pad again; dropped 2 automation lane(s), 1 MIDI mapping(s)`
- `1 track(s) had the old one-sample Sampler and now have an empty Sampler kit`

The session opens as modified, and the next save writes the empty kit. Tracks imported from an
older session (`file.import_session_data`) and track presets that hold the old Sampler get the
same empty kit; an import does not bring the sample file along. Built-in Sampler user presets held
only the old settings: each startup writes one log line per old preset, the files stay on disk, and
the Sampler does not list them. The Sampler's presets are now kit files ([Kit presets](#kit-presets)).
The control-channel commands `mix.sampler_load` and `mix.sampler_clear` are removed: use
`mix.sampler_zone_source` or `mix.sampler_load_kit`. A session saved by 0.7.0 and opened in
v0.6.2 or earlier warns that it needs `sampler_patch`, has a silent Sampler, and loses the empty
kit if saved there.

**What converts.** Any 0.7 release reads clip group files and track presets saved by older
versions, and writes every new one in transit's format. Reading an old file does not change it, so
the status line and the command's result (its `note` field) say what to do:

- `Clip Group.scgrp: saved before transit 0.7.0; import it and export the clips again to keep it opening in 0.8.0 and later`
- `Bass.json: saved before transit 0.7.0; load it and save it again to keep it opening in 0.8.0 and later`

To convert a clip group file, import it and export the imported clips again with
`file.export_clip_groups`. To convert a track preset, load it onto a track and save it again with
`track.save_preset`. New clip group files are named `.tclips` and new track presets
`.tpreset`; older files keep reading whatever their names, because transit goes by their
contents.

**0.8.0 and later refuse unconverted files.** From 0.8.0, transit reads only files in transit's
format. An older file is refused with:

> This file was saved by a version before transit 0.7.0. Open and save it in transit 0.7.0 first:
> https://github.com/transit-audio/transit/releases/tag/v0.7.0

**In one line:** before you move past 0.7, import each clip group file you want to keep and export
the clips again, and load each track preset you want to keep and save it again, in transit 0.7. If
you have already moved on, install any 0.7 release from the link, convert the files there, and go
back.

**File associations.** Session files register under a new type, `application/x-transit-session`,
and the installer updates it. On Linux, if you had chosen transit by hand as the app for `.transit`
files, choose it once more after upgrading.

## Retired built-in effects

transit retires the effects it inherited from the SoundCraft fork in stages
([ADR 0040](adr/0040-inherited-built-in-effects-retired-in-stages.md)). Stage 1, in v0.6.0, removes
twelve: Pitch Shifter, Drum Synth, Channel Strip, Maximizer, De-Esser, Rectifier, Lo-Fi, Time
Shift, Mod Delay, Chorus, Flanger and Phaser. They are not in the device chain's **+** menu, the
mixer strip's insert menu or the instrument menu any more, and the control commands and the
assistant refuse them.

A session saved by an older build that used one opens with that slot **empty**. The device's
settings go with it, and so do the MIDI mappings, automation lanes and rack macro targets that
pointed at it. Nothing else on the track changes. The status line reads "N insert(s) used retired
built-in effects and were removed", and the load report lists each track,
slot and device by name, for example `track "Vox", insert B: the retired Chorus was removed`. The
session opens as modified, so the removal shows when you save; until then the file still holds the
old devices. A track that used the Drum Synth keeps its clips and notes but has no instrument until
you choose one (the Macro Oscillator's drum models or the Sampler).

What to use instead: the **Compressor** in limiter mode for a Maximizer, the **EQ 7-Band** for a Channel Strip's filters and bands, the
**Macro Oscillator** for the Drum Synth, the **Gain** or **Trim** device for gain. The Pitch
Shifter and the modulation effects have their replacements coming (the ported Time Stretcher and
the Texture insert for echoes and grains).

A user preset file saved for a retired device is skipped, with a line in the log.

Stage 2 (Compressor/Limiter, Expander/Gate, EQ 7-Band and EQ 1-Band, Room and Plate Reverb,
Saturator and Gain) follows each device's replacement, with the same rule for old sessions.

## Tuning a hardware synth

Add **Tuner** (an insert, listed under Other in the insert picker) to the track that carries the synth's audio: for an
External Instrument, put it on the return track, the one that records the synth's input. Play one
note and hold it. The panel shows the note with its octave, a meter from 50 cents flat to 50 cents
sharp that turns green within 3 cents, and the frequency in hertz. Tune the synth until the needle
sits in the green zone.

The audio passes through the Tuner unchanged and with no delay, so it can stay on the track while
you play. **Reference** sets the pitch of the A above middle C (415 to 466 Hz, 440 Hz by default; the Macro
Oscillator's concert pitch uses the same range) and **Hold** freezes the display on the last
reading. The Tuner hears the track's channels summed to one, so a synth on one side of a stereo
return reads at its full level, while a stereo pair in opposite phase cancels, as in any mono sum.
It also shows "No signal" while bypassed. It shows "No signal" when the input is quieter than -50 dBFS or has no clear pitch (a chord,
noise, a note that has not started); it follows one note at a time. The analysis runs only while the
Tuner's window is open, and not in the audio callback, so the device costs the mix nothing.
