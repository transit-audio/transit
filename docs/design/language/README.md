# transit · device UI design language

One panel language for every transit device (instruments, effects, utilities). Graphite surfaces, one **signal** colour for what you set, one **mod** colour for what moves it. Every device is built from the same panels, knobs, displays and routing rows.

Machine-readable tokens: [`tokens.json`](tokens.json). Mockup sources: [`mockups/`](mockups/) (Design canvas `.dc.html` files; they render on the canvas, not standalone).

---

## Principles

1. **Show the sound.** Every section leads with a display of what it does: waveform, curve, envelope, response.
2. **Two colours mean two things.** Signal blue is a value you set. Mod amber is modulation acting on it. Nothing else is coloured, apart from level meters.
3. **Values are always readable.** Every control prints its value in mono, in real units (`1.20 kHz`, `320 ms`, `−3.2 dB`). No hover needed to know a setting.
4. **Panels, not chrome.** Flat raised panels on a graphite ground. No skeuomorphic metal, glow or gradients.

### Arrangement grid

The grid is the arrangement's backbone: one structure that carries tempo, meter and triplets, computed and painted once. Clips are elements sitting on it. View > Grid Over Clips changes only its layer, above the clips instead of beneath them, so the grid reads as one continuous structure across empty lane space and clips alike; nothing re-draws it. Above the clips the lines are translucent, never opaque: each is the lane's own grid contrast applied as a tint (white or black at the alpha that reproduces the lane's grid token over an empty lane), so the empty lane looks the same with the option on or off. Over a clip body they behave like an Overlay blend: black at that alpha over a bright body (luma above 0.5), white over a dark one, split per segment at the body's edges. Over a body the alpha is boosted (3x the lane's) so the finest lines stay findable, capped at 40 percent alpha in both themes (`OVERLAY_BODY_ALPHA_MAX`); a clip's name strip and body are each judged by their own color. The overlay strength is a single named constant (`OVERLAY_GAIN`) and a candidate for a per-machine accessibility preference in the v0.7.0 design pass.

## Colour

| Token | Hex | Use |
|---|---|---|
| `bg.window` | `#131518` | Window ground |
| `bg.titlebar` | `#16181B` | Title bar, routing-row fill |
| `surface.panel` | `#1A1D21` | Panels |
| `surface.raised` | `#22262B` | Chips, model buttons, dropdowns on panels |
| `surface.inset` | `#0D0F11` | Display wells, value fields, segmented tracks |
| `surface.selected` | `#2A2F35` | Selected segment |
| `knob.cap` | `#262A30` | Knob face |
| `line` | `#2B3036` | Panel borders, arc tracks |
| `line.inset` | `#23272C` | Display-well borders |
| `line.strong` | `#383E46` | Knob rim, pills, dashed add-row |
| `grid` | `#1C1F23` | Grid lines inside displays |
| `text` | `#E7E9EC` | Primary text, values, pointers |
| `text.muted` | `#A7ADB6` | Control labels, section labels |
| `text.faint` | `#8C939D` | Meta, units, column headers (≥ 4.5:1 on panel and inset) |
| `ink.ghost` | `#4A5260` | Secondary traces (detuned wave, fade envelope) |
| `ink.arrow` | `#585F69` | Routing arrows, unused source dots |
| `signal` | `#4C9EFF` | Value arcs, curves, selected outline, switch on |
| `mod` | `#F0A73A` | Modulation rings, LFO traces, amounts, GR |
| `meter.ok` | `#5FCB8E` | Level meters |
| `meter.hot` | `#FF6B5B` | Clip / over only |

Signal and mod differ in lightness as well as hue, so they stay distinguishable for colour-blind users. Fills under curves use the curve colour at 12% (8% for the oscilloscope).

## Type

IBM Plex Sans for UI text, IBM Plex Mono for every number and readout.

| Role | Spec |
|---|---|
| `title.brand` | Sans 600 15 |
| `title.device` | Sans 500 14 |
| `title.sectionBar` | Sans 600 14 (collapsible section bars) |
| `label.section` | Sans 600 11, uppercase, +8% tracking, `text.muted` |
| `label.control` | Sans 400 12, `text.muted` |
| `label.hero` | Sans 500 13, `text` (hero knobs) |
| `value` | Mono 500 11, `text` |
| `readout.display` | Mono 400 11, `text.faint` (inside display wells) |
| `tag` | Mono 500 10, +6% tracking, 1 px `line.strong` border |

## Space & shape

- 4 px base. Scale: 4, 8, 12, 16, 24, 32.
- Window padding 16; panel gap 12; panel padding 14 × 16.
- Radius: panel 8, display well 6, control 5, segment 4, source chip pill (13).
- Heights: title bar 52; control 26–32; routing row 40; segment 22–24.
- Panels lay out on a 12-column grid.

## Components

### Knob
- Sizes: **L 72** (a device's 2–4 hero controls), **M 44–48** (default), **S 36–40** (dense sections).
- Geometry (100-unit box): 270° sweep from −135° to +135°; track r 40, stroke 6, `line`; value arc same radius in `signal`; cap r 28 `knob.cap` with 1.5 `line.strong` rim; pointer from r 9 to r 24, stroke 4, `text`.
- Unipolar arcs start at 7 o'clock; **bipolar** arcs start at 12 o'clock.
- **Modulation ring:** outer arc r 47, stroke 3.5, `mod`, from the base value to base + depth. Hidden when the modulation section is disabled.
- States: default, active (cap `#2E333A`), modulated, disabled (arc `ink.ghost`, pointer `ink.arrow`).
- Label below in `label.control`, value below that in `value`. Hero knobs add a context sub-label (`text.faint`, 11) that changes with the engine/model.
- Interaction: vertical drag, Shift for fine, double-click to reset, click the value to type, scroll to nudge.

### Fader
One dB taper everywhere, shared by the fader, its scale and any meter beside it so positions line up.

- **Taper** (fraction of travel from the top): `+6 → 0`, `0 → 0.14`, `−6 → 0.28`, `−12 → 0.42`, `−24 → 0.60`, `−36 → 0.74`, `−48 → 0.86`, `−60 → 0.94`, `−∞ → 1`. Linear between marks; −60…−90 compresses into the last 6%.
- **Variants:**
  - **Channel** (mixer): fader + labelled scale + stereo meter, width 116. Labels `+6 0 −6 −12 −24 −36 −48 −∞` in Mono 10, right-aligned; `0` in `text`, others `text.faint`. Meter grid lines in `line` at each mark.
  - **Compact** (device panels, e.g. Sampler layer level): fader + ticks only, width 40.
  - **Horizontal** (sends, crossfade, layer mix): 4 px track, 12 × 18 cap.
- **Geometry:** track 4 px `line`, radius 2, 10 px top/bottom inset. Fill in `signal` from the bottom up to the cap (`ink.ghost` when the channel is muted). Cap 32 × 16 (compact 28 × 14), radius 3, fill `#2E333A`, 1 px `ink.arrow` rim, 2 px `text` centre line marking the exact value.
- **Meter:** per channel a `surface.inset` well with 1 px `line.inset`; peak fill `meter.ok` at 35%, RMS fill solid on top, 2 px `text` peak-hold line, 4 px clip cell above the scale (`meter.hot` when latched). `meter.hot` is never used below 0 dBFS.
- **Readouts:** gain field under the fader (Mono 12, click to type), peak readout under the meter (Mono 11, click to reset; `meter.hot` when > 0).
- **Interaction:** vertical drag, Shift for fine, Alt-click or double-click to unity, scroll to nudge 0.5 dB.

### Toggle (latching button)
Used for M / S / R / I, option toggles (SYNC, RETRIG, HZ) and any on/off that isn't a whole section.

| Role | Off | On |
|---|---|---|
| All roles | `surface.raised`, `text.muted`, 1 px `line` | — |
| Mute | — | fill `text`, text `bg.window` (inverted) |
| Solo | — | fill `signal`, text `surface.inset` |
| Record arm | — | fill `meter.hot`, text `bg.window` |
| Monitor / option | — | fill `#1E2A38`, text + 1 px border `signal` |
| Unavailable | fill `#1E2125`, text `ink.arrow`, disabled | — |

Height 24, radius 4, Sans 600 11. Unavailable toggles stay visible (ghosted) so strips keep the same layout. A muted channel also dims its name and fader fill.

The **switch** (below) is reserved for enabling an entire section; everything else is a toggle.

### Pad
Drum-machine style trigger, for Drum Synth, Sampler slices and any pad grid.

- **Size:** 56 (grid in a panel), 80 (focused / editor). Square, radius 6, `surface.raised`, 1 px `line`. Grids use 4 px gaps, 4 × 4 by default, pad 1 at bottom-left (MPC order).
- **Content:** pad number top-left (Mono 9–10, `text.faint`), MIDI note top-right (80 px size only), sample name bottom-left (Sans 500 10–11, ellipsis).
- **Hit:** fill rises from the bottom in `signal` at 26%, height = velocity, with a 2 px solid `signal` top edge; it decays over ~250 ms.
- **States:** idle, hit, selected (2 px `signal` border), muted (40% opacity), choke group / group colour (optional 3 px colour tag on top, same as track colour bars), empty (dashed `line.strong` border, transparent, "+ Drop" — accepts a dropped sample).
- **Interaction:** click or MIDI to audition, click to select for editing, drag a sample onto it, drag between pads to swap.

### Channel strip (mixer)
Built from the parts above, 132 wide (main bus 160). Top to bottom: 3 px track colour bar · name + number · input dropdown (+ MIDI OUT) · inserts and sends · output dropdown · pan knob (36, bipolar) with mono value · 2 × 2 toggle grid M S / R I · channel fader · gain + peak readouts.

- **Full height:** inserts (fixed 112 px: up to 3 insert slots + "+ Insert") and sends (fixed 86 px: up to 2 rows with a level bar + "+ Send"). Fixed heights keep every fader aligned across tracks.
- **Insert slot:** 22 px, `surface.raised`, 1 px `line`, radius 4. Device name on the left (Sans 500 11, ellipsis), and an **on/bypass indicator** on the right: a 12 px power glyph (stroke 1.5), `signal` when the insert is on, `text.faint` when bypassed, with the name dimmed to `text.muted` as well. No dots: the glyph is the state. Click the glyph to toggle bypass, click the name to open the device; the indicator keeps the same meaning in the device title bar (A, bypass).
- **Short:** inserts and sends collapse to `FX n` / `Snd n` count buttons; the fader keeps the full scale.
- **Selected strip:** 1 px `signal` border. **Main bus:** Mute / Mono / Dim / Ref toggles, wider meters, integrated LUFS readout.

### Display well
Inset `surface.inset`, 1 px `line.inset`, radius 6, padding 8. Readouts in `readout.display` across the top. Curve in `signal`, stroke 2, 12% fill. Draggable points are hollow white circles (r 4–5, stroke 1.5). A modulation range is a `mod` band at 10% or a dashed `mod` trail.

### Segmented control
Track `surface.inset`, 1 px `line`, radius 6, 2 px padding. Selected segment `surface.selected` with `text`; others transparent with `text.faint`. Icons for shapes (LFO), text for modes (LP/BP/HP).

### Dropdown
`surface.inset` on panels (`surface.raised` inside routing rows), 1 px `line`, radius 5, chevron in `text.faint`.

### Switch
34 × 20, radius 10. On: `signal` track, white knob. Off: `line.strong` track, `text.muted` knob. Paired with a mono `ON`/`OFF` readout.

### Section bar (collapsible)
Used for optional or secondary sections (e.g. Shaper, Modulation).
- **Left:** chevron + title button. Collapses/expands only. Chevron rotates −90° when collapsed.
- **Middle:** optional `tag` (e.g. `TRANSIT EXTENSION`), a description when open, or a one-line mono **summary** of current settings when collapsed.
- **Right:** `ON`/`OFF` readout + switch. Enables/disables only.
- Collapse and enable are independent. A disabled-but-open section dims to 40% opacity and stays editable.
- **Persistence:** collapse is a per-user view setting; enable is a patch parameter saved with the preset.

### Source chip
Pill, `surface.raised`, 1 px `line.strong`, 8 px dot (`mod` when the source is routed, `ink.arrow` when unused), name, route count `×n` in mono. Draggable onto any knob to create a route.

### Routing row
Grid: `#` · Source · → · Destination (with group prefix) · Via · Curve (icon + name) · Amount (bipolar bar + mono value) · mute / remove. Row fill `bg.titlebar`, 1 px `line.inset`, radius 6, height 40. Only active routes are shown; a dashed **Add route** row sits below. The header shows `n / 16 routes`.

### Bipolar amount bar
Track 4 px `line`; fill 7 px `mod` from the centre tick (`text.faint`) towards the value.

### Level meter
4 px bars on a `line` track, `meter.ok`, `meter.hot` only above 0 dBFS.

## Device anatomy

- **A · Title bar.** Track / device, centred preset browser (prev, field, next, save), output meter, bypass, menu, close. Identical on every device.
- **B · Hero.** The device's main display plus its 2–4 most important controls: oscillator scope + XY pad, sample waveform, EQ curve, compressor transfer curve.
- **C · Sections.** Panels on the 12-column grid. Each panel: section label, optional display well, a row of knobs.
- **D · Routing.** Source chips plus routing rows, full width at the bottom. Only for devices that modulate.

## Applying it to upcoming devices

| Device | Hero display | Sections |
|---|---|---|
| Macro Oscillator | Scope + spectrum + Timbre/Morph XY pad, model picker | Tone, Gate, Pitch, Output · Shaper (Amp env, Filter, LFOs) · Modulation |
| Compressor | Transfer curve + level/GR history + GR meter | Dynamics knobs, detector mode, sidechain |
| Sampler | Waveform with start/end/loop handles | Playback, Amp env, Filter, LFOs · Modulation |
| EQ 7-Band | Response curve with band handles over a live spectrum | Selected-band controls, band list |
| Drum Synth | 4 × 4 pad grid + selected pad's waveform/envelope | Per-pad voice, Amp env, Filter · Modulation |
| Mixer | — (channel strips) | Channel strip, short and full height |

## Keeping this document current

This README, `tokens.json` and the mockups are the canonical design source: the repo copy, not any
export. Whenever styling work settles a new component, token, pattern or rule on this framework,
it is added here in the same landing, never later:

- a new component gets a row or subsection under Components (anatomy, states, sizes, the tokens it uses);
- a new or changed token goes into `tokens.json` and the matching table above;
- a new rule or pattern (a layout, an interaction, a readout format) goes under the section it belongs to;
- a device panel built from these parts adds its row to the device table, with a screenshot once it lands.

Review before a panel lands checks that its entry exists. The point is reuse: the sampler, the
stock effects and every later instrument, effect or panel are built from what is documented here,
so an undocumented component gets reinvented.
