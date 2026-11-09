# E78: a header test measured its controls against the row, not the painted band (#412)

1. **Failure.** The piano roll header's controls did not look centered or equal in height, in
   two rounds of the same issue (2026-11-08 and 2026-11-09). The maintainer's screenshots of
   the installed build, at a 1.5 display scale, showed a painted band about 33 pt tall with the
   24 pt controls in its lower 28 pt: about 7 pt of band above them and 2 below. The Length
   field's frame also read as a different height from the buttons in two shots.
2. **Mechanism.** The header band is the pane's top drag strip (`HANDLE_H`, 5 pt) plus the
   header row (`HEADER_H`, 28 pt), all in the panel color with a line at each edge. The layout
   handed `header_row` only the 28 pt row under the strip, so the controls were centered on
   the row, not on the band the eye sees. Round one sized every control's allocated rectangle to
   the dense control height and measured the allocations against the row. Round two painted the
   field frames ourselves (`header_field`) and measured the painted frames, again against the
   row. Both measured the right thing against the wrong reference; the headless rig was
   correct about every number it was asked for.
3. **Why checks missed it.** A test that compares controls with the rectangle the layout hands
   in can only find controls that disagree with each other. Here they agreed and all sat 2.5 pt
   low in the band that the user sees. The painted band is not a layout rectangle: it is two
   lines and a fill, and only the painted shapes say where it is.
4. **Fix and regression tests.** The header band is one rectangle (`HEADER_BAND_H`, 33 pt, the
   old strip plus row so nothing below moves); the drag strip (4 pt) overlays its top edge above
   the controls, and the tabs, swatch, separator, title and every control are centered on the
   band. `piano_roll_header_controls_share_one_height_and_center_line` reads the band from the
   painted shapes (`painted_band`: the pane's top line and the line under the header) and
   asserts every control's center is the band's center, within a physical pixel, at 1.0, 1.5
   and 2.0 scale, and keeps the allocation and painted-frame assertions.
   `the_length_field_frame_is_the_button_height_in_every_state` asserts the Length frame is the
   buttons' height with no selection, with a value and focused, wide and narrow.
   Control, run and reverted: with the controls centered on the bottom row again (the pre-fix
   layout), both tests fail on the band-center assertion (`step: center 458, painted band
   439..472 center 455.5`).
5. **Gate class.** A header or toolbar test measures its controls against the painted band
   behind them, not against the row rectangle the layout hands in. The rule is in
   `docs/contributing/evidence/gates.md`.
6. **Protocol change, owner, blind spot.** Review rule plus the shared band reader
   (`painted_band`). Owner: the maintainer. Blind spot: the band is read from its two edge
   lines, so a band that changes how it is painted needs the reader updated with it.
