# ADR 0024: A group is a routing folder track; members follow it in session order

- **Status:** accepted 2026-10-24 (retroactive; decided 2026-10-23 in #60)
- **Date:** 2026-10-24
- **Reopen when:** a group needs to be a non-routing grouping (a folder that only organizes, with members left on Main); nesting from the Ctrl+G gesture is asked for (today only drag nests); membership needs to live somewhere other than `Track::folder` and session order (a track in two groups); or a group needs a view state that must survive a reopen (collapse is not saved today).

## Context

Folder tracks existed in the model (`TrackKind::Folder`), and the arranger drew them with a disclosure triangle, but there was no direct way to make an empty one, and Cmd+G ran `track.group`, the Pro Tools-style edit/mix group, which is a different feature: tracks that share edit and mix operations. Users of Ableton and Logic expect Cmd+G to make a group track from the selection (#60). The record is needed because a group is more than a visual fold: it is a part of the mixer's routing graph, and the order of tracks in the session file carries its membership.

## Decision

1. **A group is a `TrackKind::Folder` track ("Routing Folder Track") that owns a summing bus.** Creating one (`track.add` with kind `folder`, or `track.move_to_new_folder`) also adds a stereo bus named "<name> Bus" (unique among busses; a user bus of the same name is never reused) and sets the folder's `mixer.input` to `Route::Bus(bus)`. The folder's strip reads that bus like an aux (`crates/mix/src/lib.rs`: `Aux | Folder` share the render path), so inserts on the group are group effects. Folders are processed after every strip that feeds them, at any depth, by routing order (`processing order`, same file).
2. **Membership is `Track::folder: Option<TrackId>` on the member** (`crates/model/src/track.rs`, `#[serde(default)]`), the parent group's id; the group does not list its members. Collapse is `Track::folder_open` in the engine, but the studio keeps collapse as view state (`UiState::collapsed`) and never calls `track.folder_toggle`, so opening or closing a group neither dirties the document nor makes an undo step.
3. **Members are contiguous after their group.** Session order (`Session::tracks`) is the tree flattened depth first: each group is followed by its members, and a nested group by its own. The engine's `place_under` keeps a folder's subtree together as one stable block whenever a track moves in or out, and the studio's `normalize` restores the order after a gesture (`crates/ui-studio/src/groups.rs`: `tree`, `flatten`). A track the walk cannot reach (a cyclic chain in a hostile file) keeps its place at the end. `parent_of` ignores a folder that is the track itself or does not exist, and depth and ancestry walks are capped (16 and 64 steps).
4. **Routing follows membership.** A member whose output is Main (or its old group's bus) moves to the group's bus when it joins (`track.move_to_folder`, `track.move_to_new_folder`). When it leaves (`track.remove_from_folder`), it goes to the parent group's bus, or Main at the top level. A member whose output was set to something else (another bus, a path) keeps it.
5. **Commands, all undoable as one step per gesture.**
   - *New Group Track* and "+ Group" (quick-add row: Audio / Instrument / MIDI / Group / +...) make an empty folder named "Group n", selected and focused.
   - *Ctrl+G* (`group_tracks`) makes a group named "Group n" from the selected tracks, placed where the first was, via `track.move_to_new_folder`. Masters are not grouped. Members of another group leave it first, so a Ctrl+G group is one level deep.
   - *If the selection holds one group's header*, the other tracks join that group instead of nesting it. *Two headers* are refused with "Groups are one level deep".
   - *Ctrl+Shift+G* (`ungroup`) dissolves the chosen groups (the selected group, or the group of a selected member): the members leave and stay in place, the group track is deleted, and its bus is removed when nothing else uses it. A bus that outside tracks still feed is kept and those tracks are rerouted to Main so nothing goes silent; the status names them. Deleting a group takes this path (`actions.rs`), so members survive.
   - *Drag a header* into a group (onto its header: last member; or between members) or out of one (to the top level, one level per engine step). A group moves with its members. A group cannot drop into itself or a descendant, and a drop target must be a folder. Dragging can nest groups (the engine allows it); Alt+Up/Down moves the selected tracks one place among their siblings.
   - *Track > Edit Group...* is the old `track.group` (Pro Tools-style edit/mix group, Cmd+Shift+G in the classic UI), unchanged and not a folder.
6. **Undo.** A gesture that runs several commands (`track.move` per displaced track, membership changes, ungroup) runs them with `execute_merged` under one key (`composite`), so it is one undo step. If a step fails, the helper undoes what it did.

## Consequences

- A group is a real bus in the mix, so its inserts are group effects; its members cannot be reordered out of their group except through the commands above, because the file order is the membership view.
- Because membership is a field on the member plus file order, a hand-edited or hostile file can hold a folder pointing at a missing or cyclic parent. The walks tolerate it (the caps and `parent_of` above).
- An ungrouped session keeps its bus only if something else uses it, so dissolving a group does not leave an orphaned bus.
- The edit/mix group (`track.group`) and the folder group share a word in the UI and nothing in the model; the menu names them apart ("Edit Group...").

## Alternatives rejected

- **Reuse Cmd+G for the edit/mix group.** Rejected in #60: Ableton and Logic users expect a folder, so Cmd+G makes the folder and the old command moves to Cmd+Shift+G.
- **A group that only organizes the arranger, with members left on Main.** The folder kind already summed through its own bus; ungroup and the reroute rules above exist so that a group stays a mixing object.

## Related

[ADR 0015](0015-io-routing-abstraction.md) (busses and route graph), `crates/engine/src/cmd/track.rs`, `crates/ui-studio/src/groups.rs`, `groups_tests.rs`, `docs/architecture.md`, issue #60, commits 622315d and 242df73.
