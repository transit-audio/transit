# transit · device UI design language

One panel language for every transit device (instruments, effects, utilities). Graphite surfaces, one **signal** colour for what you set, one **mod** colour for what moves it. Every device is built from the same panels, knobs, displays and routing rows.

Machine-readable tokens: [`tokens.json`](tokens.json). Mockup sources: [`mockups/`](mockups/) (Design canvas `.dc.html` files; they render on the canvas, not standalone). The application chrome document (draft 1: controls, the decisions, menus, dialogs, the loop indicator, the loop hue, the hatch) is in [`mockups/chrome/`](mockups/chrome/).

In code ([ADR 0045](../../adr/0045-design-language-in-code.md)): `crates/ui-studio/src/tokens.rs` mirrors `tokens.json` (a test reads the file and fails on any value or name that differs, in either direction), `theme.rs` builds the dark and light token sets from it, and the controls below live in `crates/ui-studio/src/widgets/`, one file per control with its layout test.

---

## Principles

1. **Show the sound.** Every section leads with a display of what it does: waveform, curve, envelope, response.
2. **Two colours mean two things.** Signal blue is a value you set. Mod amber is modulation acting on it. Nothing else is coloured, apart from level meters, clip, arm and error (`meter.hot`), and the loop's own hue (magenta, kept apart from both). A warning is not colored.
3. **Values are always readable.** Every control prints its value in mono, in real units (`1.20 kHz`, `320 ms`, `−3.2 dB`). No hover needed to know a setting.
4. **Panels, not chrome.** Flat raised panels on a graphite ground. No skeuomorphic metal, glow or gradients.
5. **No analog dials.** A slow reading is a ballistics mode of the flat meter, never a needle on a dial; the trace or the bar shows the signal, and the VU mode shows how it sits.

### Arrangement grid

The grid is the arrangement's backbone: one structure that carries tempo, meter and triplets, computed and painted once. Clips are elements sitting on it. View > Grid Over Clips changes only its layer, above the clips instead of beneath them, so the grid reads as one continuous structure across empty lane space and clips alike; nothing re-draws it. Above the clips the lines are translucent, never opaque: each is the lane's own grid contrast applied as a tint (white or black at the alpha that reproduces the lane's grid token over an empty lane), so the empty lane looks the same with the option on or off. Over a clip body they behave like an Overlay blend: black at that alpha over a bright body (luma above 0.5), white over a dark one, split per segment at the body's edges. Over a body the alpha is boosted (3x the lane's) so the finest lines stay findable, capped at 40 percent alpha in both themes (`OVERLAY_BODY_ALPHA_MAX`); a clip's name strip and body are each judged by their own color. The overlay strength is a single named constant (`OVERLAY_GAIN`) and a candidate for a per-machine accessibility preference in the v0.7.0 design pass.

## Colour

| Token | Hex | Light | Use |
|---|---|---|---|
| `bg.window` | `#131518` | `#F9FAFA` | Window ground |
| `bg.titlebar` | `#16181B` | `#F6F6F8` | Title bar, routing-row fill |
| `surface.panel` | `#1A1D21` | `#EFF1F3` | Panels |
| `surface.raised` | `#22262B` | `#E3E5E9` | Chips, model buttons, dropdowns on panels |
| `surface.inset` | `#0D0F11` | `#FFFFFF` | Display wells, value fields, segmented tracks |
| `surface.selected` | `#2A2F35` | `#D5D9DE` | Selected segment |
| `knob.cap` | `#262A30` | `#DCE0E4` | Knob face |
| `knob.capActive` | `#2E333A` | `#CED3D9` | Knob face while held; fader cap |
| `line` | `#2B3036` | `#D4D7DD` | Panel borders, arc tracks |
| `line.inset` | `#23272C` | `#E1E4E8` | Display-well borders |
| `line.strong` | `#383E46` | `#BDC3CB` | Knob rim, pills, dashed add-row |
| `grid` | `#1C1F23` | `#EDEEF1` | Grid lines inside displays |
| `text` | `#E7E9EC` | `#202329` | Primary text, values, pointers |
| `text.muted` | `#A7ADB6` | `#484D56` | Control labels, section labels |
| `text.faint` | `#8C939D` | `#5B616B` | Meta, units, column headers (≥ 4.5:1 on panel and inset) |
| `ink.ghost` | `#4A5260` | `#9EA6B5` | Secondary traces (detuned wave, fade envelope) |
| `ink.arrow` | `#585F69` | `#8F96A1` | Routing arrows, unused source dots |
| `signal` | `#4C9EFF` | `#0055BB` | Value arcs, curves, selected outline, switch on |
| `mod` | `#F0A73A` | `#633E08` | Modulation rings, LFO traces, amounts, GR |
| `meter.ok` | `#5FCB8E` | `#1A4F31` | Level meters |
| `meter.hot` | `#FF6B5B` | `#B51200` | Clip, arm, error (an error's icon and edge; its message stays `text`) |

Signal and mod differ in lightness as well as hue, so they stay distinguishable for colour-blind users. Fills under curves use the curve colour at 12% (8% for the oscilloscope).

**Light theme.** The dark set is the design; the light set is derived from it by one rule, so it keeps every role. Grounds (`bg`, `surface`, `knob`, `line`, `grid` and the arrangement grounds below) mirror the dark set: each keeps its contrast ratio to `bg.window`, in the opposite direction, with `surface.inset` at white and the graphite hue kept. Inks (`text`, `ink`, `signal`, `mod`, `meter`, and the loop, cursor and selection colors) keep their hue and at least the contrast ratio they have against `bg.window` in the dark set. Aliases follow their token. v0.7.0 ships dark only: View > Appearance shows System and Light unavailable, and this derived set stays in code, tested, as the baseline for a designed light theme.

### Arrangement and transport

Tokens the device design does not name, used by the arrangement, the transport and the track headers. The grounds and lines are re-derived on the graphite ground at the contrast ratio each had on the previous ground (lanes, ruler and headers against `bg.window`; grid lines against the lane). Grid lines are the lane tinted toward white (toward black in light), so the translucent Grid Over Clips overlay reproduces them over an empty lane. The state colors alias design tokens: record arm is `meter.hot`, mute is `text` (the toggle inverts), solo is `signal`, monitoring and meter levels are `meter.ok`. A warning has no color (see Warnings and errors under Application chrome). A bypassed device's status reads `text.faint`, as an insert slot's does. The loop has its own hue, `loop`, chosen to stay clear of `signal`, `mod`, `meter.ok` and `meter.hot` for normal, protan, deutan and tritan vision.

| Token | Dark | Light | Rust | Contrast (before, now) | Use |
|---|---|---|---|---|---|
| `lane` | `#171A1D` | `#F3F4F6` | `lane` | 1.05, 1.05 on the window | Track lanes |
| `lane.alt` | `#1A1E22` | `#EEF0F2` | `lane_alt` | 1.09, 1.09 | Alternate lanes |
| `ruler` | `#23282D` | `#E0E3E7` | `ruler` | 1.22, 1.23 | Ruler |
| `header` | `#23262C` | `#E2E5E9` | `header` | 1.21, 1.21 | Track headers |
| `header.selected` | `#333840` | `#C6CBD2` | `header_selected` | 1.55, 1.55 | Selected track header |
| `grid.bar` | `#323537` | `#CFCFD1` | `grid_bar` | 1.41, 1.41 on the lane | Bar lines |
| `grid.beat` | `#242729` | `#E3E4E5` | `grid_beat` | 1.15, 1.16 | Beat lines |
| `grid.sub` | `#1C1F21` | `#EEEEF0` | `grid_sub` | 1.05, 1.05 | Sub-beat lines |
| `loop` | `#D05DAD` | `#B73590` | `loop_region` | designed: 5.1 on the window, 4.9 on a lane, 5.35 on the key rows; worst ΔE 23 against signal, mod, meter.ok and meter.hot over every vision type, 36 against text; `ink.dark` on it 5.1 | The loop: the brace, the ruler tint and edges while Cycle is on, the Cycle button, out-of-scale piano roll rows (see The loop below). `loop` is a Rust keyword, so the Rust name stays `loop_region` |
| `cursor` | `#E1E1E3` | `#28282B` | `cursor` | 13.94, 14.01 | Playhead |
| `selection.outline` | `#EDEDF0` | `#1F1F26` | `selection_outline` | 15.52, 15.65 | Selected clip outline, marquee |
| `ink.dark` | `#131518` | `#131518` | `ink_dark` | = `bg.window` | Letters on light fills |
| `ink.light` | `#F6F6F8` | `#F6F6F8` | `ink_light` | unchanged | Letters on dark fills |
| `record` | `#FF6B5B` | `#B51200` | `record` | = `meter.hot` | Record arm, clip cap |
| `mute` | `#E7E9EC` | `#202329` | `mute` | = `text` | Mute on (inverted) |
| `solo` | `#4C9EFF` | `#0055BB` | `solo` | = `signal` | Solo on |
| `monitor` | `#5FCB8E` | `#1A4F31` | `monitor` | = `meter.ok` | Input monitoring, meter levels |
| `focus.ring` | `#4C9EFF` | `#0055BB` | `focus_ring` | = `signal` | Keyboard focus |
| `button.hover` | `#2E333A` | `#CED3D9` | `button_hover` | = `knob.capActive` | Hovered buttons and rows, one step above `surface.raised` |
| `overlay` | `#00000046` | `#0000001C` | `overlay` | unchanged | Dims what lies outside a range |

The older names in code read the design values: `bg` is `bg.window`, `panel` is `surface.panel`, `panel_raised` and `button` are `surface.raised`, `text_dim` is `text.muted`, `accent` is `signal`. `mod` is `modulation` in Rust (a keyword).

### Component fills

Literal fills the component specs use, with their light values (the fader cap is `knob.capActive`).

| Path | Dark | Light | Use |
|---|---|---|---|
| `toggle.on.option.fill` | `#1E2A38` | `#D9E1EB` | Monitor and option toggles when on |
| `toggle.unavailable.fill` | `#1E2125` | `#EAECEF` | Unavailable toggles |
| `switch.onKnob` | `#FFFFFF` | `#FFFFFF` | The switch's knob when on |

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

The app bundles IBM Plex Sans (Regular, Medium, SemiBold) and IBM Plex Mono (Regular, Medium), unmodified, under the SIL OFL 1.1, and registers them as the families `sans`, `sans-medium`, `sans-semibold`, `mono` and `mono-medium`; each role picks the family for its weight (`TypeRole::font_id`). Tracking is applied as extra letter spacing. Plex draws tabular digits by default, so readouts do not jitter.

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
- In code (`widgets/knob.rs`): 200 px of drag for the full travel, Shift a tenth; a wheel notch nudges 1% (Shift 0.1%); the cell is the dial plus a 16 px label row and a 15 px value row, at least 56 wide. The knob reports a normalized position and the typed text; the caller maps, formats and parses.

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
- **In code** (`widgets/fader.rs`): `taper` and its inverse `db_at` are the one taper (below −90 dB is −∞). The Channel variant lays out a 36 px fader column, a 30 px scale column (labels right-aligned at its end, Mono 10) and the meter slot to the right; Compact draws 4 px ticks in `line.strong` beside the track. Gain and peak readouts belong to the strip, not the fader.

### Toggle (latching button)
Used for M / S / R / I, option toggles (SYNC, RETRIG, HZ) and any on/off that isn't a whole section.

| Role | Off | On |
|---|---|---|
| All roles | `surface.raised`, `text.muted`, 1 px `line` | — |
| Mute | — | fill `text`, text `bg.window` (inverted) |
| Solo | — | fill `signal`, text `surface.inset` |
| Record arm | — | fill `meter.hot`, text `bg.window` |
| Monitor / option | — | fill `#1E2A38`, text + 1 px border `signal` |
| Cycle | — | fill `loop`, glyph `ink.dark` (a role color, like solo or arm) |
| Unavailable | fill `#1E2125`, text `ink.arrow`, disabled | — |

Height 24, radius 4, Sans 600 11. Unavailable toggles stay visible (ghosted) so strips keep the same layout. A muted channel also dims its name and fader fill. A toggle is at least square and otherwise fits its label plus 12 px (`widgets/toggle.rs`).

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

In code (`widgets/display_well.rs`): the readout row is 14 px; points are r 4.5 in `text` (white in dark) on `surface.inset` with an 8 px larger hit box; positions are normalized (x left to right, y bottom to top) and a dragged point comes back as a normalized position.

### Segmented control
Track `surface.inset`, 1 px `line`, radius 6, 2 px padding. Selected segment `surface.selected` with `text`; others transparent with `text.faint`. Icons for shapes (LFO), text for modes (LP/BP/HP). In code (`widgets/segmented.rs`): the track is 24 high, segments are radius 4 (`radius.segment`) with Mono 500 11 text, sized to their text plus 10 px a side or spread evenly over a given width.

### Dropdown
`surface.inset` on panels (`surface.raised` inside routing rows), 1 px `line`, radius 5, chevron in `text.faint`. egui's combo box is this dropdown (Application chrome below).

### Switch
34 × 20, radius 10. On: `signal` track, white knob. Off: `line.strong` track, `text.muted` knob. Paired with a mono `ON`/`OFF` readout. In code (`widgets/switch.rs`): the 14 px knob sits 3 px inside the track; the readout (`value`, `text` on, `text.faint` off) takes a 24 px slot 6 px left of the track so the track never moves.

### Section bar (collapsible)
Used for optional or secondary sections (e.g. Shaper, Modulation).
- **Left:** chevron + title button. Collapses/expands only. Chevron rotates −90° when collapsed.
- **Middle:** optional `tag` (e.g. `TRANSIT EXTENSION`), a description when open, or a one-line mono **summary** of current settings when collapsed.
- **Right:** `ON`/`OFF` readout + switch. Enables/disables only.
- Collapse and enable are independent. A disabled-but-open section dims to 40% opacity and stays editable.
- **Persistence:** collapse is a per-user view setting; enable is a patch parameter saved with the preset.
- **In code** (`widgets/section_bar.rs`): the bar is 32 high (the larger control height); a 12 px chevron (stroke 1.5, `text.muted`) and the title in `title.sectionBar` form one button; the tag is boxed 18 high with 6 px padding and radius 4; the description is `label.control` in `text.muted`, the summary `readout.display` in `text.faint`, clipped before the switch. The kit stores neither state: both come in as inputs and go out as clicks, and `body_opacity` gives the 40% dim.

### Source chip
Pill, `surface.raised`, 1 px `line.strong`, 8 px dot (`mod` when the source is routed, `ink.arrow` when unused), name, route count `×n` in mono. Draggable onto any knob to create a route.

### Routing row
Grid: `#` · Source · → · Destination (with group prefix) · Via · Curve (icon + name) · Amount (bipolar bar + mono value) · mute / remove. Row fill `bg.titlebar`, 1 px `line.inset`, radius 6, height 40. Only active routes are shown; a dashed **Add route** row sits below. The header shows `n / 16 routes`.

### Bipolar amount bar
Track 4 px `line`; fill 7 px `mod` from the centre tick (`text.faint`) towards the value. In code (`widgets/bipolar_amount.rs`): the amount runs from −1 to +1 across the full width; drag sideways (Shift a tenth), double-click for zero; the bar is 16 high with a 1 px tick 11 high.

### Level meter
4 px bars on a `line` track, `meter.ok`, `meter.hot` only above 0 dBFS.

**Ballistics** are a pair of time constants (attack and release, the time to reach 99% of a step), with two presets. **Peak fall-off** (attack 0, release 300 ms) is the default for every meter: the bar jumps to any higher value within the frame and falls off over 300 ms, so only the fall is smoothed and transients are never hidden. **VU** (attack 300 ms, release 300 ms) is the symmetric averaging reading, scaled so 0 VU sits at a reference that defaults to −18 dBFS (a device may set −20 or −14), with `meter.hot` only above 0 VU and a `VU` tag. Device panels pick; the peak-hold line and the clip cell are never smoothed. The same bar reads gain reduction: it fills down from the top in `mod`, with a `GR` tag and no over zone. In code (`widgets/level_meter.rs`) the widget is stateless: the caller holds the displayed value and steps it each frame with `meter_step(prev, target, dt, ballistics)`, a one-pole integration in dB on the fader's taper.

## Application chrome

egui's own widgets outside the device panels: the menu bar and its menus, popups, dialogs, tool windows, buttons, fields, combo boxes, checkboxes and radio buttons, scroll bars and tooltips. From the maintainer's application chrome document (draft 1, [`mockups/chrome/`](mockups/chrome/)); the numbers are `chrome` in `tokens.json`. One rule carries over from the devices: color means something. Blue is a value you set or the thing that has focus; red is clip, record and error; nothing else is colored.

In code: `theme.rs` maps the global rules onto egui's style (`visuals`); egui gives buttons, fields and checks one shared stroke and fill, so `crate::chrome` sets the field, combo box, checkbox and radio rules at the call site (`chrome::field`, `chrome::text_field`, `chrome::checkbox`, `chrome::radio_value`) and draws the default button, the modal, the tool-window footer, the menu styles, the status lines and the keyboard focus ring. Device windows keep their 8 px body and egui's earlier button padding until their migration (326-M).

### Buttons
- **Standard:** `surface.raised`, no border, height 26, radius 5, padding 0 12, Sans 500 12 in `text`. Hover `button.hover`; pressed `surface.selected` (no signal edge: that is the option-on look); keyboard focus a 1.5 px `focus.ring` 2 px outside; disabled dims to read as `ink.arrow` (egui dims by one alpha, 0.3).
- **Default:** `signal` fill, ink `surface.inset`, Sans 600 12, padding 0 16; hover `#66ACFF`, pressed `#3E85D8` (`chrome.defaultButton`); disabled the unavailable fill with `ink.arrow`. At most one per dialog; it is the action Enter performs.
- **Icon:** 26 square, transparent, a 14 px glyph in `text.muted`.
- **Toggles keep their 1 px line and muted caps** (Toggle above), so a button and an off toggle never look alike: borderless means momentary, outlined means it latches.
- **Destructive actions:** the label names the loss ("Delete 3 Tracks", "Delete Track", "Delete Group", "Delete Take", never "OK" or a bare "Delete"), never red; it takes the default style when Enter performs it.
- egui sizes its own buttons from the label (egui 0.36 sets it in the Body style) and the padding, so an egui button is about 24 high (12 px a side, 4 above and below); the chrome's own buttons (`chrome::button`, `chrome::default_button`) are exactly 26. egui's interact height stays 18: it also sets the height of every horizontal row and grid row.

### Windows and dialogs
- **Every window:** `surface.panel`, 1 px `line.strong` edge, radius 8, no shadow.
- **Modal:** width 380, padding 18 18 16, no title bar: the question is the title (Sans 600 13), the detail below it (Sans 12, `text.muted`). The scrim is `overlay`. Footer: the default button rightmost, Cancel left of it, a third choice ("Don't Save") alone at the far left, 14 px under the text. Enter is the default, Escape is Cancel. The save-changes question and the delete confirmations are this modal (`chrome::modal`). Windows that open modally with a title bar (About, Keyboard Shortcuts, Quantize) dim what they cover with `overlay` too.
- **Tool window:** the compact title bar, 26 high, on `bg.titlebar` with a 1 px `line` below it; title Sans 500 14; close with an 18 px hit. Body padding 14 16; groups under `label.section`, split by 1 px `line.inset`; the label column `text.muted`. Footer (`chrome::tool_footer`): padding 10 16, a 1 px line above, buttons right-aligned.
- **Two title bars, never mixed:** app windows and dialogs 26 (above); devices 52 (Device anatomy, A).

### Menus and popups
- **Menu bar:** height 32 on `bg.titlebar` with a 1 px `line` below; items height 22, padding 0 8, radius 4, Sans 400 12; the open item takes `button.hover`.
- **Popup** (menu, submenu, context menu, combo list, tooltip): `surface.raised`, 1 px `line.strong`, radius 5, padding 4, no shadow; it opens flush under its item, a submenu overlapping its parent by 4.
- **Row:** height 24, radius 4, Sans 400 12. A 16 px check gutter (the check mark 12 px in `text`, never `signal`: a menu check is a view setting, not a value on a device). The shortcut Mono 400 11 in `text.faint`, right-aligned; a submenu chevron 12 in `text.faint`. Hover `button.hover`; disabled `ink.arrow`. Separator: 1 px `line`, 4 above and below. Delete stays a plain row (no red in menus).
- **Tooltip:** padding 5 8, Sans 12 `text`, after 500 ms, no arrow, 4 px below the target, the shortcut in mono.
- In code the menu bar's menus take the row style (`chrome::menu_style`, 24 px rows); context menus opened elsewhere keep egui's menu style (egui offers no global one), so their rows keep egui's 18 px interact height, and egui keeps its own separator spacing and tooltip padding (the popup padding, 4).

### Fields and controls
- **Text field:** `surface.inset`, 1 px `line` (`line.strong` on hover), radius 5, height 26. Names Sans 400 12; numbers Mono 500 11, right-aligned, the unit after in `text.faint`. Focus 1.5 px `signal`. Error 1.5 px `meter.hot` with the message in `text`. Disabled `#1E2125` with a `line.inset` edge and `ink.arrow` text (in code egui dims a disabled field by its one alpha).
- **Combo box:** the dropdown above: `surface.inset`, 1 px `line`, chevron `text.faint` (`text.muted` on hover or open). Its list is a popup with the current value checked in `text` (lists built from check rows, as in Audio Devices, do this today; lists built from egui's selectable rows still mark the current value with the selection color until they move to check rows).
- **Checkbox and radio:** a 14 px box in an 18 px hit, radius 3 (the radio round). Off `surface.inset` with `line.strong`; on `signal` with the mark in `surface.inset` (the radio: a 6 px `surface.inset` dot in a `signal` ring). Checked is a value set, like the switch.
- **Scroll bars:** floating. At rest a 4 px `ink.ghost` handle; on hover 8 px `ink.arrow` on a `surface.panel` track; `text.faint` while dragging. Inset 2, minimum handle 24, round ends.
- **Text selection** `signal` at 60 percent (35 in the light set); **links** `signal`.

### Warnings and errors
- **A warning** is the message in `text` with a triangle glyph, no color, on a `surface.raised` card with a `line.strong` edge where there is room for one (Audio Devices' low-buffer note in the artboard). A dedicated warning yellow would be a third amber next to `mod` and the old loop color.
- **An error** is a circle-alert in `meter.hot`, with a `meter.hot` edge where there is one; the message stays `text`.
- In code: `chrome::Severity` and `chrome::status_text` / `status_label`: Lucide's triangle-alert for a warning, circle-alert for an error, as the artboard draws them (every red alert icon in the app is circle-alert). The load readout above 80 percent is a warning (the bar and figures in `text`), at 100 percent an error; the MIDI Monitor's closed or missing port is an error, a quiet open input and the paused line warnings; the Unresponsive marker on a device card and a track header is an error; the External Instrument's refused or failed measurement is an error, a stale measurement or a result waiting for Keep a warning.

### The loop
The loop region has its own hue, `loop` (magenta), clear of amber (mod) and blue (selection).

- **Brace** (the strip at the top of the ruler): solid `loop` with the range in Mono 9 `ink.dark` ("5.1 – 9.1 · 4 bars"; the length only when it runs bar line to bar line) while Cycle is on; at 30 percent with a 55 percent edge and the range in `text` while Cycle is off (parked, no lane effect).
- **Cycle on:** the ruler under the loop takes the hue at 20 percent; the loop's edges run down the ruler and the lanes at 75 percent; the lanes outside the loop take `overlay`. Nothing tints inside: the selection stays the only color there.
- **Cycle button:** a role toggle, filled `loop` with an `ink.dark` glyph when on (Toggle table above); the toggle's off look when off.
- **Piano roll:** out-of-scale rows at 12 percent of the hue (the artboard allows 9 to 12), their key labels in the hue; the keys keep their own fills; a live out-of-scale note is solid `loop`.
- The ranges the artboard allows (ruler tint 16 to 20 percent, edges 60 to 75) are pinned to one value each in `alpha` (`loopRulerTint`, `loopEdge`, `loopBraceOff`, `loopBraceOffEdge`, `loopScaleRow`).

### Hatch
A pattern for a region that cannot take input right now: hatch means "can't take input"; dim means "off, still editable"; never both.

- `text` at 7 percent (6 in the light set), 1.5 px lines at −45 degrees, 8 px pitch, anchored to the window so the stripes line up across a header and its lane (`hatch` in `tokens.json`).
- **Use:** inactive tracks (header and lane hatched, clips to ghost outlines in `ink.ghost` with an 18 percent fill, the color bar `ink.ghost`); suspended or missing plug-ins (the hatch fills the panel; the reason and the way out sit on a solid card with a Resume or Retry button); frozen lanes; unavailable insert slots; beyond a clip's end in an editor.
- **Never:** a disabled-but-editable section (it dims to 40 percent), a muted track, a single small control (ghost it instead), anything under 48 px.
- In code `widgets/hatch.rs` (`hatch::paint`, a `HatchLayout` with the lines it drew). Its first uses are the inactive track and the device card of a plug-in that does not answer; frozen lanes, unavailable insert slots and the editor's space past a clip's end move to it in their migrations (today a frozen lane is tinted `signal` at 12 percent, and the piano roll dims past the clip's end with `overlay`).

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
