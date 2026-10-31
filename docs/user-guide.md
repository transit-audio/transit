# User guide: appearance and interface size

Open **View > Appearance** to choose **System**, **Dark**, or **Light**. System follows
your operating system's appearance. These choices are saved per machine in `ui.json`.

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

## Cycle

Cycle (C, the transport button or Transport > Cycle On) only turns looping on and off; it never
moves the cycle region. Turning Cycle on reuses the region you already have, whether you dragged
it on the loop bar or set it earlier. Only when there is no region does Cycle On make one: from
the span of the selected clips (earliest start to latest end), else the selected time range, else
four bars from the bar holding the playhead, in one undo step with the switch. Cycle Off keeps
the region. To move the region to the current selection, use Transport > Set Cycle to Selection
(Ctrl+L, Cmd+L on macOS), which does not toggle Cycle.

## Takes and comping

With Cycle on, Record keeps a take per lap until Stop; recording over clips keeps the old clips as
the previous take. The takes of one stretch of time form a stack, and the track plays one of them:
the newest when you stop. Takes work the same for audio and MIDI.

- **Take lanes.** After cycle recording the track's take lanes open under it: one lane per take,
  the newest on top, labeled **Take 1**, **Take 2** and so on. A take's clips are highlighted
  where it plays and dimmed elsewhere. Click **T** in the track header (or **Show Take Lanes** in
  the track's right-click menu) to show or hide the lanes; a clip with stacked takes also carries
  an "n takes" chip that does the same. Showing lanes only changes the view.
- **Choose a take.** Click a dimmed clip in a lane: that take plays over the clip's range, and the
  rest of the track keeps playing. Click a lane's label to play that take wherever it was
  recorded. A shorter take plays over its own range, with the longer take around it; choose the
  longer take back and it is one clip again, with its notes and fades whole. Each choice is one
  undo step.
- **Flatten Takes** (the lane menu at the label's right end, the track menu, or the clip menu for
  that stack) keeps what plays and removes the other takes. One undo brings them back.
- **Delete Take** (the lane menu) removes a take that is not playing, after asking.
- A frozen track's takes cannot be changed: unfreeze it first.

Swipe comping (picking a range from one take and the next range from another, with crossfades),
renaming or coloring takes and auditioning a lane are not in this version. Logic's Take Folder and
Live's Take Lanes work the same way for everything above.

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
again.

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

## Macro Oscillator

The Macro Oscillator is a built-in instrument (choose it from a track's instrument slot): up to 16
voices, or one mono voice, with 24 synthesis models, from virtual analog, waveshaping and FM through grain, additive,
wavetable, chords and speech to swarms, noise, string and modal physical models and three drums.
It is a port of Émilie Gillet's open-source macro oscillator firmware (MIT license), by way of
Oliver Rockstedt's Rust port (MIT): the digital synthesis is the original's, checked sample by
sample against her C++ code, but it is a port, not an emulation of the hardware module or its
analog stages. Their license notices are in **Help > About transit**.

- **Model** picks the synthesis model. **Harmonics**, **Timbre** and **Morph** are the model's
  three tone controls; what each does depends on the model (hover a control with the Info panel on,
  Shift+?, for a summary).
- **Playing**: a note opens the voice's low-pass gate at the note's velocity; releasing it closes
  the gate over **Decay**, and **LPG Color** sets how it closes (filtering as it fades at 0%, a plain
  fade and a longer release at 100%). The drum models ring out by themselves over **Decay**; the Chiptune model holds while its key is
  held and fades over **Decay** after it.
- **Voices** sets how many notes sound at once, 1 to 16 (8 by default); the lights beside it show
  each voice (lit: a key holds it; half lit: its release is fading; dark: free). A new note takes a
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
- **Tuning**: the port plays at concert pitch, A4 = 440 Hz. The original firmware's arithmetic is
  kept bit-exact in transit's code for its tests; the module itself plays in tune because its audio
  clock runs at the rate that arithmetic assumes.
- The voices run at 44.1, 48, 88.2 or 96 kHz. At other project rates they run at the nearest of
  these, still in tune, with their envelopes slightly faster or slower. Changing the model while a note
  sounds can click, as on the original.

### Shaper and modulation matrix

Below the module's own controls, a line labels the **shaper** and the **modulation matrix** as
transit's addition: they are not part of the original module, and both are off by default (with the
shaper off and every matrix row Off, the instrument plays exactly as the module does). Click the
line to show or hide them; they show by themselves while they are in use.

- **Shaper** switches on a section after each voice's low-pass gate, per voice: an **amp envelope**
  (Attack, Decay, Sustain, Release, and **Velocity**: how much velocity sets the level) that becomes
  the voice's envelope (the gate stays open and the note ends when the amp envelope's release
  does), and a **filter** (low, band or high pass; 12 or 24 dB; **Cutoff**, **Resonance**, **Drive**,
  **Key Track**) with its own envelope (**Env Amount**, up to six octaves, and its ADSR). The
  filter is the Filter effect's own, one per voice, so a chord's notes each get their own sweep.
  Turning the shaper on or off applies to notes started after the switch; notes already sounding
  finish the way they started, so the switch does not click.
- **LFO 1** and **LFO 2**, per voice: Sine, Triangle, Saw, Square, Sample & hold or Smooth random;
  a **Rate** or, with **Sync**, a **Division** of the tempo; **Retrigger** (on: each new note starts
  the cycle; off: notes pick up the instrument's running LFO, so held notes move together);
  **Fade In** after each note; **Amount**. A synced LFO with Retrigger off follows the song position
  while the transport plays, so it lines up with the bars wherever playback starts. The envelopes
  and LFOs run whether or not the shaper is on: they reach the sound through the matrix. Known
  limit: the song position sets a synced LFO's phase, but Sample & hold and Smooth random values
  depend on the cycles before, so playback from bar 5 and an export from bar 1 can differ in those
  two shapes (the other four match).
- The **matrix** has 16 rows, in two columns of eight. Each row has a **Source**, a **Via**, a
  **Destination**, a **Curve** and an **Amount** (-100% to +100%): every 12 frames, for every voice,
  it adds amount × curve(source) × via to its destination. Sources: LFO 1 and 2, the amp and filter
  envelopes, velocity, the key (centered on middle C, C3), release velocity, aftertouch, poly
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
Plucked String, Key Tracked Timbre, and your own from the panel's preset menu. The panel opens in a
window inside transit's main window, so it minimizes and returns with it.

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
