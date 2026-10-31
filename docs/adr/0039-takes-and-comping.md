# ADR 0039: Takes and comping: take stacks, the active take, take lanes and Flatten

- **Status:** proposed 2026-10-31 (#25 slice 1)
- **Date:** 2026-10-31
- **Reopen when:** swipe comping lands (slice 2: a range across takes with crossfades), which may want a comp kept apart from the takes; a user needs a take's material to play on two tracks at once; or the "copy of a clip on the active playlist is no take" rule hides a take someone recorded on purpose.

## Context

Cycle recording keeps a take per lap ([ADR 0026](0026-cycle-recording-stacks-takes.md)), and recording over clips keeps the old clips as the previous take: both are stored as track playlists (plan 06, L8). Two things were implicit. Which take plays was the whole track's active playlist, so choosing a take switched every clip of the track; and after cycle recording the track played the last lap's playlist, which held only that lap, so the rest of the track went silent. A clip under the loop of an audio cycle recording was cut away. The take lanes showed one lane per playlist and could only switch the whole track or keep one playlist.

Convention (transit follows what Ableton and Logic agree on): Logic's Take Folder (takes stacked under the track, click a take to make it the one that plays, Quick Swipe Comping, Flatten) and Ableton's Take Lanes (Live 11: Show Take Lanes under the track, a lane per pass, click a take to make it the main clip, highlight a range to comp, Flatten). Both agree on lanes under the track, one per take; a click on a take makes it play for that region; Flatten keeps what plays and removes the other takes. After cycle recording, Logic opens the take folder and Live shows the lanes, with the newest take playing.

## Decision

1. **A take stack** is a set of clips across a track's playlists that overlap in time (transitively) and hold at least two takes on at least two playlists (`Track::take_stacks`). A clip on another playlist that equals one on the active playlist in everything but its id is no take: Duplicate Playlist does not stack the whole track.
2. **A take** is identified by its number, `Clip::take` (optional, written only when set, so files without stacks are unchanged). Recording writes it: the material a take is recorded over becomes take 1 (the whole old clip, as Logic keeps it), each pass the next number. A clip without a number belongs to its playlist's untagged take, numbered with the lowest number no tagged take uses, in playlist order (older sessions); a take command writes those numbers before it moves anything.
3. **The active take is explicit in the data:** it is the take on the active playlist, which is all the mix renders (verified: the mix reads only `Track::clips()`). Making a take play (`takes.set_active {track, clip}` or `{track, take, stack?}`, one undo step) moves its clip onto the active playlist and the material it replaces, cut at the clip's edges, onto the clip's playlist. Nothing is copied or lost, numbers stay, so lanes stay put. A shorter take plays over its own range with the longer take around it (Live's main lane, Logic's comp); the stack's active take is the one that plays most of it.
4. **After recording,** the track plays the playlist it played before the passes, and each pass is made to play in turn, so the newest plays over its range (a partial last lap: the lap before it plays after it). An audio cycle recording over clips now duplicates the playlist first, as MIDI did, so the clip under the loop is take 1. After cycle recording the track's take lanes open.
5. **Flatten** (`takes.flatten {track, stack?}`, one undo step) keeps what plays in the stack (or all stacks), removes the other takes' clips, clears the numbers of what stays, and removes the playlists it emptied. **Delete Take** (`takes.delete {track, take, stack?}`) removes a take that does not play and refuses one that does. `takes.list` reports the stacks.
6. **Take lanes** (view state): one lane per take number, newest on top, under the track at a fixed lane height; the take's clips dimmed where another take plays and highlighted where it plays. A click on a lane's clip makes its take play there; a click on the lane's label makes the take play in every stack it has material in; the lane menu has Flatten Takes and Delete Take. The header's T button and the track menu show or hide the lanes; the track and clip menus have Flatten Takes. No new keys.

## Consequences

- Choosing a take never silences the rest of the track; old sessions' takes number themselves from playlist order.
- Playlists stay the storage, so Pro Tools style playlist commands keep working; after a take change a non-active playlist holds a mix of takes, which only the playlist selector shows.
- Swipe comping, take names and colors, lane order and audition (per-lane solo) are slice 2. No recording time is stored, so lanes are labeled by number only.

## Alternatives considered

- **Copy the chosen take onto a comp lane (Live's main lane holds copies).** Duplicates every chosen clip and needs a link back to its take; moving keeps one copy of each clip.
- **Keep the whole-playlist switch.** It is what silenced the rest of the track.
- **Number takes per playlist without a field.** Numbers would move when clips move between playlists.
