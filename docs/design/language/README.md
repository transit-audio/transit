# transit · device UI design language

One panel language for every transit device (instruments, effects, utilities). Graphite surfaces, one **signal** colour for what you set, one **mod** colour for what moves it. Every device is built from the same panels, knobs, displays and routing rows.

Machine-readable tokens: [`tokens.json`](tokens.json). Mockup sources: [`mockups/`](mockups/) (Design canvas `.dc.html` files; they render on the canvas, not standalone). The application chrome document (draft 1: controls, the decisions, menus, dialogs, the loop indicator, the loop hue, the hatch) is in [`mockups/chrome/`](mockups/chrome/).

In code ([ADR 0045](../../adr/0045-design-language-in-code.md)): `crates/ui-studio/src/tokens.rs` mirrors `tokens.json` (a test reads the file and fails on any value or name that differs, in either direction), `theme.rs` builds the dark and light token sets from it, and the controls below live in `crates/ui-studio/src/widgets/`, one file per control with its layout test.

---

## Principles

1. **Show the sound.** Every section leads with a display of what it does: waveform, curve, envelope, response.
2. **Two colours mean two things.** Signal blue is a value you set. Mod amber is modulation acting on it. Nothing else is coloured, apart from level meters, clip, arm and error (`meter.hot`), and the loop's own hue (magenta, kept apart from both). A warning is not colored.
3. **Values are always readable.** Every control prints its value in mono, in real units (`1.20 kHz`, `320 ms`, `−3.2 dB`). No hover needed to know a setting. A time prints three significant digits with at most two decimals, in milliseconds below a second and in seconds from it (`0.00 ms` under 0.005 ms, `0.50 ms`, `35.0 ms`, `999 ms`, `1.50 s`, `10.0 s`), as Ableton and Logic print envelope and LFO times; typing takes a bare number or `ms` as milliseconds and a trailing `s` as seconds.
4. **Panels, not chrome.** Flat raised panels on a graphite ground. No skeuomorphic metal, glow or gradients.
5. **No analog dials.** A slow reading is a ballistics mode of the flat meter, never a needle on a dial; the trace or the bar shows the signal, and the VU mode shows how it sits.

### Arrangement grid

The grid is the arrangement's backbone: one structure that carries tempo, meter and triplets, computed and painted once. Clips are elements sitting on it. View > Grid Over Clips changes only its layer, above the clips instead of beneath them, so the grid reads as one continuous structure across empty lane space and clips alike; nothing re-draws it. Above the clips the lines are translucent, never opaque: each is the lane's own grid contrast applied as a tint (white or black at the alpha that reproduces the lane's grid token over an empty lane), so the empty lane looks the same with the option on or off. Over a clip body they behave like an Overlay blend: black at that alpha over a bright body (luma above 0.5), white over a dark one, split per segment at the body's edges. Over a body the alpha is boosted (3x the lane's) so the finest lines stay findable, capped at 40 percent alpha in both themes (`OVERLAY_BODY_ALPHA_MAX`); a clip's name strip and body are each judged by their own color. The overlay strength is a single named constant (`OVERLAY_GAIN`) and a candidate for a per-machine accessibility preference in the v0.7.0 design pass.

## Colour

| Token | Hex | Light | Use |
|---|---|---|---|
| `bg.window` | `#131518` | `#E6E8EB` | Window ground |
| `bg.titlebar` | `#16181B` | `#DFE2E6` | Title bar, routing-row fill |
| `surface.panel` | `#1A1D21` | `#F7F8F9` | Panels |
| `surface.raised` | `#22262B` | `#FFFFFF` | Chips, model buttons, dropdowns on panels |
| `surface.inset` | `#0D0F11` | `#ECEEF1` | Display wells, value fields, segmented tracks |
| `surface.selected` | `#2A2F35` | `#DCE0E5` | Selected segment |
| `knob.cap` | `#262A30` | `#FFFFFF` | Knob face |
| `knob.capActive` | `#2E333A` | `#EEF0F3` | Knob face while held; fader cap |
| `line` | `#2B3036` | `#D3D7DD` | Panel borders, arc tracks |
| `line.inset` | `#23272C` | `#DCE0E5` | Display-well borders |
| `line.strong` | `#383E46` | `#B4BBC4` | Knob rim, pills, dashed add-row |
| `grid` | `#1C1F23` | `#DFE2E7` | Grid lines inside displays |
| `text` | `#E7E9EC` | `#1B1E23` | Primary text, values, pointers |
| `text.muted` | `#A7ADB6` | `#4A515B` | Control labels, section labels |
| `text.faint` | `#8C939D` | `#5F6670` | Meta, units, column headers (≥ 4.5:1 on panel and inset) |
| `ink.ghost` | `#4A5260` | `#A2A9B4` | Secondary traces (detuned wave, fade envelope) |
| `ink.arrow` | `#585F69` | `#7D8591` | Routing arrows, unused source dots |
| `signal` | `#4C9EFF` | `#1764D8` | Value arcs, curves, selected outline, switch on |
| `mod` | `#F0A73A` | `#E17000` | Modulation rings, LFO traces, amounts, GR |
| `meter.ok` | `#5FCB8E` | `#1A7943` | Level meters |
| `meter.hot` | `#FF6B5B` | `#A8200E` | Clip, arm, error (an error's icon and edge; its message stays `text`) |

Signal and mod differ in lightness as well as hue, so they stay distinguishable for colour-blind users. Fills under curves use the curve colour at 12% (8% for the oscillator's rendered cycle). An XY pad's point sits on a `signal` halo at 14% (`alpha.padHalo`), and a voice light whose release is fading is `signal` at 45% (`alpha.voiceReleasing`, "half lit").

**Light theme.** Designed, not derived: the light set keeps every role and every hue of the dark set, and meets targets instead of mirroring contrast ratios (mirroring pushed `mod` to brown and `meter.ok` to forest green). Grounds: `bg.window` is a mid-light graphite, panels sit lighter on it as cards, `surface.raised` and `knob.cap` are white, and `surface.inset` is one step darker than the panel so wells still recess. Inks: text roles are ≥ 4.5:1 on panel and inset; marks are ≥ 3:1 on the panel. `mod` is a true orange (`#E17000`; a darker amber reads brown on white) and a mark colour only (rings, traces, dots, amount bars, never body text); its traces inside display wells are 2.5 px, where the well gives 2.77:1. Ink on a filled role (solo, arm, Cycle, the default button) is the light ground. `meter.ok` is `#1A7943`, one step darker than `#1E8A4C`, chosen so a letter on its fill (the monitor toggle) clears 4.5:1 while the bar still clears 3:1 on the panel. The accents (`signal`, `mod`, `loop`, `meter.ok`, `meter.hot`) stay ≥ ΔE 20 apart for normal, protan, deutan and tritan vision; `meter.hot` is a deep red so it parts from the orange (the derived pair was ΔE 6 for deutan vision). Track, clip and choke colours come from the content palette below. Aliases follow their token. Mockup: the chrome canvas, Light page (palette board, Macro Oscillator, design language, Compressor, Mixer, chrome). v0.7.0 ships dark only: View > Appearance shows System and Light unavailable until this set lands in code.

### Arrangement and transport

Tokens the device design does not name, used by the arrangement, the transport and the track headers. The grounds and lines are re-derived on the graphite ground at the contrast ratio each had on the previous ground (lanes, ruler and headers against `bg.window`; grid lines against the lane). Grid lines are the lane tinted toward white (toward black in light), so the translucent Grid Over Clips overlay reproduces them over an empty lane. The state colors alias design tokens: record arm is `meter.hot`, mute is `text` (the toggle inverts), solo is `signal`, monitoring and meter levels are `meter.ok`. A warning has no color (see Warnings and errors under Application chrome). A bypassed device's status reads `text.faint`, as an insert slot's does. The loop has its own hue, `loop`, chosen to stay clear of `signal`, `mod`, `meter.ok` and `meter.hot` for normal, protan, deutan and tritan vision.

| Token | Dark | Light | Rust | Contrast (before, now) | Use |
|---|---|---|---|---|---|
| `lane` | `#171A1D` | `#F3F4F6` | `lane` | 1.05, 1.05 on the window | Track lanes |
| `lane.alt` | `#1A1E22` | `#EDEFF2` | `lane_alt` | 1.09, 1.09 | Alternate lanes |
| `ruler` | `#23282D` | `#DDE1E5` | `ruler` | 1.22, 1.23 | Ruler |
| `header` | `#23262C` | `#F0F2F4` | `header` | 1.21, 1.21 | Track headers |
| `header.selected` | `#333840` | `#DCE1E8` | `header_selected` | 1.55, 1.55 | Selected track header |
| `grid.bar` | `#323537` | `#CDD1D6` | `grid_bar` | 1.41, 1.41 on the lane | Bar lines |
| `grid.beat` | `#242729` | `#DDE0E4` | `grid_beat` | 1.15, 1.16 | Beat lines |
| `grid.sub` | `#1C1F21` | `#E6E8EB` | `grid_sub` | 1.05, 1.05 | Sub-beat lines |
| `loop` | `#D05DAD` | `#B73590` | `loop_region` | designed: 5.1 on the window, 4.9 on a lane, 5.35 on the key rows; worst ΔE 23 against signal, mod, meter.ok and meter.hot over every vision type, 36 against text; `ink.dark` on it 5.1 | The loop: the brace, the ruler tint and edges while Cycle is on, the Cycle button, out-of-scale piano roll rows (see The loop below). `loop` is a Rust keyword, so the Rust name stays `loop_region` |
| `cursor` | `#E1E1E3` | `#28282B` | `cursor` | 13.94, 14.01 | Playhead |
| `selection.outline` | `#EDEDF0` | `#1F1F26` | `selection_outline` | 15.52, 15.65 | Selected clip outline, marquee |
| `ink.dark` | `#131518` | `#131518` | `ink_dark` | = `bg.window` | Letters on light fills |
| `ink.light` | `#F6F6F8` | `#F6F6F8` | `ink_light` | unchanged | Letters on dark fills |
| `record` | `#FF6B5B` | `#A8200E` | `record` | = `meter.hot` | Record arm, clip cap |
| `mute` | `#E7E9EC` | `#1B1E23` | `mute` | = `text` | Mute on (inverted) |
| `solo` | `#4C9EFF` | `#1764D8` | `solo` | = `signal` | Solo on |
| `monitor` | `#5FCB8E` | `#1A7943` | `monitor` | = `meter.ok` | Input monitoring, meter levels |
| `focus.ring` | `#4C9EFF` | `#1764D8` | `focus_ring` | = `signal` | Keyboard focus |
| `button.hover` | `#2E333A` | `#EEF0F3` | `button_hover` | = `knob.capActive` | Hovered buttons and rows, one step above `surface.raised` |
| `overlay` | `#00000046` | `#0000001C` | `overlay` | unchanged | Dims what lies outside a range |

The older names in code read the design values: `bg` is `bg.window`, `panel` is `surface.panel`, `panel_raised` and `button` are `surface.raised`, `text_dim` is `text.muted`, `accent` is `signal`. `mod` is `modulation` in Rust (a keyword).

### Component fills

Literal fills the component specs use, with their light values (the fader cap is `knob.capActive`).

| Path | Dark | Light | Use |
|---|---|---|---|
| `toggle.on.option.fill` | `#1E2A38` | `#E3ECFA` | Monitor and option toggles when on |
| `toggle.unavailable.fill` | `#1E2125` | `#EDEFF2` | Unavailable toggles |
| `switch.onKnob` | `#FFFFFF` | `#FFFFFF` | The switch's knob when on |

### Content palette

One palette for every colour the user picks: track colour bars, clip bodies and name strips, and choke-group tags. Sixteen hues evenly spaced in OKLCH (22.5° apart from 11°, the rotation that keeps every swatch furthest from the five accents) plus Graphite, the UI's own grey. Each swatch has three values per theme: **bar** (track bars, choke tags, the picker), **body** (clip fill) and **strip** (clip name strip). The user picks a name; the theme supplies the value, so a project's colours survive a theme switch. Mockup: the chrome canvas, Content palette page.

| # | Name | Dark bar · body · strip | Light bar · body · strip |
|---|---|---|---|
| 1 | Red | `#F08696` · `#934955` · `#793943` | `#C8536A` · `#F9C5CA` · `#F0ACB4` |
| 2 | Vermilion | `#F18A74` · `#934C3C` · `#793B2E` | `#CA5840` · `#F9C7BB` · `#F0AF9F` |
| 3 | Orange | `#E99454` · `#8E5223` · `#744117` | `#C16300` · `#F4CBAF` · `#EAB48F` |
| 4 | Amber | `#D7A03D` · `#825B0B` · `#6A4900` | `#AE7300` · `#EAD0A8` · `#DDBB86` |
| 5 | Yellow | `#BDAD3E` · `#70640C` · `#5B5100` | `#948100` · `#DCD6A9` · `#CCC386` |
| 6 | Lime | `#9BB856` · `#586C25` · `#465818` | `#6F8E00` · `#CCDBB0` · `#B6CA91` |
| 7 | Green | `#71C178` · `#39723F` · `#2B5D30` | `#369744` · `#BCDFBD` · `#A0CFA2` |
| 8 | Jade | `#3FC59C` · `#037559` · `#005F47` | `#009A70` · `#AEE2CD` · `#8CD2B8` |
| 9 | Teal | `#00C4BE` · `#007571` · `#005F5C` | `#009692` · `#A7E1DE` · `#81D2CD` |
| 10 | Cyan | `#00BFDC` · `#007185` · `#005C6D` | `#0092AD` · `#A7DFEC` · `#82CFE0` |
| 11 | Sky | `#49B7F1` · `#166B94` · `#06567A` | `#008BCD` · `#AFDBF7` · `#8ECAEE` |
| 12 | Blue | `#77ACFC` · `#3E639C` · `#2F5081` | `#447FD8` · `#BDD6FC` · `#A2C3F5` |
| 13 | Violet | `#9DA0FB` · `#5A5B9B` · `#484980` | `#7272D7` · `#CDD0FC` · `#B7BCF4` |
| 14 | Purple | `#BC95EE` · `#6F5492` · `#5A4278` | `#9366C8` · `#DCCBF5` · `#CBB5EC` |
| 15 | Magenta | `#D58DD7` · `#804E82` · `#693D6A` | `#AD5CB0` · `#EAC7EA` · `#DDAFDD` |
| 16 | Pink | `#E787B8` · `#8C4A6D` · `#733958` | `#BF558F` · `#F4C5DB` · `#E9ACCA` |
| — | Graphite | `#A5ACB2` · `#5E646A` · `#4B5056` | `#7B8187` · `#CED5DC` · `#BBC2C9` |

- **Content, not signal.** With sixteen hues round the wheel some swatch always lands near an accent (best rotation: ΔE 5), so palette colours are calmer than the accents (OKLCH chroma 0.13 against signal's 0.16) and only ever **fill**: bars, clip bodies and strips, choke tags. They never draw a ring, arc, outline, curve or meter, so a blue line still means a value you set on a blue track.
- **Ink:** clip names are `#F6F6F8` on dark bodies (≥ 5.1:1, strips ≥ 7.0) and `text` on light ones (≥ 11, strips ≥ 8.9). Bars are ≥ 6.9:1 on the dark panel and ≥ 3.4:1 on the light one.
- **Why sixteen:** neighbours are already ΔE₂₀₀₀ 8.6 apart, tellable side by side but not across a screen on a 3 px bar; more hues add swatches people cannot tell apart. If tracks ever need more, add a second, muted brightness row of the same sixteen hues; choke groups stay at sixteen.
- **Picker:** a popup (Application chrome) with the sixteen hues as 22 px swatches in two rows of eight, then Graphite and None; the current swatch has a 2 px `text` ring 2 px out. Names show on hover and under the grid.

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
- Heights: title bar 52; **dense control 24**: every control inside a device panel or a strip (toggles, segmented tracks, dropdowns, value fields, the strip's input and output dropdowns), on one centre line when they share a row; **chrome control 26**: dialogs and tool windows (Application chrome); **list row 22**: insert and send rows and a short list's rows; routing row 40; hero controls up to 32. Mockup: the chrome canvas, "Strip details" (11). In code: `layout::DENSE_CONTROL_HEIGHT`, `CHROME_CONTROL_HEIGHT` and `LIST_ROW_HEIGHT` (`layout.denseControlHeight`, `chromeControlHeight`, `listRowHeight`).
- Panels lay out on a 12-column grid.
- **Device windows** lay out at the board's width, 1440, and below it shrink their fractional columns (the hero's 2fr and 1fr, the Shaper's 3, 5, 2 and 2 columns, a routing row's destination) down to a minimum content width of 1192, which is also the window's minimum width. The minimum comes from the content (326-X): a test lays the Macro Oscillator out with every section open and every knob at the widest value it prints, and 1192 is the narrowest multiple of 8 at which no label or readout meets its neighbor or leaves its panel.
- **Content-sized, one shared rule** (326-X; Ableton's device windows and Logic's plug-in windows are content-sized with no resize handle): a device window is the design width (less 32 a side on a narrower screen, never below its minimum) and its content's height (title bar, body, footer), at most the screen. It re-fits in the frame a section opens or closes, has no resize handle, and keeps its top left: when its bottom would leave the screen it moves up, and when it still does not fit the body scrolls vertically between the title bar and the footer. Only its position is remembered. Every device panel calls the one implementation (`crates/ui-studio/src/device_window.rs`) with its own widths, content height and footer, never a copy. **Collapse** (326-V; Ableton folds a device by double-clicking its title bar, Logic has no equivalent for plug-in windows): a double (or triple) click on an empty part of the A bar folds the window to the bar and back, a click on a control doing what the control does; collapsed, the window is the bar plus the frame's edge high at the same width and top left, the body and footer are not drawn, every bar control keeps working, and the window still drags by its bar and closes with Escape. In code: `device_window::collapsed_size`, `device_editor::window::State::collapsed` (per instance for the session, never saved; follow selection carries it), and Window > Collapse All Editors and Expand All Editors (`window.collapse_editors`, `window.expand_editors`).

## Components

### Knob
- Sizes: **L 72** (a device's 2–4 hero controls), **M 44–48** (default), **S 36–40** (dense sections).
- Geometry (100-unit box): 270° sweep from −135° to +135°; track r 40, stroke 6, `line`; value arc same radius in `signal`; cap r 28 `knob.cap` with 1.5 `line.strong` rim; pointer from r 9 to r 24, stroke 4, `text`.
- Unipolar arcs start at 7 o'clock; **bipolar** arcs start at 12 o'clock.
- **Modulation ring:** outer arc r 47, stroke 3.5, `mod`, from the base value to base + depth. Hidden when the modulation section is disabled.
- States: default, active (cap `#2E333A`), modulated, disabled (arc `ink.ghost`, pointer `ink.arrow`).
- Label below in `label.control`, value below that in `value`. Hero knobs add a context sub-label (`text.faint`, 11) that changes with the engine/model.
- Interaction: vertical drag, Shift for fine, double-click to reset (a double or a triple click resets: a double click soon after any other click reads as a triple, and a triple click means nothing else on a control that resets; the fader, the amount bar and the XY pad follow the same rule), click the value to type, Ctrl+scroll (Cmd on macOS) to nudge. A plain wheel is never the knob's: it scrolls the panel or window around it (a device body scrolls) and leaves the value alone; Ctrl+wheel over a knob nudges it and does not scroll. Two conventions stand side by side: the fader and the mixer's controls (in panes that do not scroll) nudge on a plain wheel; device knobs, in bodies that scroll, need Ctrl. In a device window, Escape while a value is typed gives the typing up and leaves the window open.
- In code (`widgets/knob.rs`): 200 px of drag for the full travel, Shift a tenth; a Ctrl+wheel notch nudges 1% (Shift 0.1%) and is consumed (`widgets::command_wheel`), and a wheel gesture's nudges are one undo step (merged until a frame passes without one); the cell is the dial plus a 16 px label row and a 15 px value row, at least 56 wide. A drag accumulates from its start in the knob's own gesture state (egui memory under the knob's id), so the caller's rounding of a stepped parameter picks the step and never becomes the next frame's base, and the dial paints the stored value, not the raw position. The knob reports a normalized position and the typed text; the caller maps, formats and parses. `knob::paint_dial` draws the dial alone for a caller that owns its gestures (the mixer's 36 px pan knob, its value beside it).

### Fader
One dB taper everywhere, shared by the fader, its scale and any meter beside it so positions line up.

- **Taper** (fraction of travel from the top): `+6 → 0`, `0 → 0.14`, `−6 → 0.28`, `−12 → 0.42`, `−24 → 0.60`, `−36 → 0.74`, `−48 → 0.86`, `−60 → 0.94`, `−∞ → 1`. Linear between marks; −60…−90 compresses into the last 6%.
- **Variants:**
  - **Channel** (mixer): fader + labelled scale + stereo meter, width 116. Labels `+6 0 −6 −12 −24 −36 −48 −∞` in Mono 10, right-aligned; `0` in `text`, others `text.faint`; thinned on short travel (Labels by travel, below). Meter grid lines in `line` at each mark.
  - **Compact** (device panels, e.g. Sampler layer level): fader + ticks only, width 40.
  - **Horizontal** (sends, crossfade, layer mix): 4 px track, 12 × 18 cap.
- **Geometry:** track 4 px `line`, radius 2, 10 px top/bottom inset. Fill in `signal` from the bottom up to the cap (`ink.ghost` when the channel is muted). Cap 32 × 16 (compact 28 × 14), radius 3, fill `#2E333A`, 1 px `ink.arrow` rim, 2 px `text` centre line marking the exact value.
- **Frame** (Channel; mockup: the chrome canvas, "Fader construction"): one vertical frame places everything. The level block's travel runs from **Y0** (block top + 10) to **Y1** (Y0 + travel), and the cap's centre line, each label's vertical centre, each tick, each meter grid line and the meter wells themselves (exactly Y0 to Y1) sit at `Y0 + taper(dB) × travel`, on whole physical pixels. The clip cells are 4 high, 6 above Y0, over each meter bar. Every strip in a row, the main bus included, places its level block on the **same Y0 and Y1**: strips align by the frame, not by their own content, so a 0 dB line runs straight across the mixer.
- **Columns** (Channel, 116 wide): the fader column 0–36 (track centred at 18, cap 2–34); labels right-aligned at x 60; the tick 63–67; the meter from x 72, two bars 10 · 3 · 10. The main bus block is 144 wide with bars 14 · 3 · 14 (`barWidthMain`).
- **Labels by travel:** place labels greedily in priority order `0, −∞, −12, −24, +6, −48, −6, −36`, skipping any whose centre would fall closer than 13 px to a label already placed. All eight fit from about 130 pt of travel; at 100 the `−36` drops; below about 93 only `0 −12 −24 −∞` remain (the docked short strip, 80 pt). A tick goes with each placed label only; the meter grid lines stay at every mark.
- **Ticks:** 4 px after the label, toward the meter (3 px past the label's right edge), `line.strong`, `text.muted` at 0 dB. Never on the cap side of a label, where a dash before `0` reads as a minus sign. 0 dB is marked by its label in `text`, its tick in `text.muted` and its meter grid line in `line.strong` (the other grid lines `line`).
- **Main bus without a fader:** its block keeps the frame, the scale column and its meters, so a level can be read off it; the fader column holds a dashed `+ Fader` (24 high, `line.strong` dashed, `text.faint`) at Y0.
- **Meter:** per channel a `surface.inset` well with 1 px `line.inset`; peak fill `meter.ok` at 35% up to the 0 dBFS line and `meter.hot` at 35% from 0 to the peak above it, RMS fill solid on top, 2 px `text` peak-hold line, 4 px clip cell above the scale (`meter.hot` when latched). `meter.hot` is never used below 0 dBFS. **Draw order inside a well:** the well and its edge, the grid lines, the peak fill, the solid RMS fill, the hold line; nothing is drawn over the level, so the grid shows only through the empty part of the well and the 35 % peak zone, and a sustained signal reads as a bright bar with a paler cap above it (the RMS is never drawn at the peak fill's alpha).
- **Readouts:** gain field under the fader (Mono 12, click to type), peak readout under the meter (Mono 11, click to reset; `meter.hot` when > 0).
- **Interaction:** vertical drag, Shift for fine, Alt-click or double-click to unity, scroll to nudge 0.5 dB.
- **In code** (`widgets/fader.rs`): `taper` and its inverse `db_at` are the one taper (below −90 dB is −∞). The Channel variant lays out a 36 px fader column, a 30 px scale column (labels right-aligned at x 60 of the block, Mono 10, the tick at 63–67) and the meter slot to the right, and draws only the labels `fader::placed_marks` places for its travel (Labels by travel; the layout tests sweep travel 80 to 600); Compact draws 4 px ticks in `line.strong` beside the track; Channel draws a 4 px tick 3 px after each placed label's right edge, toward the meter, `line.strong` and `text.muted` at 0 dB, on whole physical pixels (the Fader construction board; `fader::CHANNEL_TICK`, `FaderLayout::ticks`). Gain and peak readouts belong to the strip, not the fader. `fader::paint` draws a fader without taking input, for a caller that owns the gestures (the mixer strip keeps its one-undo-step drags, snapping and MIDI Learn); the cap's center line sits on whole physical pixels. The mixer's fader follows the design taper everywhere it shows gain: the drag, the wheel and the typed gain field stop at +6 dB (as in Ableton and Logic); a higher gain from elsewhere (an old session, an automation lane, MIDI Learn) reads truthfully in the gain field with the cap pinned at the top of the travel.

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

Height 24, radius 4, Sans 600 11. Unavailable toggles stay visible (ghosted) so strips keep the same layout. A muted channel also dims its name and fader fill. A toggle is at least square and otherwise fits its label plus 12 px (`widgets/toggle.rs`); `toggle::paint` draws one without its click, for a caller that owns it (the strip's M S / R I).

The **switch** (below) is reserved for enabling an entire section; everything else is a toggle.

### Pad
Drum-machine style trigger, for Drum Synth, Sampler pads and any pad grid. Choke groups follow ADR 0041: Off or groups 1–16, labelled by number, colour supplementary, groups cross banks; a muted pad, or a non-solo pad while any pad is solo, neither sounds nor chokes.

- **Size:** 56 (grid in a panel), 80 (focused / editor). Square, radius 6, `surface.raised`, 1 px `line`. Grids use 4 px gaps, 4 × 4 by default, pad 1 at bottom-left (MPC order).
- **Top-left:** the pad id, bank + slot (`A6`), Mono 400 9 (56) or 10 (80), `text.faint`. The bank shows because groups cross banks.
- **Top-right:** the **choke badge**, the group number in Mono 500 9 inside a box at least 16 × 14, radius 3, 1 px `line.strong`, `text.muted`; absent when Off. The MIDI note moves to the selected-pad strip.
- **Top edge:** the group's colour as a 3 px tag, the palette **bar** value, only once the user has picked one. A new group has no colour: the number is the identity.
- **Bottom:** sample name bottom-left (Sans 500 10–11, ellipsis); a 14 px **M** chip (fill `text`, letter `bg.window`) or **S** chip (fill `signal`, letter `surface.inset`) bottom-right.
- **Hit:** fill rises from the bottom in `signal` at 26%, height = velocity, with a 2 px solid `signal` top edge; it decays over ~250 ms.
- **Choked:** a pad cut by its group drops its hit fill at once and its badge inverts (fill `text.muted`, number `surface.panel`) for 200 ms, so you can see which pad cut which.
- **Selected:** 2 px `signal` border. Selecting a grouped pad outlines its peers in this bank (1 px dashed `text.muted`, inset 3, radius 4) and puts a 5 px `text.muted` dot on any other bank button that holds one.
- **Muted / solo-excluded:** 40% opacity (the M chip says which). **Empty:** dashed `line.strong` border, transparent, "+ Drop" (accepts a dropped sample).
- **Selected-pad strip:** the choke picker is a segmented row of Off and 1–16 (groups in use brighter, with their colour as a 2 px underline), then a line naming the peers ("Chokes with A5 CH Hat, A7 Ped Hat, B2 Hat FX"), then M and S toggles.
- **Interaction:** click or MIDI to audition, click to select for editing, drag a sample onto it, drag between pads to swap. Mockup: the chrome canvas, "Pads with choke groups".
- **In code** (326-S, `widgets/pad.rs`): both sizes and every state above from the `pad` numbers in `tokens.json`; the insets of the id, the badge, the name and the chip are the board's at each size (56: 5, 4, 5/18/4, 4; 80: 6/7, 5/6, 6/24/6, 5), the chip letter Sans 600 8 (56) or 9 (80), "+ Drop" Sans 400 10 or 11. The dashed edges (empty, peer) are the kit's straight dashes, 3 on 2 (the mixer's add rows). The hit falls linearly from the velocity's share to nothing over 250 ms (`hit_fill`), a cut at or after the hit drops it at once (`live_fill`), and the badge inverts for 200 ms (`choked`); the caller passes the ages from its frame clock. `PadLayout` records every part (rects, inks, opacity). The selected-pad strip and the grid are the Sampler's (`sampler_panel/`): the strip's name is Sans 600 16 (the board's), the choke picker a segmented grid of nine columns (the board's: a `surface.inset` track, 1 px `line`, radius 6, padding and gaps of 3, 22 px segments in Mono 500 10; the pad's group `surface.selected` with `text`, groups in use `text.muted`, the others `ink.arrow`), the peers line Sans 12 `text.muted` at line height 1.5, M and S 28 wide. A double-click on the strip's name renames the pad in place (the app's rename field). Group colors are not drawn yet: the model has no field for a group's color and no picker is placed (the tag and the picker's underline wait for both). A sample dragged from the Files browser or the file manager onto a pad becomes its sample; while a browser drag hovers a pad, the pad is outlined with the selected border (2 px `signal`), because the board has no drop-hover state yet (a gap for the board).

### Channel strip (mixer)
Built from the parts above, 132 wide (main bus 160). Top to bottom: 3 px track colour bar · name + number · input dropdown (+ MIDI OUT) · inserts and sends · output dropdown · pan knob (36, bipolar) with mono value · 2 × 2 toggle grid M S / R I · channel fader · gain + peak readouts.

- **Full height:** inserts (fixed 112 px: up to 3 insert slots + "+ Insert") and sends (fixed 86 px: up to 2 rows with a level bar + "+ Send"). Fixed heights keep every fader aligned across tracks.
- **Insert slot:** 22 px, `surface.raised`, 1 px `line`, radius 4. Device name on the left (Sans 500 11, ellipsis), and an **on/bypass indicator** on the right: a 12 px power glyph (stroke 1.5), `signal` when the insert is on, `text.faint` when bypassed, with the name dimmed to `text.muted` as well. No dots: the glyph is the state. Click the glyph to toggle bypass, click the name to open the device; the indicator keeps the same meaning in the device title bar (A, bypass).
- **Overflow** (more than three inserts or two sends): the section keeps its fixed box, so every fader still lines up, and its rows scroll inside it. The add row stays pinned under the rows and carries the hidden count after its label, Mono 400 10 `text.faint` ("+ Insert · 2 more"). A 12 px fade into the panel fill marks each clipped edge; the scroll bar is the chrome's floating one (4 px at rest, 8 on hover). The wheel scrolls the box only while it overflows; otherwise it goes to the mixer. Mockup: "Strip details" (12). In code (326-T, `mixer/overflow.rs`): the rows scroll in a view the height of the rows the box holds (72 for three inserts, 47 for two sends); the add row draws its label 6 in from the left and the count ("2 more") right-aligned in it, as the board does; the rows leave a 6 px gutter for the bar at rest and 10 on hover (the board's); the handle is the view's share of the rows, at least 24, inset 2, `ink.ghost` at rest, `ink.arrow` on a `surface.panel` track on hover, `text.faint` dragged; the wheel over an overflowing box is taken from the mixer (`StripLayout::inserts_overflow`, `sends_overflow`). With all ten slots in use the pinned row shows the count alone. The "n more…" row is gone.
- **Short:** inserts and sends collapse to `FX n` / `Snd n` count buttons; the fader keeps the full scale.
- **An insert that does not answer** (suspended or missing, ADR 0007): the short strip's `FX n` button carries a 10 px circle-alert in `meter.hot`, ringed in the panel fill, riding its top-right corner, and its tooltip names the device ("Room Reverb is not responding"; two or more, one glyph and the tooltip lists them). In the opened list and on a full-height slot the circle-alert takes the power glyph's place and the name drops to `text.muted`; a click opens the device card, hatched with the reason and Resume (Hatch). Mockup: "Strip details" (13). In code (326-T, `mixer/inserts.rs`): an insert does not answer when `slot_hang` says so (#213: a stuck or given-up host thread, or a render worker held in it); the FX button's glyph is the 10 px `circle-alert` icon 4 past its right and top edges on a disc of `surface.panel` 1 px wider than the glyph (`StripLayout::fx_alert`, `fx_alert_tip`); the tooltip reads "Room Reverb is not responding", or "A, B and C are not responding"; the row's glyph is the same icon at 12 (`InsertRow::alert`), its hover the #213 explanation, and a click on it or the name focuses the track and shows the device chain, where the card is hatched.
- **Selected strip:** 1 px `signal` border. **Main bus:** Mute / Mono / Dim / Ref toggles, wider meters, integrated LUFS readout.
- **In code** (326-R, `crates/ui-studio/src/mixer/`): `anatomy.rs` places the sections from `channelStrip` in `tokens.json` (padding 7 8 8, 6 between sections, the name row 18, dropdowns and toggles 24, the inserts and sends bodies `insertsHeight` 112 and `sendsHeight` 86 each under a 1 px `line.inset` and 6 of padding, `shortRow` 22, the pan row 40, readouts 26), so everything but the fader takes 466 points at full height and 270 short, and every fader lines up. `channel.rs` draws a strip, `inserts.rs` and `sends.rs` the sections and the short variant's count buttons, `short.rs` the list a count button opens (an area of its own, so the menus opened from it can open while it stays; a second click, Escape or a press outside closes it). The section labels (INSERTS, SENDS) are `label.section` at 10 in `text.faint`, as the board draws them; add rows ("+ Insert", "+ Send") are dashed `line.strong` with `text.faint`. A send row is `bg.titlebar` with the target, a 28 px bar on the fader taper and the level in Mono 500 10; it has no room for the device chain's mute toggle, so Mute Send sits in its right-click menu. `StripLayout` and `MixerLayout` (`ui.inspect mixer`) carry what the tests click and check: the color bar, the sections, the count buttons and the open list, each insert row's name and power glyph with their inks, the dial, the toggles, and the fader and meter layouts. The docked pane (475 points: the short strip's 270, the Mixer board's 176 of fader travel, the fader's insets and the pane's margins, `mixer::default_pane_height`; a saved 420, the earlier default, reads as 475; at least 379, the short strip with 80 points of fader travel) shows the short variant; the full mixer window shows full-height strips whose fader fills the window, at least 80 points of travel. The 3 px color bar is cut to the panel's rounded corners (radius 7 inside the 1 px border) as a polygon, since egui clamps a 3 px rect's radius to 1.5 (`StripLayout::color_bar_outline`). The gain, peak and send readouts print the typographic minus (`−6.0`, `−inf`) through one formatter, `fine::minus`; typed text keeps the hyphen. A send row names its target without a "Bus:" prefix (the routing menus keep it for a bare bus). A MIDI strip's input reads "All MIDI" beside OUT, with the channel only when one is picked; the full label is its hover. The FX channel's `FX` tag stays (no board draws an FX channel). Until tracks carry a palette name (the model keeps a free RGB color) the bar bridges: it takes the `bar` value of the palette hue nearest the track's RGB in OKLCH hue angle (the sixteen from `palette.oklch`), and Graphite below a chroma of 0.03 (`mixer/hue.rs`). The power glyph is Lucide's `power` drawn at 12 with a 1.5 stroke. Each channel of the stereo meter latches its own clip cell (the engine publishes a peak per channel). The main bus (#366, `mixer/main_bus.rs`): M, Mono, Dim and Ref in the 2 by 2 grid (Mono and Ref take the monitor role, Dim the mute role, as the board draws them; Alt+click on Ref stores the reference level), and the footer line under the readouts, 13 high after a section gap, in Mono 400 10: "INT" in `text.faint`, the figure in `text` ("−14.2 LUFS", "—" before the first gated block; a click resets it). The main fader gives up the footer's height, so its travel is 19 points shorter than a channel's in the same lane. Without a master track the main-out strip keeps the set, M ghosted. Its input row reads where it plays, Mono 11 `text.faint`: the output path and the device the audio system opened for it ("Out 1-2 · Speakers"; the path alone without a device), and its output dropdown reads the path; it sits `gapMain` (8) after the last strip, "+ FX Channel" after it, and is pinned to the right edge only when the strips do not fit beside it.

### Display well
Inset `surface.inset`, 1 px `line.inset`, radius 6, padding 8. Readouts in `readout.display` across the top. Curve in `signal`, stroke 2, 12% fill. Draggable points are hollow white circles (r 4–5, stroke 1.5). A modulation range is a `mod` band at 10% (`alpha.modBand`) or a dashed `mod` trail. A well shows rendered content, never a live scope of the summed output: one voice's shape is what tells you what a knob does, and a polyphonic mix never reads.

**Modulation reach band** (326-X, the Macro Oscillator's filter well): while anything modulates a curve's parameter, the well fills the region between the curve at the lowest and at the highest value the modulation can reach in `mod` at `alpha.modBand`, under the base curve, which stays on top in `signal`. The filter's reach is read from the route table and Env amt, through the engine's own cutoff mapping: the routes to Cutoff move it by their summed |amount| either way, Env amt from 0 to its full offset (an envelope only adds). No modulation, no band. In code `Well::between` (two curves sampled at the same x positions).

In code (`widgets/display_well.rs`): the readout row is 14 px; points are r 4.5 in `text` (white in dark) on `surface.inset` with an 8 px larger hit box; positions are normalized (x left to right, y bottom to top) and a dragged point comes back as a normalized position.

### Segmented control
Track `surface.inset`, 1 px `line`, radius 6, 2 px padding. Selected segment `surface.selected` with `text`; others transparent with `text.faint`. Icons for shapes (LFO), text for modes (LP/BP/HP). Until the kit has shape icons, LFO shapes are short text (`SIN TRI SAW SQR S&H RND`) spread evenly over the panel's width. In code (`widgets/segmented.rs`): the track is 24 high, segments are radius 4 (`radius.segment`) with Mono 500 11 text, sized to their text plus 10 px a side or spread evenly over a given width.


**View switch** (a variant): the Sampler's view bar, `Instrument · Pads · Slices`, uses Sans 500 12 with 12 px padding a side, because its segments are words naming places in the device, not modes or values; every other segmented control keeps Mono 500 11. The difference in face is part of what keeps it from reading as a playback mode.
### Dropdown
`surface.inset` on panels (`surface.raised` inside routing rows), 1 px `line`, radius 5, chevron in `text.faint`. egui's combo box is this dropdown (Application chrome below). In device panels (326-M, `device_editor::dropdown`, shared since 326-K): 24 high on panels, the dense height (`layout::DENSE_CONTROL_HEIGHT`; 104 wide in an Output column, the label left in `label.control`), 28 in routing rows; the text Sans 500 12 (mono for numbers) with a lead before it: a faint group or operator (`Tone`, `×`), an 8 px dot, or room for an icon; hover `button.hover`; the list is a popup of check rows. Its list rows are the Menus board's: 24 high, radius 4, Sans 400 12, padding 4 on the left, a 16 px check gutter with the check centered in it in `text`, and the text straight after the gutter (no extra gap).

### Switch
34 × 20, radius 10. On: `signal` track, white knob. Off: `line.strong` track, `text.muted` knob. Paired with a mono `ON`/`OFF` readout. In code (`widgets/switch.rs`): the 14 px knob sits 3 px inside the track; the readout (`value`, `text` on, `text.faint` off) takes a 24 px slot 6 px left of the track so the track never moves.

### Section bar (collapsible)
Used for optional or secondary sections (e.g. Shaper, Modulation).
- **Left:** chevron + title button. Collapses/expands only. Chevron rotates −90° when collapsed.
- **Middle:** optional `tag` (e.g. `TRANSIT EXTENSION`), a description when open, or a one-line mono **summary** of current settings when collapsed.
- **Right:** `ON`/`OFF` readout + switch. Enables/disables only.
- Collapse and enable are independent. A disabled-but-open section dims to 40% opacity and stays editable.
- **Persistence:** collapse is a per-user view setting; enable is a patch parameter saved with the preset.
- **Without an enable parameter** (the Macro Oscillator's Modulation: the matrix has none) the bar has no switch; its middle says the count (`5 / 16 routes`) while open and adds the summary while collapsed.
- **Before the user chooses**, a section is open while it is in use (the Shaper on, a route set) and collapsed otherwise; the user's choice is kept per device and section (`UiState::device_sections`, in `ui.json`).
- **In code** (`widgets/section_bar.rs`): the bar is 32 high (the larger control height); a 12 px chevron (stroke 1.5, `text.muted`) and the title in `title.sectionBar` form one button; the tag is boxed 18 high with 6 px padding and radius 4; the description is `label.control` in `text.muted`, the summary `readout.display` in `text.faint`, clipped before the switch. The kit stores neither state: both come in as inputs and go out as clicks, and `body_opacity` gives the 40% dim.

### Source chip
Pill, `surface.raised`, 1 px `line.strong`, 8 px dot (`mod` when the source is routed, `ink.arrow` when unused), name, route count `×n` in mono. Draggable onto any knob to create a route.

- **In code** (326-M): 26 high, radius 13, padding 6 then 8, the name Sans 500 12, the badge in the `tag` type and `text.faint`; hover `button.hover`. One chip per source of the device, in the matrix's order, wrapping after the `Sources — drag onto any knob` label (`label.control`, `text.faint`).
- **Drop:** while carried, a copy follows the pointer with a `mod` edge. Dropped on a knob that has a matrix destination, the first free row (source and destination Off) takes the chip's source and the knob's destination at amount 0, one undo step; the knob then draws its ring. A knob without a destination takes nothing.

### Routing row
Grid: `#` · Source · → · Destination (with group prefix) · Via · Curve (icon + name) · Amount (bipolar bar + mono value) · mute / remove. Row fill `bg.titlebar`, 1 px `line.inset`, radius 6, height 40. Only active routes are shown; a dashed **Add route** row sits below. The header shows `n / 16 routes`.

- **Columns** (326-M): 32 · 200 · 24 · the rest · 180 · 140 · 300 · 64, gap 8, padding 0 8; below the board's width the fixed columns shrink together and the destination keeps at least 160. Column headers are `label.section` in `text.faint`.
- **Cells:** the source a `surface.raised` dropdown with a `mod` dot; the arrow 1.5 px `ink.arrow`; the destination a `surface.raised` dropdown with its group in `text.faint` (Pitch; Tone for Harmonics, Timbre and Morph; Gate for Decay; Output for Level and Pan; Filter for Cutoff, Resonance and Env amount; LFO 1 and LFO 2 for their rate and amount); via and curve on `surface.panel`, via led by `×` and reading `None` when off; the curve's icon is the curve itself sampled into 26 by 18 (1.6 px `text`); the amount bar takes the column less 58 and prints a signed whole percent (`+32%`) right-aligned in a 48 slot; remove is a 26 px icon button (`text.faint`, hover `button.hover`).
- **Active** means a source or a destination is set; **Add route** reveals the next unused row for the session and writes nothing; remove returns the row to its defaults in one undo step. The mute button waits for a per-row mute parameter (not drawn).

### Bipolar amount bar
Track 4 px `line`; fill 7 px `mod` from the centre tick (`text.faint`) towards the value. In code (`widgets/bipolar_amount.rs`): the amount runs from −1 to +1 across the full width; drag sideways (Shift a tenth), double-click for zero; the bar is 16 high with a 1 px tick 11 high.

### Level meter
4 px bars on a `line` track, `meter.ok`, `meter.hot` only above 0 dBFS.

**Title bar variant** (Device anatomy A, 326-M): a 140 by 4 bar, radius 2, filled along the fader's taper with peak fall-off, its readout beside it in `value` and `text.muted` (`-6.0 dB`, `-inf dB`) in a 64 slot. One bar while the device's tap carries the peak of its channels; the board's pair waits for a per-channel tap.

**Vertical channel meter** (326-R, `level_meter::paint_channel`, the mixer's stereo meter): one well per channel, `fader.meter.barWidth` 10 (`barWidthMain` 14 on the main bus) and 3 apart, spanning the fader's travel so it shares the taper; grid lines in `line` at the scale's marks on whole physical pixels; the peak fill at 35 percent, the RMS fill solid on top only where the source has an RMS reading (the engine publishes peaks only, so the strips draw no RMS layer); the hold line; above each well, 4 into the fader's top inset, a 4 px clip cell per channel, `line` until that channel latches. The wells and fills take the board's 1.5 radius and the clip cell 1 (`CHANNEL_WELL_RADIUS`, `CHANNEL_CLIP_RADIUS`). An idle strip draws no `meter.hot` anywhere: a new document clears the Player's last meter snapshot, whose track ids the new document reuses (326-T). It paints only: the strip owns the clicks and steps the reading with the peak fall-off preset (`MeterDisplay::falloff`).

**Ballistics** are a pair of time constants (attack and release, the time to reach 99% of a step), with two presets. **Peak fall-off** (attack 0, release 300 ms) is the default for every meter: the bar jumps to any higher value within the frame and falls off over 300 ms, so only the fall is smoothed and transients are never hidden. **VU** (attack 300 ms, release 300 ms) is the symmetric averaging reading, scaled so 0 VU sits at a reference that defaults to −18 dBFS (a device may set −20 or −14), with `meter.hot` only above 0 VU and a `VU` tag. Device panels pick; the peak-hold line and the clip cell are never smoothed. The same bar reads gain reduction: it fills down from the top in `mod`, with a `GR` tag and no over zone. In code (`widgets/level_meter.rs`) the widget is stateless: the caller holds the displayed value and steps it each frame with `meter_step(prev, target, dt, ballistics)`, a one-pole integration in dB on the fader's taper.

### Model picker
A device's engine choice as a grid of buttons (326-M, the Macro Oscillator): eight columns with 4 px gaps, buttons 30 high, radius 5; groups labeled in a 92 px column on the left (`label.section` in `text.faint`), 8 px between groups. Each button prints its number (two digits, `tag` type) and the model's short name (Sans 500 12), clipped to the button; the full name is the tooltip and shows on the device's model card (326-N). **Short-name rule:** a short name is at most 12 characters, unique within the device, and narrower than the button's text slot (the button less 5 px left, 3 px right, the widest number and a 4 px gap) at the window's narrowest width; a layout test asserts it per name, so a name that clips fails the build. At the Macro Oscillator's narrowest width the slot is about 50 px, which in practice means eight or nine characters. Off: `surface.raised`, 1 px `line`, the number `text.faint`, the name `text.muted`, hover `button.hover`. Current: the option fill (`toggle.on.option.fill`) with a `signal` edge, the number `signal`, the name `text`. A click selects; after it the picker holds the arrow keys (left and right one, up and down a row, not past either end) and shows the keyboard focus ring until a press lands elsewhere. The Macro Oscillator numbers its 24 models in the firmware's order: Pitched 1 to 16, Noise · Perc 17 to 24 (the board's group, from Swarm on).

### XY pad
Two parameters on one surface (326-M, Timbre across and Morph up): a display well 212 square with grid lines at the quarters in `grid`; its axes named inside it in `readout.display` (`TIMBRE →` bottom left, `↑ MORPH` top left); the point a 7 px `signal` dot with a 2 px `text` rim on the halo. Drag sets both (the point follows the pointer), Shift drags fine (a tenth of the pointer's travel), double-click resets both, one undo step per gesture. A route to either parameter draws its reach as a dashed `mod` trail (3 px, dash 1 gap 5) from the point, the horizontal one ending in a hollow 4 px circle. The values print below in `value`, the letters `T` and `M` in `text.faint`.

### Voice lights
One 10 px square (radius 3) per voice, eight to a row with 5 px gaps, right-aligned under the controls that set the count: `signal` while a key holds the voice, `signal` at 45% while its release fades, `line` while free. The hero's display says how many sound (`3 voices`).

### Icons
Every icon the app draws is a Lucide glyph (lucide-static 1.50.0, ISC, unmodified; `crates/ui-studio/assets/icons/`, credit in `ATTRIBUTION.md`) through `crate::icons::Icon`, rasterized in white and tinted with a token. A character is never an icon; typography (the fader scale's `-inf`, an arrow inside a sentence or a readout, a middle dot between facts) stays text. Letters stay letters where they read better (M, S, R, I, Pre/Post).

| Icon | Lucide | Where | Size | Ink |
|---|---|---|---|---|
| Rewind | `skip-back` | Transport | 16, filled | `text` |
| Stop, Play, Record | `square`, `play`, `circle` | Transport, browser preview | 16, filled | `text`; Record `record` when armed |
| Loop | `repeat` | Transport cycle | 16 | `text`; `loop` when on |
| Metronome | `metronome` | Click toggle and its popup | 16 | `text` |
| ChevronRight, ChevronDown | `chevron-right`, `chevron-down` | Disclosure, combo chevron, menu bars | 12-16 | `text.faint`, `text.muted` hot |
| ArrowRight | `arrow-right` | Mixer strip output dropdown (leads the label) | 10 | `ink.arrow` |
| Plus | `plus` | Add lane, device, effect, send, folder, tempo | 11-24 | `text`, `text.dim` |
| Remove | `x` | Close lane, remove device, effect, send | 11-14 | `text.dim`, `record` on hover |
| Minus | `minus` | Meter and tempo steppers | 16 | `text` |
| Check | `check` | Menu checks | 12 | `text` |
| Power | `power` | Insert bypass | 12 | `signal` on, `text.faint` off |
| OpenEditor | `external-link` | Open a plug-in editor, MIDI Monitor direction | 12-16 | `text`, `record` for out |
| Star, StarFilled | `star` | Favorite plug-ins | 12 | `text.faint`, `signal` filled |
| Filter | `funnel` | Browser and menu filters | 12-16 | `text.dim` |
| Folder, FolderOpen, FileAudio, File | `folder`, `folder-open`, `file-audio`, `file` | Browser rows | 14 | `text.dim` |
| Warning | `triangle-alert` | Warnings (no color) | glyph size of the text | `text` |
| Error | `circle-alert` | Errors | glyph size of the text | `meter.hot` |
| Snowflake | `snowflake` | Freeze on the track header | 14 | `text` |
| Headphones | `headphones` | Browser auto-preview | 16 | `text` |
| Help | `circle-help` | Window header question mark (18 px hit) | 16 | `text.dim`, `accent` hot |

The unsaved-changes marker in the menu bar is a 6 px dot painted in `text`, not a glyph.

## Application chrome

egui's own widgets outside the device panels: the menu bar and its menus, popups, dialogs, tool windows, buttons, fields, combo boxes, checkboxes and radio buttons, scroll bars and tooltips. From the maintainer's application chrome document (draft 1, [`mockups/chrome/`](mockups/chrome/)); the numbers are `chrome` in `tokens.json`. One rule carries over from the devices: color means something. Blue is a value you set or the thing that has focus; red is clip, record and error; nothing else is colored.

In code: `theme.rs` maps the global rules onto egui's style (`visuals`); egui gives buttons, fields and checks one shared stroke and fill, so `crate::chrome` sets the field, combo box, checkbox and radio rules at the call site (`chrome::field`, `chrome::text_field`, `chrome::checkbox`, `chrome::radio_value`) and draws the default button, the modal, the tool-window footer, the menu styles, the status lines and the keyboard focus ring. The Macro Oscillator's window is the first device window in the language (326-M, Device window below); the other device windows keep their 8 px body and egui's earlier button padding until their migrations.

### Buttons
- **Standard:** `surface.raised`, no border, height 26, radius 5, padding 0 12, Sans 500 12 in `text`. Hover `button.hover`; pressed `surface.selected` (no signal edge: that is the option-on look); keyboard focus a 1.5 px `focus.ring` 2 px outside; disabled dims to read as `ink.arrow` (egui dims by one alpha, 0.3).
- **Default:** `signal` fill, ink `surface.inset`, Sans 600 12, padding 0 16; hover `#66ACFF`, pressed `#3E85D8` (`chrome.defaultButton`); disabled the unavailable fill with `ink.arrow`. At most one per dialog; it is the action Enter performs.
- **Icon:** 26 square, transparent, a 14 px glyph in `text.muted`.
- **Toggles keep their 1 px line and muted caps** (Toggle above), so a button and an off toggle never look alike: borderless means momentary, outlined means it latches.
- **Destructive actions:** the label names the loss ("Delete 3 Tracks", "Delete Track", "Delete Group", "Delete Take", never "OK" or a bare "Delete"), never red; it takes the default style when Enter performs it.
- egui sizes its own buttons from the label (egui 0.36 sets it in the Body style) and the padding, so an egui button is about 24 high (12 px a side, 4 above and below); the chrome's own buttons (`chrome::button`, `chrome::default_button`) are exactly 26. egui's interact height stays 18: it also sets the height of every horizontal row and grid row.
- **A legacy dialog's row centers on its field:** a row holding a field (`chrome::field` around a drag value, `chrome::text_field`) sets `interact_size.y` to the control height in a scope around `ui.horizontal`, so the row is the field's height before anything is placed and the check and the labels center on it (Audio Devices > MIDI). A note under the rows wraps (`Label::wrap`) to the pane.

### Windows and dialogs
- **Every window:** `surface.panel`, 1 px `line.strong` edge, radius 8, no shadow.
- **Modal:** width 380, padding 18 18 16, no title bar: the question is the title (Sans 600 13), the detail below it (Sans 12, `text.muted`). The scrim is `overlay`. Footer: the default button rightmost, Cancel left of it, a third choice ("Don't Save") alone at the far left, 14 px under the text. Enter is the default, Escape is Cancel. The save-changes question and the delete confirmations are this modal (`chrome::modal`). Windows that open modally with a title bar (About, Keyboard Shortcuts, Quantize) dim what they cover with `overlay` too.
- **Tool window:** the compact title bar, 26 high, on `bg.titlebar` with a 1 px `line` below it; title Sans 500 14; close with an 18 px hit. Body padding 14 16; groups under `label.section`, split by 1 px `line.inset`; the label column `text.muted`. Footer (`chrome::tool_footer`): padding 10 16, a 1 px line above, buttons right-aligned.
- **Two title bars, never mixed:** app windows and dialogs 26 (above); devices 52 (Device anatomy, A).
- **Device window** (326-M; devices stay floating windows, the maintainer's decision of 2026-11-06): the window rule's edge, radius and no shadow, but on the `bg.window` ground the device's panels sit on as cards (the board's), with no egui title bar: the device's A bar is its top and moves the window when dragged; the footer (32, 1 px `line` above) carries the credit and the load readout in `text.faint`. One window per device instance, content-sized with no resize handle (Space & shape, Device windows). The position (the top left) is remembered per instance while the app runs (keyed by the insert, which has no identity that survives a restart) and per device across launches (`UiState::device_windows` in `ui.json`, where the device's last window was when it closed; an entry from before 326-X also holds a size, which is read past): an instance opens where it was left, and one not opened in this launch takes the device's position, moved 24 px down and right while it would land on an open window. A window that follows selection stays where it is and swaps its content (Logic's Link): while it follows it writes no instance's position, and when follow is turned off its position becomes the shown instance's. Either is moved onto the screen when it no longer fits. **Escape** closes the focused device window (the one last clicked or opened, until a press lands outside every device window); a value being typed, an open list or a dialog keeps Escape. **Window > Close All Editors** closes every plug-in editor, built-in and hosted.

### Menus and popups
- **Menu bar:** height 32 on `bg.titlebar` with a 1 px `line` below; items height 22, padding 0 8, radius 4, Sans 400 12; the open item takes `button.hover`.
- **Popup** (menu, submenu, context menu, combo list, tooltip): `surface.raised`, 1 px `line.strong`, radius 5, padding 4, no shadow; it opens flush under its item, a submenu overlapping its parent by 4.
- **A list that holds a field** (the preset list's Save As name) closes on a click outside only (`chrome_pick::dropdown_with_fields`, `Combo::keep_open`); a pick, Save, Delete and Escape close it explicitly, so the click into the field and the typing never do.
- **Row:** height 24, radius 4, Sans 400 12. A 16 px check gutter (the check mark 12 px in `text`, never `signal`: a menu check is a view setting, not a value on a device). The shortcut Mono 400 11 in `text.faint`, right-aligned; a submenu chevron 12 in `text.faint`. Hover `button.hover`; disabled `ink.arrow`. Separator: 1 px `line`, 4 above and below. Delete stays a plain row (no red in menus).
- **Tooltip:** padding 5 8, Sans 12 `text`, after 500 ms, no arrow, 4 px below the target, the shortcut in mono.
- In code the menu bar's menus take the row style (`chrome::menu_style`, 24 px rows); context menus opened elsewhere keep egui's menu style (egui offers no global one), so their rows keep egui's 18 px interact height, and egui keeps its own separator spacing and tooltip padding (the popup padding, 4).

### Fields and controls
- **Text field:** `surface.inset`, 1 px `line` (`line.strong` on hover), radius 5, height 26. Names Sans 400 12; numbers Mono 500 11, right-aligned, the unit after in `text.faint`. Focus 1.5 px `signal`. Error 1.5 px `meter.hot` with the message in `text`. Disabled `#1E2125` with a `line.inset` edge and `ink.arrow` text (in code egui dims a disabled field by its one alpha).
- **Combo box:** the dropdown above: `surface.inset`, 1 px `line`, chevron `text.faint` (`text.muted` on hover or open). Its list is a popup with the current value checked in `text`, opened `chrome.comboPopupGap` (4 px) below the field; a menu-bar menu opens flush under its item. In code (`chrome_pick::Combo`, `chrome_pick::dropdown` for lists opened from a field you draw yourself): the face is 26 high (24 as `.dense()` in device panels and strips, the dense height) and the list is built from check rows (`choice_label`, `choice_value`). A latching choice outside a list is the toggle's option role (`option_label`, `option_value`, `option_toggle`); egui's `selectable_label` is not used.
- **Checkbox and radio:** a 14 px box in an 18 px hit, radius 3 (the radio round). Off `surface.inset` with `line.strong`; on `signal` with the mark in `surface.inset` (the radio: a 6 px `surface.inset` dot in a `signal` ring). Checked is a value set, like the switch.
- **Scroll bars:** floating. At rest a 4 px `ink.ghost` handle; on hover 8 px `ink.arrow` on a `surface.panel` track; `text.faint` while dragging. Inset 2, minimum handle 24, round ends.
- **Text selection** `signal` at 60 percent (35 in the light set); **links** `signal`.

### Warnings and errors
- **A warning** is the message in `text` with a triangle glyph, no color, on a `surface.raised` card with a `line.strong` edge where there is room for one (Audio Devices' low-buffer note in the artboard). A dedicated warning yellow would be a third amber next to `mod` and the old loop color.
- **An error** is a circle-alert in `meter.hot`, with a `meter.hot` edge where there is one; the message stays `text`.
- In code: `chrome::Severity` and `chrome::status_text` / `status_label`: Lucide's triangle-alert for a warning, circle-alert for an error, as the artboard draws them (every red alert icon in the app is circle-alert). The load readout above 80 percent is a warning (the bar and figures in `text`), at 100 percent an error; the MIDI Monitor's closed or missing port is an error, a quiet open input and the paused line warnings; the Unresponsive marker on a device card and a track header is an error; the External Instrument's refused or failed measurement is an error, a stale measurement or a result waiting for Keep a warning.

- **Notice** (information that offers a way back, never a fault): `surface.raised`, 1 px `line.strong`, radius 5, padding 6 8 6 10, a 13 px Lucide `info` in `text`, the message in Sans 12 `text`, and at most one action as a 24 px button filled `button.hover` (`Undo`, `New Kit`). No colour. It goes at the next edit or when its subject changes.
- **Success** has no colour and no banner: a completed action reports in its surface's last-edit line, led by a 12 px Lucide `check` in `text` (`Created clip at 5.1.1 · 8 notes`). Success is the normal case.
### The loop
The loop region has its own hue, `loop` (magenta), clear of amber (mod) and blue (selection).

- **Brace** (the strip at the top of the ruler): solid `loop` with the range in Mono 9 `ink.dark` ("5.1 – 9.1 · 4 bars"; the length only when it runs bar line to bar line) while Cycle is on; at 30 percent with a 55 percent edge and the range in `text` while Cycle is off (parked, no lane effect). The strip is `arrangement.loopStripHeight`, 14 points, so the Mono 9 range sits centered with at least 2 points above and below its text.
- **Cycle on:** the ruler under the loop takes the hue at 20 percent; the loop's edges run down the ruler and the lanes at 75 percent; the lanes outside the loop take `overlay`. Nothing tints inside: the selection stays the only color there.
- **Cycle button:** a role toggle, filled `loop` with an `ink.dark` glyph when on (Toggle table above); the toggle's off look when off.
- **Piano roll:** out-of-scale rows at 12 percent of the hue (the artboard allows 9 to 12), their key labels in the hue; the keys keep their own fills; a live out-of-scale note is solid `loop`.
- The ranges the artboard allows (ruler tint 16 to 20 percent, edges 60 to 75) are pinned to one value each in `alpha` (`loopRulerTint`, `loopEdge`, `loopBraceOff`, `loopBraceOffEdge`, `loopScaleRow`).

### Hatch
A pattern for a region that cannot take input right now: hatch means "can't take input"; dim means "off, still editable"; never both.

- `text` at 7 percent (6 in the light set), 1.5 px lines at −45 degrees, 8 px pitch, anchored to the window so the stripes line up across a header and its lane (`hatch` in `tokens.json`).
- **Use:** inactive tracks (header and lane hatched, clips to ghost outlines in `ink.ghost` with an 18 percent fill, the color bar `ink.ghost`); suspended or missing plug-ins (the hatch fills the panel; the reason and the way out sit on a solid card with a Resume or Retry button); frozen lanes; unavailable insert slots; beyond a clip's end in an editor.
- **Never:** a disabled-but-editable section (it dims to 40 percent), a muted track, a single small control (ghost it instead). Track headers and lanes hatch at every height down to ADR 0044's 24 px floor (a 24 px row still shows three stripes at the 8 px pitch); `minSize` (16 px on the short side) only keeps slivers from being hatched.
- In code `widgets/hatch.rs` (`hatch::paint`, a `HatchLayout` with the lines it drew). Its first uses are the inactive track and the device card of a plug-in that does not answer; frozen lanes, unavailable insert slots and the editor's space past a clip's end move to it in their migrations (today a frozen lane is tinted `signal` at 12 percent, and the piano roll dims past the clip's end with `overlay`).

## Device anatomy

- **A · Title bar.** Track / device, centred preset browser (prev, field, next, save), output meter, bypass, menu, close. Identical on every device.
  - In code (326-M; shared by every device panel since 326-K, `device_editor::title_bar`, and the window around it `device_editor::window`): 52 high on `bg.titlebar` with a 1 px `line` below, padding 0 16, gaps 12. Left: `transit` in `title.brand`, an 18 px `line.strong` divider, the track in `label.control` and `text.faint`, `/`, the device in `title.device`. Centered: previous, the field (300 by 32, `surface.inset`, `line.strong` on hover, the list's name, `Factory` or `User`, in `text.faint` before the preset's, the chevron right), next, and 8 px further, save. The buttons are 32 square, `surface.panel` with a 1 px `line` and radius 6 (hover `button.hover`), their 14 px glyphs `text.muted`; close is borderless.
  - **Reflow** (326-X): the left group (brand, divider, track / device) and the right group (close, menu, bypass, follow, readout, meter) keep their places; the preset browser centers in the free space between them, not on the bar. As the window narrows the field shrinks from 300 toward a floor of 160 before anything moves; below that the device title truncates with an ellipsis (the names' rule), never a control.
  - The preset browser is the existing preset system: previous and next step through the factory presets then the user's (wrapping; from no preset next is the first and previous the last), each a recall in one undo step; the field opens the same list as before (factory, user, Save As, Delete); save writes the current values over the current user preset and, for a factory preset or none, opens the list to Save As.
  - Bypass keeps the insert slot's meaning (`signal` on, `text.faint` bypassed) and is ghosted (`ink.arrow`, no click) on an instrument, which has no bypass. The menu holds Follow Selection and Close All Editors.
  - **Follow selection** (a link glyph, the toggle's option role at 32 square, between the readout and bypass): on, the window shows the focused track's device in the same slot whenever that device is one the panel draws (Logic's Link); off, the default, it stays on its device.
- **B · Hero.** The device's main display plus its 2–4 most important controls: oscillator cycle + XY pad, sample waveform, EQ curve, compressor transfer curve.
- **C · Sections.** Panels on the 12-column grid. Each panel: section label, optional display well, a row of knobs.
  - **The Macro Oscillator's Shaper row** (326-X): Amp Envelope, Filter, LFO 1, LFO 2 (3, 5, 2, 2 columns). The row is as tall as its tallest card's content and the four cards share that height; none stretches past it. The Filter card is the response well and one knob row of nine: Cutoff, Resonance, Drive, Key track, Env amt (M), a 1 px `line` divider with 8 either side, then the filter envelope's Attack, Decay, Sustain, Release (S, on the same label and value lines) under a `FILTER ENV` caption in `label.section`. The filter envelope's mini display is gone: the envelope shows on the response well as its modulation reach band (Display well).
- **D · Routing.** Source chips plus routing rows, full width at the bottom. Only for devices that modulate.

## Applying it to upcoming devices

| Device | Hero display | Sections |
|---|---|---|
| Macro Oscillator | Rendered cycle + spectrum + Timbre/Morph XY pad, model picker. The well draws one cycle of the current model rendered from the parameters (326-W), so the shape is there before a note and follows every knob; while a key holds a voice it follows the newest one (its note for the period, its modulated Harmonics, Timbre and Morph for the shape); the noise and percussion group (17 to 24) draws its first 50 ms from the trigger instead. The cycle spans the plot's width, `signal` 2 px over an 8% fill, its peak at half the height (silence is the midline); after the release it keeps the last shape at the knobs' values. Readouts: `WAVE · <model>` (`BURST · <model>`) left, the voice count and the note it was rendered at (`C3` at rest) right | Tone, Gate, Pitch, Output · Shaper (Amp env, Filter, LFOs) · Modulation. Landed: 326-M (`crates/ui-studio/src/macro_osc_panel/`), with the per-model labels, descriptions and short picker names of 326-N (`crates/dsp/src/plugins/macro_osc_models.rs`); the spectrum waits for its source (the note readout landed with 326-W). Main/Aux/Mix is dropped from the design: the instrument has no separate aux output (Aux mix crossfades inside it), so the model card carries the description in its place (a later aux feature may bring it back) |
| Compressor | Transfer curve + level/GR history + GR meter. Landed (326-K): the hero panel's three columns at the board's widths: Transfer (a 236 square display well, grid lines at the quarters in `grid`, the static curve from threshold, ratio, knee and makeup as the plug-in's gain computer applies them, Limit its ratio of 1000, both axes −60 to 0 dB, in `signal` over the curve fill), History (a display well with grid lines at the quarters across) and GR (the level meter's gain-reduction bar, 14 wide, its `GR` tag under it, peak fall-off, the readout `-4.2 dB` in `value` below). Landed (326-U): the operating point, the unity diagonal and the history well per Compressor displays below; the history's grid adds a line each second | Dynamics knobs (Threshold, Ratio, Knee, Attack, Release, Makeup, Mix; size M in the board's 84 px slots, 8 apart), then a 1 px `line` divider and the sidechain column: `Sidechain` in `label.control` with the key dropdown (the combo box, 110 wide, reading `Kick · pre` or `None`; its list None, Post-fader and Pre-fader, the other tracks and the busses) and under it the mode as a segmented control, Peak or Limit (RMS waits for a detector parameter). The parameters the board does not place (Lookahead, SC HPF as an option toggle, HPF Freq) sit behind a Detector section bar without a switch, open while in use. Design width 960, minimum 888 (the 326-X method). The maintainer's decisions (2026-11-07): the transfer curve is the static curve and ignores Mix, as Ableton's does. The detector control is Peak / Limit until an RMS detector exists (#361). Lookahead, the sidechain high-pass and its frequency live in the Detector section. There is no limit ceiling line: Limit is a ratio of 1000, and the curve flattens on its own. In code: `crates/ui-studio/src/compressor_panel/` (`CompressorLayout`, `HeroLayout` with `unity`, `dot` and `history`, `KeyRecord`, `KnobRecord`, `WellRecord`; `history::HistoryRecord`) |
| Sampler | Waveform with start/end/loop handles | Playback, Amp env, Filter, LFOs · Modulation. **Pads view built; the Instrument and Slices views have their boards (Sampler views, below)** (326-S, `crates/ui-studio/src/sampler_panel/`, ADR 0041). The Pads view's hero row is the Pads panel (`PADS`, the bank buttons A to D as a segmented control at least 34 wide each with a 5 px `text.muted` dot on a bank holding a peer of the selection, the 4 × 4 grid of 56 px pads, the status line `Bank A · 3 of 16 loaded` in `readout.display`) and beside it the Selected pad panel (the Pad entry's strip, taking the rest of the row as ADR 0041's panel note has it); a refused edit's note (circle-alert in `meter.hot`, the message in `text`) sits under the row; the sections row is Playback (Level, Pan, Tune, Fine in knob slots up to 84 px 8 apart, a divider, the trigger mode as a segmented control and Reverse and Key track as option toggles) and Amp Envelope (Attack, Decay, Sustain, Release), at 40 % with no input while an empty pad is selected. Design width 976 (the content at its natural size; there is no device board), minimum 832 (the 326-X method; the title bar sets it, the knob readouts last fit at 616). A one-sample (legacy) Sampler shows its sample line, Load Sample, Clear and its parameter rows unrestyled, with New Kit (`mix.sampler_new_patch`). No factory presets ship (ADR 0041), so the browser lists user presets only, and a user preset holds the legacy parameters, not the patch. Waiting for their entries here: the group colors and their picker, the shaper's filter and LFOs per zone, and the audition velocity (the panel uses the piano roll's) |
| EQ 7-Band | Response curve with band handles over a live spectrum | Selected-band controls, band list |
| Drum Synth | 4 × 4 pad grid + selected pad's waveform/envelope | Per-pad voice, Amp env, Filter · Modulation |
| Mixer | — (channel strips) | Channel strip, short and full height |

### Compressor displays

Decided for the Compressor's hero (the Transfer and History wells); mockups: `Compressor.dc.html` and `chrome/LightCompressor.dc.html`.

- **Unity diagonal** (Transfer): a faint 1 px `line`, dashed 3 3, from (−60, −60) to (0, 0) under the curve, so the ratio reads against "no change".
- **Operating point** (Transfer): a solid 8 px dot in `text` with a 2 px `surface.inset` ring, at (input, curve(input)), hidden below −60 dB. Not `mod` (nothing modulates it) and not `signal`: it sits on the `signal` curve, where a blue dot would vanish, and it is a live position, like the playhead, not a value the user sets. The engine publishes the detector's input level in dB (after the Peak or RMS detector) at UI rate for it.
- **History:** a 4 s window, newest at the right edge, scrolling left. Vertical grid lines each second and horizontal ones at the quarters in `grid`; `−4 s` and `now` in Mono 400 10 `text.faint` in the bottom corners. Input: an area from the bottom in `ink.ghost` at 35%, no line. Output: a 1.75 px `signal` line, no fill. Gain reduction: a band hanging from the top in `mod` at 22% with a 1.75 px `mod` line at its lower edge (gain reduction is one of mod's uses, Colour), on its own scale of 0 to 24 dB over the well's height. Threshold: a 1 px dashed (4 4) line in `text` at 45% at the threshold on the level scale (−60 to 0 dB, the Transfer well's). The legend sits right of the `HISTORY` label: an 8 px `ink.ghost` swatch `IN`, a 10 × 2 `signal` stroke `OUT`, an 8 px `mod` swatch `GR`, labels Mono 400 11 `text.faint`. Hovering the legend or the well shows the measurement convention (help entry `compressor.history`, as a tooltip and in the Info panel): IN and OUT are absolute levels in dBFS at the detector, OUT includes the makeup gain and sits above IN until the gain reduction exceeds the makeup, and GR is the gain reduction measured down from the top of the well (0 to 24 dB).

In code (326-U): the engine's half is `Plugin::detector_level_db` (the Compressor reports the block's peak of what its detector measured, the external key when one is given, after the sidechain filter), stored in the insert's scope tap beside the gain reduction and read through `ScopeHub::detector_level_db`; a tap switched on starts with no level, so the dot stays hidden until the render publishes one. The dot steps with the meter's peak fall-off ballistics. The history is a ring per instance in the editor's state (`compressor_panel::history::History`, one sample a frame while the instance has a scope tap, at most 1024 samples), the output its newest tap peak. The time labels are inset 12 across and 10 up from the well's inside edge, and the legend is right-aligned to the column with 4 between a swatch and its label and 10 between items, both as the board places them. Records: `HeroLayout::unity` (`UnityRecord`), `HeroLayout::dot` (`DotRecord`) and `HeroLayout::history` (`HistoryRecord`); the color names and labels are mirrored in `tokens::compressor::STRINGS`.

### Sampler views

The Instrument and Slices views (ADR 0041), drawn at the built panel's design width, 976, with the same title bar, sections row and footer as the Pads view. Mockups: `chrome/SamplerInstrument.dc.html`, `chrome/SamplerSlices.dc.html` (the chrome canvas, Sampler views page).

- **View bar** (every view, the Pads view included): a 24 px segmented control `Instrument · Pads · Slices` under the title bar, Sans 500 12; beside it a one-line summary in `readout.display` (zones, groups, layers and range; the loop's file, length, rate and cue points); right-aligned, the view's actions as standard buttons (`Add zone…`, `Import SFZ…`; `Load loop…`). Switching views changes no routing, parameters or notes.
- **Footer** (every view): Voices, Memory, Streaming in `readout.display` with their values in `text`, Audio load at the right.
- **Shared sections:** Playback and Amp envelope are the Pads view's sections and edit the selected zone (Instrument), the selected slice's pad zone (Slices) or the selected pad (Pads). Instrument adds the per-zone Shaper and Modulation as section bars (collapsed in the board; their open state is a later board).

**Instrument view**

- **Key map** (hero): a display well over a keyboard, with the velocity axis at its left (`127`, `64`, `1` in Mono 10 `text.faint`, a `grid` line at 64; octave lines in `grid`). A zone is a rectangle over the keys and velocities it covers: `surface.raised`, 1 px `line.strong`, radius 4, its root key and layer in Mono 500 9 top-left (`E2 ff`) and its root as a 7 px dot on its bottom edge (`text.faint`; `text` when selected). Selected: 2 px `signal` edge over `signal` at 14 percent. A round-robin group carries an `RR n` tag top-right (Mono 500 8, 1 px `line.strong`). Pad-bound zones show too, as one-key, full-velocity zones with a dashed `ink.arrow` edge and the pad id as label, because the view edits every zone. Held keys light `signal` on the keyboard and as a `signal` 10 percent column over the zones. Drag a zone's side edges for the key range, its top edge for the velocity split, its dot for the root.
- **Keyboard:** white keys `text.muted` (keys bound to pads `text.faint`) with octave names in Mono 500 8 `ink.dark`; black keys `bg.titlebar`; both 1 px edged.
- **Zone panel:** the zone's name, file, rate and length; its keys, velocities, root and fades in Mono; the zone waveform (below); Loop as a segmented `Off · Forward · Ping-pong` and Crossfade as a value field marked "forward loops only", dimmed under Ping-pong (ADR 0041 refuses ping-pong with a crossfade).
- **Zone waveform** (the hero waveform with start, end and loop handles): a display well. The region between start and end is `signal` at 55 percent; outside it `ink.ghost`. Start and end are 2 px `signal` lines with `S` and `E` flag tabs (Sans 600 9, `surface.inset` letters) at the foot, dragged to move. The loop takes the `loop` hue: a 14 px brace across the top with its mode and crossfade in Mono 500 9 `ink.dark` (`loop · fwd · xf 12 ms`), a 10 percent band down the well, and the crossfade as a `loop`-hue hatch at the loop's end. A sounding voice's playhead is a 1 px `text` line.
- **Group panel:** the selected zone's group (`D#2–F2 · 2 of 8 layers`): one row per velocity layer with its range, its round-robin alternatives as 22 px chips (the selected one with a 2 px `signal` edge) and a dashed add chip; Round robin as a segmented `Sequential · Random` with the seed beside it (`text.faint`, `—` while sequential); velocity sensitivity as a horizontal fader (default 70 percent). Making a pad's zone a group unbinds it from its pad, said in the panel.

**Slices view**

- **Bench** (hero): the `SLICES` label, the mode as a segmented `Auto · Manual`, the count (`8 of 64 slices`), then Zoom as a segmented `1× 2× 4× 8×` and `SNAP 0×` as an option toggle (on by default; unavailable in Auto, where it shapes nothing). Below, a display well with an 18 px flag strip (`bg.titlebar`, 1 px `line.inset` under it). Each slice start is a 1 px line with a numbered flag (Mono 500 9, radius 0 3 3 0); the first reads `S 1` and stays at the loop start; `E` is the end boundary, a 2 px `ink.arrow` line with its own flag, not a slice. Alternate slices take `text` at 3 percent so they read as cells; the selected slice is `signal` at 12 percent with its waveform in `signal` at 60 percent; the rest of the waveform is `text.faint` at 45 percent (source audio is content, not a value). A time ruler in Mono 10 `text.faint` sits under the well; past 1× the chrome's horizontal scroll bar runs inside the well's foot and the zoom keeps the selected slice centred. Under it, the mode's hint and the selected slice (`Slice 5 · 1.380–1.725 s · pad B5 · E3`).
- **Auto markers** are read-only and quiet: `ink.arrow` lines, flags `surface.panel` with `text.faint` numbers. **Manual markers** are `text.muted` lines, flags `surface.selected` with `text` numbers, and a 13 px hollow `text` handle at the foot. The gestures (ADR 0041), each one undo step:
  - **Add:** click the bench. A dashed `text` ghost line with its snapped time on a `text` flag (`+ 0.869 s`) follows the pointer; with snap on, a short `text.faint` tick marks the raw pointer when the snap moved it. Where an add would be refused the ghost turns `meter.hot` and says `too close`.
  - **Move:** drag a handle. While held the line and handle are `text` and `signal`, the time shows in a tooltip (`1.412 s · snapped`), and the marker stops short of its neighbours.
  - **Remove:** click a flag; the slice merges into the one before. The first marker refuses with a message, since it is the loop start.
  - **Select:** click a slice's cell in the flag strip (between two flags).
  - Any gesture in Auto first copies Auto's set into Manual and switches there. The Manual panel lists the four gestures in Mono 11 and, under them, the last edit or a refusal's error banner.
- **Detect panel** (Auto): Source as a segmented `Cue points · Transients · Grid`; Sensitivity as a horizontal fader with its value (0 to 1); Grid as the division segmented `1/4 1/8 1/16 1/32` with Bars as a dropdown (`From tempo`, 1–16) and the derived BPM printed after it, dimmed to 40 percent unless the source is Grid. A changed result is a candidate (`Candidate: 9 slices (showing 8)`) until **Replace markers** accepts it.
- **Manual panel** (Manual): the gesture rules in a sentence, **Start from Auto's markers** and **Clear**. A refused edit (too close, over 64 slices, missing data) is an error banner in this panel (Warnings and errors), never a silent move.
- **Per-slice loop:** the Slices view's Playback section adds Loop for the selected slice, a segmented `Off · Loop · Ping-pong` (the chosen loop filled `loop` with an `ink.dark` label) with the crossfade as a value field beside it, forward loops only (`xf —` in `ink.arrow` otherwise). The loop spans the whole slice; loop points inside a slice are the Instrument view's zone waveform. A looping slice plays as Gate (loops while held, then releases): choosing a loop sets Gate, and One-shot ghosts to `ink.arrow` with its reason as a tooltip, since a one-shot cannot loop (ADR 0041); pressing it refuses with an error banner. On the bench a looping slice carries a 4 px `loop` band right under the flag strip across its region, a `loop` wash at 7 percent, `↻` (forward) or `⇄` (ping-pong) after its flag number, and its crossfade as a `loop`-hue hatch at its end.
- **Use slices panel:** both actions consume the set showing (`the Auto set, as shown`). Root as a dropdown (`C3 · 60`, slice i to key root+i). **Slices to pads** with a 16-cell preview of the bank it fills (12 px cells, filled `surface.raised`, the selected slice's cell `signal`-edged) and what it replaces. **Slice to MIDI** with the timing as a segmented `Fit length · Keep duration` and the beat length as a value field; a line says where the clip lands and that an overlap becomes a take (ADR 0039).

**Round 2 decisions** (answers to `tmp/design-requests/sampler-gaps-round-2.md`; drawn on the chrome canvas, "Sampler — round 2 answers"):

- **Double-click (bench):** the bench acts on single clicks only and swallows the second click of a double-click, so a double-click never adds two markers or adds and then removes one. A marker has no default, so nothing resets: empty bench, one add; a flag or marker, one remove; S, nothing; a handle, nothing. Double-clicking a slice's cell in the flag strip renames the slice in place (as the Selected pad name does).
- **Spacing:** no standing readout. The detector's 30 ms gap is in the Sensitivity tooltip (`Detections keep at least 30 ms apart`); a Manual drag that reaches a neighbour clamps there and its tooltip says so (`0.871 s · clamped at marker 4`); the ghost's `too close` stays.
- **Pending detection:** while a candidate differs from the current Auto set, its markers preview on the bench as dashed `text.faint` lines without flags (Auto only); the current set stays solid and in use. The Detect panel reads `Candidate 8 → 9`, **Replace markers** takes the default-button style and **Keep current** (standard) discards the candidate.
- **The Auto-to-Manual copy:** a Notice in the Manual panel (`Copied Auto's 8 markers to Manual, then added marker 4.` with **Undo**). Undo is the ordinary single step (Ctrl+Z does the same) and returns to Auto with Manual as it was before.
- **No cell strip in 0.7.0:** the flag strip selects and the selection line names id, time, pad and key. Moving a slice to another pad is the Pads view's drag (ADR 0041), not a Slices gesture; the Use slices pad preview shows the mapping.
- **Slice to MIDI** is an inline panel: the timing segmented control with a one-line help for each, the beat length (`8.00 beats`), the placement (`at 5.1.1 · selection start · active playlist`), the summary (`8 notes · C3 to G3 · velocity 100 · channel 1`) and **Create clip** as the default button. A stale mapping is an error banner in the panel (`The slices' keys no longer run in order: slices 3 and 5 swapped pads.`) with **Map slices to keys** beside it; Create clip stays clickable and repeats the reason. An overlap is a warning line (triangle, no colour) saying the result becomes a new take. Two slices on one tick, a frozen track and missing loop metadata refuse the same way.
- **Crossfade unit:** milliseconds with one decimal, converted at the session rate, the frame count in the tooltip. The stored value is kept; when a slice edit leaves it too long, the field still shows it and a warning line says what is used (`Slice 7 is 9.4 ms long now: 4.7 ms of crossfade is used.`).
- **Keyboard** (bench focused): ← → previous and next slice; Alt+← → nudge the selected slice's start marker one frame (Manual), Shift+Alt ten; Delete removes it (never S); Space or Enter auditions the selected slice; Escape cancels a drag. The same nudges move the zone waveform's handles. Each is in its control's tooltip and in Help › Keyboard Shortcuts under a Sampler scope.
- **Success:** the last-edit line with a check (Warnings and errors), for Replace markers, Create clip and every Manual edit.
- **Edge fades (key map):** drawn, not dragged, in 0.7.0: the zone's fill runs from full to 25 percent across a fade's keys or velocities (a wedge inside the edge). The four values (key low, key high, velocity low, velocity high) are fields in the Zone panel; `fades none` when all are zero.
- **No zone table in 0.7.0:** the key map shows every zone and the Zone and Group panels edit one. A table returns as a collapsible section when SFZ import (#271) makes large maps common.
- **Pad-bound zones:** their edges do not drag; a 10 px Lucide `lock` in `text.faint` sits by the label and the tooltip says why. Adding a layer or an alternative to one (the Group panel's add chip, or Alt-drag on the map) asks inline in the Group panel with a Notice (`Pad A3's zone becomes a group and leaves pad A3, which will be empty. Its notes still play the group.`), **Cancel** and **Make group** (default). Nothing happens until Make group.
- **Round-robin seed:** editable while Random, an integer field (1 to 4 294 967 295; 0 becomes 1, as ADR 0041 normalises) with a reroll icon button (Lucide `dices`); while Sequential it shows `—` in `ink.arrow`, disabled, tooltip `Random only`, and the stored seed is kept.
- **States:** *empty*, the hero well is a dashed `line.strong` drop target (`+ Drop a loop` / `+ Drop a sample`) with Load…, controls ghosted, sections at 40 percent; *loading*, the waveform draws as it decodes and an `Analyzing…` tag sits in the well corner while everything stays editable; *refused file*, the error in the note row, the previous source kept; *missing media*, the well hatched with a card naming the file and Relink…, ids and round-robin places kept; *stale mapping*, a `keys out of order` tag on the bench and the refusal in Slice to MIDI; *legacy one-sample patch*, a Notice with **New Kit** in every view, never converted on open; *conflict while dragging*, the dragged zone outlined dashed in `meter.hot` before release when dropping would break a bound or overwrite, then a snap back and a refusal; *over capacity*, the count reads `64 of 64 slices` and the add refuses in the error banner (Auto over 64, 256 zones and the per-group bounds likewise).
- **Pads view bar:** `Bank A · 12 of 16 loaded · 3 choke groups` (the selected bank; groups across banks), with **Load kit…** (a folder, ADR 0041) and **Expand to rack…** (its preview is a later board).
- **View switch face:** the Segmented control's view-switch variant (Sans 500 12).

**Two choices that differ from the brief** (`tmp/design-requests/sampler-brief.md`), kept deliberately: sampler loops (the zone waveform's loop and the per-slice loop) use the `loop` hue, because the hue means time structure (Colour), not the arrangement alone, so a loop reads the same everywhere; and zone bodies on the key map are `surface.raised`, not the track's palette body, so the selected zone's `signal` and a conflict's `meter.hot` stay the only colours on the map (confirmed by the maintainer, 2026-11-08; a per-zone colour picker may come later, and until then zones stay neutral).

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
