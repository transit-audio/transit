# MIDI Monitor

Window > MIDI Monitor (issue #91): every message from every open input port, routed or not, and
every message transit sends to an output port, plus the "MIDI" LED at the right of the transport
bar. Input side: `midi.md`; output side: `midi-output.md`; thread rules: `audio-thread.md`.

## Shape

| Piece | Where | Thread |
|---|---|---|
| Input tap | `Mon::tap` in `crates/playback/src/player/ingress.rs` | ingress worker (never the OS callback) |
| Output tap | `Worker::send` in `player/midi_out.rs`, right after a successful send | output thread |
| Rings, element, clock estimator, activity counter | `player/monitor.rs` | both producers; the UI reads |
| Player API | `Player::set_midi_monitor`, `take_midi_monitor`, `midi_activity` | UI |
| Window, decoder, filters, copy, LED | `crates/ui-studio/src/midi_monitor.rs` | UI |

## The ring

One `rtrb` SPSC ring per direction, 4096 elements, allocated in `Hub::new`. Element `MonitorEvent`
(24 bytes, `Copy`): `host_ns` (the runtime's host clock; inputs carry the callback's arrival time,
outputs the send time), `sysex_len`, `port` (the ingress input id, or the output thread's port
index), `dir`, `channel` (`NO_CHANNEL` for system messages), `status`, `data1`, `data2`. SysEx bytes
are not copied. A full ring drops the event and bumps a counter per direction; the window shows it.
Transport position is not stamped; the window shows wall-clock time.

## Thread contract

The producers do no allocation, locking, logging or blocking for the monitor: a `Copy` struct
into a preallocated ring, a few relaxed atomics. Nothing is produced while the window is closed
(`Tap::enabled`), except the activity counter below. The ingress worker reads input in the order it
pops it. A port with no admitting route is read without being popped (`for_each_pending`), so a
route attached later keeps its backlog; such a port is trimmed only past 512 pending events, all of
them already shown. While the monitor is on, `Native::service` opens every present input port with
no routes, and closes the extra ports when it goes off: the only change to what transit opens.
Routing, delivery and echo are unchanged (tests: `monitor_shows_an_unrouted_port_and_leaves_routing_alone`).

## What is not an event

- **Clock (0xF8)** feeds `ClockRate` (exponential average of the pulse interval, BPM = 60e9 /
  (interval x 24)) and is published in a per-input slot of atomics; it becomes an event only with
  "Show clock pulses". No pulse for 1 s: no rate.
- **Active Sensing (0xFE)** is filtered by the backend and counted; the worker turns the counter
  moving into "Active Sensing present" (2 s window).
- **SysEx** never enters the input ring; the worker turns the dropped-byte counter into one event
  with the byte count.
- Start, Continue, Stop, Song Position and the rest are ordinary single rows.

## Header LED

`Hub::activity` counts every input message except Clock and Active Sensing, whether or not the window
is open (it is lit by the ports open for routing; with the window open, by all of them). The UI
reads the counter once per frame; `Led::level` lights it on a change and fades it linearly over
120 ms, so a stream reads as steady and one tap as a blink. Clicking it opens the window; the
tooltip is the last port and message.

## Window

Filters (saved in `ui.json` with the window state): direction, devices, channels, type chips, text
(matches the decoded text and hex). Port header lines: state, tracks, msg/s, Clock BPM, Active
Sensing, "no data in N s" after 10 s. Rows: time, direction, port (colored dot), channel, decoded
message, hex. Hovering a note or CC row highlights the same number. Pause freezes the view while
rows keep arriving (the count is shown); Follow pins to the newest; Learn catches the next input
message in large type; Copy last 100 lines and Copy report put plain text on the clipboard.
Bounded to 5000 rows.

## Extending

A new message kind: add its decode to `describe` and a row to the table test
`every_message_class_decodes`. A new filter: a field in `Filters` (with `#[serde(default)]`), a
branch in `passes`, a row in `filter_combinations`. A new tap point (slice 2: the track insert
variant) takes a `Tap` from `Hub::new` and follows the contract above.
