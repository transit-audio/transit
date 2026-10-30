# Help catalog

The text behind the Info panel (Shift+? in the app, Ableton's Info View and Logic's Quick Help).
One entry per control or figure: an `id`, a `name`, one to three plain sentences (`text`), an
optional `key` (spelled as the shortcut table spells it) and an optional `link` (a path under
`docs/`, with an anchor). A file sets `area` and a default `link` for its entries; a file with
`page = "troubleshooting.md"` also fills a generated section of that page (see below).

| File | Covers |
|---|---|
| [`transport.toml`](transport.toml) | The transport bar: buttons, readouts, tempo, meter, meters. |
| [`arrange-header.toml`](arrange-header.toml) | The track header in the arrangement. |
| [`mixer-strip.toml`](mixer-strip.toml) | A mixer channel strip. |
| [`device-chain.toml`](device-chain.toml) | The device chain: instrument, effects, sidechain keys. |
| [`telemetry.toml`](telemetry.toml) | The load readout and every figure of Callback Telemetry. |
| [`audio-devices.toml`](audio-devices.toml) | The Audio Devices status card and its buttons. |
| [`midi-monitor.toml`](midi-monitor.toml) | The MIDI Monitor window. |
| [`midi-mappings.toml`](midi-mappings.toml) | The MIDI Mappings window. |

## Adding a control

1. Add the entry to the area's file: what it does, or for a figure what it counts, what a healthy
   value is and what to do when it is not. American English, plain words, no more than three sentences.
2. Add its id to `crates/ui-studio/src/help/ids.rs` (`Variant => "area.name",`).
3. Register the widget: `crate::help::hover(ui, &response, Id::Variant)`.
4. Run `cargo xtask help --write`, then `cargo xtask help`.

`cargo xtask help` fails for an id without an entry, an entry without an id, an id no widget uses,
and a generated section that differs from the catalog. The sections `<!-- help:telemetry -->` and
`<!-- help:audio-devices -->` of [`../troubleshooting.md`](../troubleshooting.md) are written from
these files; edit the entry, not the page.
