pub const VIEW_BAR_FONT: &str = "sans";
pub const VIEW_BAR_VIEWS: [&str; 3] = ["Instrument", "Pads", "Slices"];
pub const ZONE_FILL: &str = "surface.raised";
pub const ZONE_BORDER: &str = "line.strong";
pub const SELECTED_BORDER: &str = "signal";
pub const PAD_ZONE_BORDER: &str = "ink.arrow";
pub const PAD_ZONE_STYLE: &str = "dashed";
pub const WHITE_KEY: &str = "text.muted";
pub const PAD_KEY: &str = "text.faint";
pub const BLACK_KEY: &str = "bg.titlebar";
pub const WAVE_REGION: &str = "signal";
pub const WAVE_OUTSIDE: &str = "ink.ghost";
pub const WAVE_HANDLE: &str = "signal";
pub const WAVE_PLAYHEAD: &str = "text";
pub const BENCH_FLAG_FONT: &str = "mono";
pub const BENCH_WAVE: &str = "text.faint";
pub const BENCH_END: &str = "ink.arrow";
pub const BENCH_AUTO_LINE: &str = "ink.arrow";
pub const BENCH_MANUAL_LINE: &str = "text.muted";
pub const BENCH_GHOST_LINE: &str = "text";
pub const BENCH_GHOST_STYLE: &str = "dashed";
pub const BENCH_GHOST_REFUSED: &str = "meter.hot";
pub const SLICE_LOOP_COLOR: &str = "loop";
pub const SLICE_LOOP_GLYPH_FORWARD: &str = "\u{21bb}";
pub const SLICE_LOOP_GLYPH_PING_PONG: &str = "\u{21c4}";
/// Round 2 (ADR 0041): the flag strip's line, the type roles, the candidate line, the lock
/// glyph, the seed field and the view switch.
pub const BENCH_FLAG_STRIP_LINE: &str = "line.inset";
/// The Sampler's type roles: (name, family, weight, size).
pub const TYPE: [(&str, &str, f32, f32); 6] = [
    ("zoneLabel", "mono", 500.0, 9.0),
    ("rrTag", "mono", 500.0, 8.0),
    ("octaveName", "mono", 500.0, 8.0),
    ("axis", "mono", 400.0, 10.0),
    ("flagLetter", "sans", 600.0, 9.0),
    ("braceLabel", "mono", 500.0, 9.0),
];
pub const CANDIDATE_LINE: &str = "text.faint";
pub const CANDIDATE_FLAG: bool = false;
pub const LOCK_GLYPH_ICON: &str = "lock";
pub const LOCK_GLYPH_COLOR: &str = "text.faint";
pub const SEED_REROLL_ICON: &str = "dices";
pub const SEED_SEQUENTIAL: &str = "ink.arrow";
pub const VIEW_SWITCH_FONT: &str = "sans";
/// The Slices view's bench: (fill, ink, edge) of an Auto, a Manual and the selected slice's flag,
/// the selected marker's line, (fill, ink) of the end boundary's flag, the Manual handle's line
/// and fill, and the waveform's midline.
pub const BENCH_AUTO_FLAG: [&str; 3] = ["surface.panel", "text.faint", "line"];
pub const BENCH_MANUAL_FLAG: [&str; 3] = ["surface.selected", "text", "line.strong"];
pub const BENCH_SELECTED_FLAG: [&str; 3] = ["signal", "surface.inset", "signal"];
pub const BENCH_SELECTED_LINE: &str = "signal";
pub const BENCH_END_FLAG: [&str; 2] = ["surface.selected", "text.muted"];
pub const BENCH_HANDLE_LINE: &str = "text";
pub const BENCH_HANDLE_FILL: &str = "surface.inset";
pub const BENCH_MIDLINE: &str = "line.inset";
/// A well's status tag (`Analyzing…`, `keys out of order`): its font, ink and edge.
pub const TAG_FONT: &str = "mono";
pub const TAG_INK: &str = "text.muted";
pub const TAG_EDGE: &str = "line.strong";
/// The Pads bar's Start note field (`startNote`; `sampler_panel::bar` draws it).
pub const START_NOTE_LABEL: &str = "Start note";
pub const START_NOTE_FONT: &str = "mono";
pub const START_NOTE_DEFAULT: &str = "C1";
pub const START_NOTE_LABEL_GAP: f32 = 6.0;
pub const START_NOTE_WIDTH: f32 = 48.0;
pub const START_NOTE_HEIGHT: f32 = 24.0;
pub const START_NOTE_FONT_WEIGHT: f32 = 500.0;
pub const START_NOTE_FONT_SIZE: f32 = 11.0;
/// The start notes the field shows (`startNote.range`, MIDI numbers, inclusive): display only.
/// The model's `MAX_PAD_START` is the authority for behavior; a test keeps the two equal.
pub const START_NOTE_RANGE: [u8; 2] = [0, 64];
pub const START_NOTE_DRAG_PX_PER_SEMITONE: f32 = 6.0;
/// Every string under `sampler` in tokens.json by its path.
pub const STRINGS: &[(&str, &str)] = &[
    ("viewBar/font/font", VIEW_BAR_FONT),
    ("viewBar/views/0", VIEW_BAR_VIEWS[0]),
    ("viewBar/views/1", VIEW_BAR_VIEWS[1]),
    ("viewBar/views/2", VIEW_BAR_VIEWS[2]),
    ("keyMap/zoneFill", ZONE_FILL),
    ("keyMap/zoneBorder", ZONE_BORDER),
    ("keyMap/selectedBorder", SELECTED_BORDER),
    ("keyMap/padZoneBorder", PAD_ZONE_BORDER),
    ("keyMap/padZoneStyle", PAD_ZONE_STYLE),
    ("keyMap/whiteKey", WHITE_KEY),
    ("keyMap/padKey", PAD_KEY),
    ("keyMap/blackKey", BLACK_KEY),
    ("waveform/region", WAVE_REGION),
    ("waveform/outside", WAVE_OUTSIDE),
    ("waveform/handle", WAVE_HANDLE),
    ("waveform/playhead", WAVE_PLAYHEAD),
    ("bench/flagFont/font", BENCH_FLAG_FONT),
    ("bench/wave", BENCH_WAVE),
    ("bench/end", BENCH_END),
    ("bench/autoLine", BENCH_AUTO_LINE),
    ("bench/manualLine", BENCH_MANUAL_LINE),
    ("bench/ghost/line", BENCH_GHOST_LINE),
    ("bench/ghost/style", BENCH_GHOST_STYLE),
    ("bench/ghost/refused", BENCH_GHOST_REFUSED),
    ("bench/sliceLoop/color", SLICE_LOOP_COLOR),
    ("bench/sliceLoop/glyphForward", SLICE_LOOP_GLYPH_FORWARD),
    ("bench/sliceLoop/glyphPingPong", SLICE_LOOP_GLYPH_PING_PONG),
    ("bench/flagStripLine", BENCH_FLAG_STRIP_LINE),
    ("bench/autoFlag/fill", BENCH_AUTO_FLAG[0]),
    ("bench/autoFlag/ink", BENCH_AUTO_FLAG[1]),
    ("bench/autoFlag/edge", BENCH_AUTO_FLAG[2]),
    ("bench/manualFlag/fill", BENCH_MANUAL_FLAG[0]),
    ("bench/manualFlag/ink", BENCH_MANUAL_FLAG[1]),
    ("bench/manualFlag/edge", BENCH_MANUAL_FLAG[2]),
    ("bench/selectedFlag/fill", BENCH_SELECTED_FLAG[0]),
    ("bench/selectedFlag/ink", BENCH_SELECTED_FLAG[1]),
    ("bench/selectedFlag/edge", BENCH_SELECTED_FLAG[2]),
    ("bench/selectedLine", BENCH_SELECTED_LINE),
    ("bench/endFlag/fill", BENCH_END_FLAG[0]),
    ("bench/endFlag/ink", BENCH_END_FLAG[1]),
    ("bench/handleLine", BENCH_HANDLE_LINE),
    ("bench/handleFill", BENCH_HANDLE_FILL),
    ("bench/midline", BENCH_MIDLINE),
    ("tag/font/font", TAG_FONT),
    ("tag/ink", TAG_INK),
    ("tag/edge", TAG_EDGE),
    ("type/zoneLabel/font", TYPE[0].1),
    ("type/rrTag/font", TYPE[1].1),
    ("type/octaveName/font", TYPE[2].1),
    ("type/axis/font", TYPE[3].1),
    ("type/flagLetter/font", TYPE[4].1),
    ("type/braceLabel/font", TYPE[5].1),
    ("candidate/line", CANDIDATE_LINE),
    ("lockGlyph/icon", LOCK_GLYPH_ICON),
    ("lockGlyph/color", LOCK_GLYPH_COLOR),
    ("seed/rerollIcon", SEED_REROLL_ICON),
    ("seed/sequential", SEED_SEQUENTIAL),
    ("viewSwitch/font/font", VIEW_SWITCH_FONT),
    ("startNote/label", START_NOTE_LABEL),
    ("startNote/font/font", START_NOTE_FONT),
    ("startNote/default", START_NOTE_DEFAULT),
];
/// The view bar's row; its controls take the dense `VIEW_BAR_CONTROL_HEIGHT`, centred, and its
/// items (a label and its field are one) stand `VIEW_BAR_GAP` apart.
pub const VIEW_BAR_HEIGHT: f32 = 28.0;
pub const VIEW_BAR_CONTROL_HEIGHT: f32 = 24.0;
pub const VIEW_BAR_GAP: f32 = 12.0;
pub const VIEW_BAR_FONT_WEIGHT: f32 = 500.0;
pub const VIEW_BAR_FONT_SIZE: f32 = 12.0;
pub const ZONE_RADIUS: f32 = 4.0;
pub const SELECTED_BORDER_WIDTH: f32 = 2.0;
pub const SELECTED_FILL_ALPHA: f32 = 0.14;
pub const ROOT_DOT: f32 = 7.0;
pub const HELD_COLUMN_ALPHA: f32 = 0.1;
pub const WAVE_REGION_ALPHA: f32 = 0.55;
pub const WAVE_HANDLE_WIDTH: f32 = 2.0;
pub const WAVE_FLAG_HEIGHT: f32 = 14.0;
pub const WAVE_LOOP_BRACE: f32 = 14.0;
pub const WAVE_LOOP_BAND_ALPHA: f32 = 0.1;
pub const WAVE_CROSSFADE_HATCH_ALPHA: f32 = 0.28;
pub const BENCH_FLAG_STRIP: f32 = 18.0;
pub const BENCH_FLAG_FONT_WEIGHT: f32 = 500.0;
pub const BENCH_FLAG_FONT_SIZE: f32 = 9.0;
pub const BENCH_ALT_SLICE_ALPHA: f32 = 0.03;
pub const BENCH_SELECTED_ALPHA: f32 = 0.12;
pub const BENCH_SELECTED_WAVE_ALPHA: f32 = 0.6;
pub const BENCH_WAVE_ALPHA: f32 = 0.45;
pub const BENCH_HANDLE: f32 = 13.0;
pub const SLICE_LOOP_BAND: f32 = 4.0;
pub const SLICE_LOOP_WASH_ALPHA: f32 = 0.07;
pub const SLICE_LOOP_HATCH_ALPHA: f32 = 0.28;
pub const BENCH_ZOOMS: [f32; 4] = [1.0, 2.0, 4.0, 8.0];
/// The bench well's height, its flags (height, top inset, minimum width, padding a side, edge),
/// the end line's width and its flag's radii, the Manual handle's inset from the well's foot and
/// its stroke, and the time ruler (row height; tick step in seconds, the finer step from zoom 4).
pub const BENCH_WELL_HEIGHT: f32 = 188.0;
pub const BENCH_FLAG_HEIGHT: f32 = 16.0;
pub const BENCH_FLAG_INSET: f32 = 1.0;
pub const BENCH_FLAG_MIN_WIDTH: f32 = 18.0;
pub const BENCH_FLAG_PADDING_X: f32 = 4.0;
pub const BENCH_FLAG_EDGE_WIDTH: f32 = 1.0;
pub const BENCH_END_WIDTH: f32 = 2.0;
pub const BENCH_END_FLAG_RADIUS: [f32; 4] = [3.0, 0.0, 0.0, 3.0];
pub const BENCH_HANDLE_INSET: f32 = 3.0;
pub const BENCH_HANDLE_STROKE: f32 = 1.5;
pub const BENCH_RULER_HEIGHT: f32 = 12.0;
pub const BENCH_RULER_STEP: f32 = 0.5;
pub const BENCH_RULER_FINE_STEP: f32 = 0.1;
pub const BENCH_RULER_FINE_FROM_ZOOM: f32 = 4.0;
/// The status tag: its inset from the well's corner, font weight and size, radius and padding a side.
pub const TAG_INSET: f32 = 6.0;
pub const TAG_FONT_WEIGHT: f32 = 500.0;
pub const TAG_FONT_SIZE: f32 = 10.0;
pub const TAG_RADIUS: f32 = 3.0;
pub const TAG_PADDING_X: f32 = 4.0;
/// The flag's corner radii (top left, top right, bottom right, bottom left).
pub const BENCH_FLAG_RADIUS: [f32; 4] = [0.0, 3.0, 3.0, 0.0];
pub const CHIP: f32 = 22.0;
pub const PREVIEW_CELL: f32 = 12.0;
pub const UNUSED_CONTROL_ALPHA: f32 = 0.4;
pub const CANDIDATE_DASH: [f32; 2] = [3.0, 3.0];
pub const FADE_FLOOR_ALPHA: f32 = 0.25;
pub const LOCK_GLYPH_SIZE: f32 = 10.0;
/// The seed field's range (ADR 0041: 1 to 4 294 967 295, 0 normalized to 1).
pub const SEED_MIN: u32 = 1;
pub const SEED_MAX: u32 = u32::MAX;
pub const VIEW_SWITCH_FONT_WEIGHT: f32 = 500.0;
pub const VIEW_SWITCH_FONT_SIZE: f32 = 12.0;
pub const VIEW_SWITCH_PADDING_X: f32 = 12.0;
/// The Slices view's Detect, Manual and Use slices panels (the board `SamplerSlices`; the beat
/// field's width from `SamplerRound2`): the gap between rows and between a row's items, the left
/// panel's least height, the label columns, the action buttons, the Root and beat fields, the
/// Sensitivity fader's travel and the pad preview's grid.
pub const PANEL_ROW_GAP: f32 = 10.0;
pub const PANEL_ITEM_GAP: f32 = 10.0;
pub const PANEL_MIN_HEIGHT: f32 = 190.0;
pub const DETECT_LABEL_WIDTH: f32 = 70.0;
pub const USE_LABEL_WIDTH: f32 = 62.0;
pub const PANEL_ACTION_WIDTH: f32 = 112.0;
pub const ROOT_WIDTH: f32 = 92.0;
pub const BEAT_WIDTH: f32 = 84.0;
pub const FADER_LENGTH: f32 = 150.0;
pub const PREVIEW_COLUMNS: f32 = 8.0;
pub const PREVIEW_GAP: f32 = 2.0;
pub const PREVIEW_RADIUS: f32 = 2.0;
pub const METRICS: &[(&str, f32)] = &[
    ("viewBar/height", VIEW_BAR_HEIGHT),
    ("viewBar/controlHeight", VIEW_BAR_CONTROL_HEIGHT),
    ("viewBar/gap", VIEW_BAR_GAP),
    ("viewBar/font/weight", VIEW_BAR_FONT_WEIGHT),
    ("viewBar/font/size", VIEW_BAR_FONT_SIZE),
    ("keyMap/zoneRadius", ZONE_RADIUS),
    ("keyMap/selectedBorderWidth", SELECTED_BORDER_WIDTH),
    ("keyMap/selectedFillAlpha", SELECTED_FILL_ALPHA),
    ("keyMap/rootDot", ROOT_DOT),
    ("keyMap/heldColumnAlpha", HELD_COLUMN_ALPHA),
    ("waveform/regionAlpha", WAVE_REGION_ALPHA),
    ("waveform/handleWidth", WAVE_HANDLE_WIDTH),
    ("waveform/flagHeight", WAVE_FLAG_HEIGHT),
    ("waveform/loopBrace", WAVE_LOOP_BRACE),
    ("waveform/loopBandAlpha", WAVE_LOOP_BAND_ALPHA),
    ("waveform/crossfadeHatchAlpha", WAVE_CROSSFADE_HATCH_ALPHA),
    ("bench/flagStrip", BENCH_FLAG_STRIP),
    ("bench/flagFont/weight", BENCH_FLAG_FONT_WEIGHT),
    ("bench/flagFont/size", BENCH_FLAG_FONT_SIZE),
    ("bench/altSliceAlpha", BENCH_ALT_SLICE_ALPHA),
    ("bench/selectedAlpha", BENCH_SELECTED_ALPHA),
    ("bench/selectedWaveAlpha", BENCH_SELECTED_WAVE_ALPHA),
    ("bench/waveAlpha", BENCH_WAVE_ALPHA),
    ("bench/handle", BENCH_HANDLE),
    ("bench/sliceLoop/band", SLICE_LOOP_BAND),
    ("bench/sliceLoop/washAlpha", SLICE_LOOP_WASH_ALPHA),
    ("bench/sliceLoop/crossfadeHatchAlpha", SLICE_LOOP_HATCH_ALPHA),
    ("bench/zooms/0", BENCH_ZOOMS[0]),
    ("bench/zooms/1", BENCH_ZOOMS[1]),
    ("bench/zooms/2", BENCH_ZOOMS[2]),
    ("bench/zooms/3", BENCH_ZOOMS[3]),
    ("bench/wellHeight", BENCH_WELL_HEIGHT),
    ("bench/flagHeight", BENCH_FLAG_HEIGHT),
    ("bench/flagInset", BENCH_FLAG_INSET),
    ("bench/flagMinWidth", BENCH_FLAG_MIN_WIDTH),
    ("bench/flagPaddingX", BENCH_FLAG_PADDING_X),
    ("bench/flagEdgeWidth", BENCH_FLAG_EDGE_WIDTH),
    ("bench/endWidth", BENCH_END_WIDTH),
    ("bench/endFlag/radius/0", BENCH_END_FLAG_RADIUS[0]),
    ("bench/endFlag/radius/1", BENCH_END_FLAG_RADIUS[1]),
    ("bench/endFlag/radius/2", BENCH_END_FLAG_RADIUS[2]),
    ("bench/endFlag/radius/3", BENCH_END_FLAG_RADIUS[3]),
    ("bench/handleInset", BENCH_HANDLE_INSET),
    ("bench/handleStroke", BENCH_HANDLE_STROKE),
    ("bench/ruler/height", BENCH_RULER_HEIGHT),
    ("bench/ruler/step", BENCH_RULER_STEP),
    ("bench/ruler/fineStep", BENCH_RULER_FINE_STEP),
    ("bench/ruler/fineFromZoom", BENCH_RULER_FINE_FROM_ZOOM),
    ("tag/inset", TAG_INSET),
    ("tag/font/weight", TAG_FONT_WEIGHT),
    ("tag/font/size", TAG_FONT_SIZE),
    ("tag/radius", TAG_RADIUS),
    ("tag/paddingX", TAG_PADDING_X),
    ("bench/flagRadius/0", BENCH_FLAG_RADIUS[0]),
    ("bench/flagRadius/1", BENCH_FLAG_RADIUS[1]),
    ("bench/flagRadius/2", BENCH_FLAG_RADIUS[2]),
    ("bench/flagRadius/3", BENCH_FLAG_RADIUS[3]),
    ("type/zoneLabel/weight", TYPE[0].2),
    ("type/zoneLabel/size", TYPE[0].3),
    ("type/rrTag/weight", TYPE[1].2),
    ("type/rrTag/size", TYPE[1].3),
    ("type/octaveName/weight", TYPE[2].2),
    ("type/octaveName/size", TYPE[2].3),
    ("type/axis/weight", TYPE[3].2),
    ("type/axis/size", TYPE[3].3),
    ("type/flagLetter/weight", TYPE[4].2),
    ("type/flagLetter/size", TYPE[4].3),
    ("type/braceLabel/weight", TYPE[5].2),
    ("type/braceLabel/size", TYPE[5].3),
    ("chip", CHIP),
    ("previewCell", PREVIEW_CELL),
    ("unusedControlAlpha", UNUSED_CONTROL_ALPHA),
    ("candidate/dash/0", CANDIDATE_DASH[0]),
    ("candidate/dash/1", CANDIDATE_DASH[1]),
    ("fadeFloorAlpha", FADE_FLOOR_ALPHA),
    ("lockGlyph/size", LOCK_GLYPH_SIZE),
    ("seed/min", SEED_MIN as f32),
    ("seed/max", SEED_MAX as f32),
    ("viewSwitch/font/weight", VIEW_SWITCH_FONT_WEIGHT),
    ("viewSwitch/font/size", VIEW_SWITCH_FONT_SIZE),
    ("viewSwitch/paddingX", VIEW_SWITCH_PADDING_X),
    ("startNote/labelGap", START_NOTE_LABEL_GAP),
    ("startNote/width", START_NOTE_WIDTH),
    ("startNote/height", START_NOTE_HEIGHT),
    ("startNote/font/weight", START_NOTE_FONT_WEIGHT),
    ("startNote/font/size", START_NOTE_FONT_SIZE),
    ("startNote/dragPxPerSemitone", START_NOTE_DRAG_PX_PER_SEMITONE),
    ("startNote/range/0", START_NOTE_RANGE[0] as f32),
    ("startNote/range/1", START_NOTE_RANGE[1] as f32),
    ("slicePanels/rowGap", PANEL_ROW_GAP),
    ("slicePanels/itemGap", PANEL_ITEM_GAP),
    ("slicePanels/minHeight", PANEL_MIN_HEIGHT),
    ("slicePanels/detectLabelWidth", DETECT_LABEL_WIDTH),
    ("slicePanels/useLabelWidth", USE_LABEL_WIDTH),
    ("slicePanels/actionWidth", PANEL_ACTION_WIDTH),
    ("slicePanels/rootWidth", ROOT_WIDTH),
    ("slicePanels/beatWidth", BEAT_WIDTH),
    ("slicePanels/faderLength", FADER_LENGTH),
    ("slicePanels/previewColumns", PREVIEW_COLUMNS),
    ("slicePanels/previewGap", PREVIEW_GAP),
    ("slicePanels/previewRadius", PREVIEW_RADIUS),
];
/// The Instrument view (`instrument`; the boards `SamplerInstrument` and `SamplerRound2`). The key
/// map: the zone well's height; the velocity axis's width, its gap to the well and its labels'
/// inset from its right; the keyboard's gap under the well and its height; a black key's height and
/// its width as a share of a white key's (centered on the white keys' boundary); the keys' bottom
/// radii; how far a zone sits inside the keys and velocities it covers; the zone label's inset
/// (left, top); the root dot's inset from the zone's foot; the `RR n` tag's inset, padding and
/// radius; the conflict outline's width.
pub const KEY_MAP_WELL_HEIGHT: f32 = 196.0;
pub const AXIS_WIDTH: f32 = 26.0;
pub const AXIS_GAP: f32 = 6.0;
pub const AXIS_INSET: f32 = 2.0;
pub const KEYBOARD_GAP: f32 = 4.0;
pub const KEYBOARD_HEIGHT: f32 = 52.0;
pub const BLACK_KEY_HEIGHT: f32 = 32.0;
pub const BLACK_KEY_WIDTH: f32 = 0.6;
pub const WHITE_KEY_RADIUS: f32 = 3.0;
pub const BLACK_KEY_RADIUS: f32 = 2.0;
pub const OCTAVE_NAME_INSET: f32 = 3.0;
pub const ZONE_INSET: f32 = 1.0;
pub const ZONE_LABEL_INSET: [f32; 2] = [4.0, 3.0];
pub const ROOT_DOT_INSET: f32 = 4.0;
pub const RR_TAG_INSET: f32 = 3.0;
pub const RR_TAG_PADDING_X: f32 = 2.0;
pub const RR_TAG_RADIUS: f32 = 3.0;
pub const CONFLICT_WIDTH: f32 = 2.0;
/// The key map's colors: the keys' edges, the octave names, a zone's label (and selected), its
/// root dot (and selected), the `RR n` tag's edge and ink, a pad zone's fill, label and dot, the
/// conflict outline.
pub const KEY_EDGE: &str = "line.strong";
pub const BLACK_KEY_EDGE: &str = "surface.inset";
pub const OCTAVE_NAME_INK: &str = "ink.dark";
pub const ZONE_INK: &str = "text.muted";
pub const SELECTED_INK: &str = "text";
pub const ROOT_DOT_INK: &str = "text.faint";
pub const SELECTED_ROOT_DOT_INK: &str = "text";
pub const RR_TAG_EDGE: &str = "line.strong";
pub const RR_TAG_INK: &str = "text.muted";
pub const PAD_ZONE_FILL: &str = "surface.panel";
pub const PAD_ZONE_INK: &str = "text.faint";
pub const PAD_ZONE_DOT: &str = "ink.arrow";
pub const CONFLICT_EDGE: &str = "meter.hot";
/// The zone waveform: the well's height, the S and E flags' padding a side, inner corner radius and
/// letters, the loop brace's radius and label ink, the crossfade hatch's stroke and spacing, and the
/// region readout's top (its right is the status tag's inset).
pub const ZONE_WAVE_HEIGHT: f32 = 108.0;
pub const ZONE_FLAG_PADDING_X: f32 = 4.0;
pub const ZONE_FLAG_RADIUS: f32 = 3.0;
pub const ZONE_FLAG_INK: &str = "surface.inset";
pub const BRACE_RADIUS: f32 = 2.0;
pub const BRACE_INK: &str = "ink.dark";
pub const HATCH_STROKE: f32 = 1.5;
pub const HATCH_SPACING: f32 = 6.0;
pub const READOUT_TOP: f32 = 18.0;
/// The zone row: the Zone and Group columns' shares; the Zone panel's row gap, header gap and field
/// gap; the Loop label's width; the gap before Crossfade and the crossfade field's width.
pub const ZONE_COLUMNS: [f32; 2] = [1.45, 1.0];
pub const ZONE_ROW_GAP: f32 = 10.0;
pub const ZONE_HEADER_GAP: f32 = 10.0;
pub const ZONE_FIELD_GAP: f32 = 14.0;
pub const LOOP_LABEL_WIDTH: f32 = 40.0;
pub const CROSSFADE_LABEL_GAP: f32 = 6.0;
pub const CROSSFADE_WIDTH: f32 = 76.0;
/// Every string under `sampler/instrument` in tokens.json by its path.
pub const INSTRUMENT_STRINGS: &[(&str, &str)] = &[
    ("instrument/keyMap/keyEdge", KEY_EDGE),
    ("instrument/keyMap/blackKeyEdge", BLACK_KEY_EDGE),
    ("instrument/keyMap/octaveNameInk", OCTAVE_NAME_INK),
    ("instrument/keyMap/zoneInk", ZONE_INK),
    ("instrument/keyMap/selectedInk", SELECTED_INK),
    ("instrument/keyMap/rootDotInk", ROOT_DOT_INK),
    ("instrument/keyMap/selectedRootDotInk", SELECTED_ROOT_DOT_INK),
    ("instrument/keyMap/rrTagEdge", RR_TAG_EDGE),
    ("instrument/keyMap/rrTagInk", RR_TAG_INK),
    ("instrument/keyMap/padZoneFill", PAD_ZONE_FILL),
    ("instrument/keyMap/padZoneInk", PAD_ZONE_INK),
    ("instrument/keyMap/padZoneDot", PAD_ZONE_DOT),
    ("instrument/keyMap/conflictEdge", CONFLICT_EDGE),
    ("instrument/waveform/flagInk", ZONE_FLAG_INK),
    ("instrument/waveform/braceInk", BRACE_INK),
];
/// Every number under `sampler/instrument` in tokens.json by its path.
pub const INSTRUMENT_METRICS: &[(&str, f32)] = &[
    ("instrument/keyMap/wellHeight", KEY_MAP_WELL_HEIGHT),
    ("instrument/keyMap/axisWidth", AXIS_WIDTH),
    ("instrument/keyMap/axisGap", AXIS_GAP),
    ("instrument/keyMap/axisInset", AXIS_INSET),
    ("instrument/keyMap/keyboardGap", KEYBOARD_GAP),
    ("instrument/keyMap/keyboardHeight", KEYBOARD_HEIGHT),
    ("instrument/keyMap/blackKeyHeight", BLACK_KEY_HEIGHT),
    ("instrument/keyMap/blackKeyWidth", BLACK_KEY_WIDTH),
    ("instrument/keyMap/whiteKeyRadius", WHITE_KEY_RADIUS),
    ("instrument/keyMap/blackKeyRadius", BLACK_KEY_RADIUS),
    ("instrument/keyMap/octaveNameInset", OCTAVE_NAME_INSET),
    ("instrument/keyMap/zoneInset", ZONE_INSET),
    ("instrument/keyMap/labelInset/0", ZONE_LABEL_INSET[0]),
    ("instrument/keyMap/labelInset/1", ZONE_LABEL_INSET[1]),
    ("instrument/keyMap/rootDotInset", ROOT_DOT_INSET),
    ("instrument/keyMap/rrTagInset", RR_TAG_INSET),
    ("instrument/keyMap/rrTagPaddingX", RR_TAG_PADDING_X),
    ("instrument/keyMap/rrTagRadius", RR_TAG_RADIUS),
    ("instrument/keyMap/conflictWidth", CONFLICT_WIDTH),
    ("instrument/waveform/wellHeight", ZONE_WAVE_HEIGHT),
    ("instrument/waveform/flagPaddingX", ZONE_FLAG_PADDING_X),
    ("instrument/waveform/flagRadius", ZONE_FLAG_RADIUS),
    ("instrument/waveform/braceRadius", BRACE_RADIUS),
    ("instrument/waveform/hatchStroke", HATCH_STROKE),
    ("instrument/waveform/hatchSpacing", HATCH_SPACING),
    ("instrument/waveform/readoutTop", READOUT_TOP),
    ("instrument/zonePanel/columns/0", ZONE_COLUMNS[0]),
    ("instrument/zonePanel/columns/1", ZONE_COLUMNS[1]),
    ("instrument/zonePanel/rowGap", ZONE_ROW_GAP),
    ("instrument/zonePanel/headerGap", ZONE_HEADER_GAP),
    ("instrument/zonePanel/fieldGap", ZONE_FIELD_GAP),
    ("instrument/zonePanel/loopLabelWidth", LOOP_LABEL_WIDTH),
    ("instrument/zonePanel/crossfadeLabelGap", CROSSFADE_LABEL_GAP),
    ("instrument/zonePanel/crossfadeWidth", CROSSFADE_WIDTH),
];
