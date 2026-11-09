# E78: a header test measured the allocated rect, not the painted frame (#412)

1. **Failure.** The piano roll header's Length field still looked shorter than the row after
   the first #412 fix landed (2026-11-09, the maintainer's screenshot of the installed build):
   the velocity box matched the buttons, the Length frame did not. Issue #412 was reopened.
2. **Mechanism.** The first fix sized every control's allocated rectangle to the dense control
   height (24) and the test measured those rectangles. egui's `TextEdit` and `DragValue` paint
   their own frames, from their content and their margins, and the frame was never part of what
   the test looked at. The headless rig, run at 1.0, 1.25, 1.5 and 2.0 scale and in the
   unselected, selected, hovered and focused states, painted both field frames 24 high on the
   buttons' center line before and after this fix, so the rig does not reproduce the short frame
   on that screenshot. What the fix removes is the dependency: both fields now get the kit's
   frame from one helper (`header_field` in `crates/ui-studio/src/keys/view.rs`), painted by us
   on the control band, with egui's widget bare inside it, so the painted height is the band's
   whatever the widget measures.
3. **Why checks missed it.** The layout test asserted allocation (`Response::rect`) and
   `layout` rectangles. A painted control has two sizes, the one it asks for and the one it
   paints, and a test that reads only the first cannot see the second drift.
4. **Regression test.** `piano_roll_header_controls_share_one_height_and_center_line` keeps the
   allocation assertion and adds a paint assertion (`painted_aligned`): every visible rectangle
   over the Length field and the velocity field, taken from the frame's shapes, is the control
   height high within one physical pixel on the buttons' center line, and each field has the kit
   inset frame with its 1 px line, at 0.75, 1.0, 1.5 and 2.0 scale, wide and narrow.
   `the_painted_check_fails_on_a_frame_shorter_than_its_allocation` feeds the check an 18 high
   frame in a 24 high allocation (the reported symptom): the allocation check passes it, the
   painted check fails it. Control, run and reverted: the pre-fix header code passes the new
   paint assertion in the rig, which agrees with item 2; the control that proves the assertion
   can see the escape is the synthetic short frame.
5. **Gate class.** A layout test for a painted control asserts the painted frame shape, not only
   the allocated rectangle. The rule is in `docs/contributing/evidence/gates.md`.
6. **Protocol change, owner, blind spot.** Review rule plus the shared paint helper
   (`painted_aligned`). Owner: the maintainer. Blind spot: the rig draws without the GPU, so a
   defect that appears only in rasterization (pixel snapping, a driver) is outside this check;
   if the Length frame still looks short on screen after this change, the next step is a
   screenshot through the control channel on the installed build.
