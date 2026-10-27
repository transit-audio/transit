use super::*;

/// The busses nobody listens to (no track has one as its input): nobody hears them.
pub fn orphan_busses(s: &Session) -> Vec<BusId> {
    s.busses.iter().filter(|b| !heard(s, b.id)).map(|b| b.id).collect()
}

/// Whether any track listens to `bus`.
fn heard(s: &Session, bus: BusId) -> bool {
    s.tracks.iter().any(|t| t.mixer.input == Route::Bus(bus))
}

/// Ends a routing menu with "Remove unused buses (n)" when the session has orphans.
pub(super) fn push_unused_entry(s: &Session, choices: &mut Vec<RoutingChoice>) {
    let n = orphan_busses(s).len();
    if n > 0 {
        choices.push(RoutingChoice::Entry { label: format!("Remove unused buses ({n})"), value: REMOVE_UNUSED.into() });
    }
}

/// The aux track fed by `bus`, if any: an FX or group channel.
fn channel_of_bus(s: &Session, bus: BusId) -> Option<&Track> {
    // A group's own bus reads as the group (slice 2c).
    s.tracks.iter().find(|t| matches!(t.kind, TrackKind::Aux | TrackKind::Folder) && t.mixer.input == Route::Bus(bus))
}

/// A bus as a routing target reads: the FX channel it feeds, else "Bus: name".
pub fn bus_label(s: &Session, bus: BusId) -> String {
    channel_of_bus(s, bus).map_or_else(|| format!("Bus: {}", s.bus(bus).map_or("?", |b| b.name.as_str())), |c| c.name.clone())
}

/// The busses a track feeds: its output and its sends' targets.
fn bus_targets(t: &Track) -> impl Iterator<Item = BusId> + '_ {
    std::iter::once(&t.mixer.output).chain(t.mixer.sends.iter().flatten().map(|s| &s.target)).filter_map(|r| match r {
        Route::Bus(b) => Some(*b),
        _ => None,
    })
}

/// Whether audio put on bus `from` reaches bus `to`: through every track listening to a bus and
/// on through its output and sends, any number of hops (`from == to` counts).
pub fn reaches(s: &Session, from: BusId, to: BusId) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut todo = vec![from];
    while let Some(b) = todo.pop() {
        if b == to {
            return true;
        }
        if !seen.insert(b) {
            continue;
        }
        for t in s.tracks.iter().filter(|t| t.mixer.input == Route::Bus(b)) {
            todo.extend(bus_targets(t));
        }
    }
    false
}

/// Whether `t` feeding bus `b` (output or send) would close a loop back to `t`'s own input.
pub fn feeding_loops(s: &Session, t: &Track, b: BusId) -> bool {
    route_loops(s, transit_model::RouteChange::Output { track: t.id, route: Route::Bus(b) }) || audio_input_loop(s, t, b)
}

/// Whether `t` listening to bus `b` would close a loop: something `t` feeds reaches `b`.
pub fn listening_loops(s: &Session, t: &Track, b: BusId) -> bool {
    route_loops(s, transit_model::RouteChange::Input { track: t.id, route: Route::Bus(b) })
        // Audio tracks do not read buses in the model graph. Keep the UI's existing refusal
        // for their input menus as well; changing that policy is separate from graph reuse.
        || (t.kind == TrackKind::Audio && bus_targets(t).any(|o| reaches(s, o, b)))
}

// Preserve the existing audio-track input-menu policy in either edit order.
fn audio_input_loop(s: &Session, t: &Track, b: BusId) -> bool {
    t.kind == TrackKind::Audio && matches!(t.mixer.input, Route::Bus(input) if reaches(s, b, input))
}

fn route_loops(s: &Session, change: transit_model::RouteChange) -> bool {
    transit_model::would_create_cycle_all(s, &[change]).is_some()
}

/// Whether a track kind has sends here: audio, instrument, FX channels and groups (a group sums
/// its members like an FX channel, and the mix runs its sends the same way).
pub fn takes_sends(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Aux | TrackKind::Folder)
}

/// Where a send from `t` can go: buses that are not already sends (`except` slot aside) and
/// would not close a loop, then a separator and the available output paths.
pub fn send_targets(s: &Session, t: &Track, except: Option<usize>, io: &IoMap) -> Vec<RoutingChoice> {
    let Some(slot) = except.filter(|&slot| slot < t.mixer.sends.len()).or_else(|| t.mixer.sends.iter().position(Option::is_none)) else {
        return Vec::new();
    };
    let taken: Vec<&Route> =
        t.mixer.sends.iter().enumerate().filter(|(i, _)| Some(*i) != except).filter_map(|(_, x)| x.as_ref()).map(|x| &x.target).collect();
    let mut v: Vec<RoutingChoice> = s
        .busses
        .iter()
        .filter(|b| {
            !taken.contains(&&Route::Bus(b.id))
                && !audio_input_loop(s, t, b.id)
                && !route_loops(s, transit_model::RouteChange::Send { track: t.id, slot, route: Route::Bus(b.id) })
                && heard(s, b.id)
        })
        .map(|b| RoutingChoice::Entry { label: bus_label(s, b.id), value: b.name.clone() })
        .collect();
    let paths = path_choices(io, IoDirection::Output)
        .into_iter()
        .filter(|choice| matches!(choice, RoutingChoice::Entry { value, .. } if !taken.contains(&&Route::Hardware(value.clone()))))
        .collect();
    push_paths(&mut v, paths);
    v
}

/// A routing menu row: a separator picks nothing; an entry carries its label and command value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoutingChoice {
    Separator,
    Entry { label: String, value: String },
}

/// A separator, then `paths`, when there are any.
fn push_paths(v: &mut Vec<RoutingChoice>, paths: Vec<RoutingChoice>) {
    if !paths.is_empty() {
        v.push(RoutingChoice::Separator);
        v.extend(paths);
    }
}

/// A path as a dropdown label: its name, with the device channels after it when the name does not
/// already say them: "Out 3-4", "Headphones (Out 3-4)".
fn path_label(p: &IoPath) -> String {
    let prefix = if p.direction == IoDirection::Input { "In" } else { "Out" };
    let channels = format!("{prefix} {}", crate::audio_devices_io::channel_label(&p.channels));
    if p.name.eq_ignore_ascii_case(&channels) { p.name.clone() } else { format!("{} ({channels})", p.name) }
}

/// The I/O map's paths of one direction as entries with path names as values, in the map's order.
/// Main's path is left out of the outputs (that is the "Main" entry).
pub fn path_choices(io: &IoMap, direction: IoDirection) -> Vec<RoutingChoice> {
    io.paths_of(direction)
        .filter(|p| direction == IoDirection::Input || !p.name.eq_ignore_ascii_case(&io.main))
        .map(|p| RoutingChoice::Entry { label: path_label(p), value: p.name.clone() })
        .collect()
}

/// A route naming an output path this machine does not have: it plays silent until the path
/// exists again (`Engine::unresolved_routes`).
pub(super) fn unresolved_output(io: &IoMap, r: &Route) -> bool {
    matches!(r, Route::Hardware(h) if io.path(h).is_none_or(|p| p.direction != IoDirection::Output))
}

pub(super) const NOT_HERE: &str = "not on this device";

/// Whether picking `value` from a route dropdown would leave the route as it is: the same route,
/// or the label the current one already reads as (an audio track's default input, L15).
pub(super) fn same_route(s: &Session, cur: &Route, cur_label: &str, value: &str, label: &str) -> bool {
    if label == cur_label {
        return true;
    }
    let parsed = match value.to_ascii_lowercase().as_str() {
        "" | "none" | "no input" | "no output" => Route::None,
        "main" | "out 1-2" | "master" => Route::Main,
        _ => s.bus_by_name(value).map_or_else(|| Route::Hardware(value.to_string()), |b| Route::Bus(b.id)),
    };
    &parsed == cur
}

/// How a route reads in a dropdown: busses by the channel they feed (L5). An audio track whose
/// input is `None` or `Main` records and monitors from input 1 (`engine::io::plan_takes`, the mix
/// engine's monitoring), so it reads as that input, not "No input".
pub fn route_label(s: &Session, r: &Route, input: bool, format_channels: usize) -> String {
    match r {
        Route::None | Route::Main if input && format_channels > 0 => if format_channels >= 2 { "In 1-2" } else { "In 1" }.into(),
        Route::None => if input { "No input" } else { "No output" }.into(),
        Route::Main => "Main".into(),
        Route::Bus(b) if input => s.bus(*b).map_or_else(|| "Bus".into(), |b| format!("Bus: {}", b.name)),
        Route::Bus(b) => bus_label(s, *b),
        Route::Hardware(h) => h.clone(),
    }
}

/// The outputs a track can pick, with values for `track.output`.
pub fn output_choices(s: &Session, t: &Track, io: &IoMap) -> Vec<RoutingChoice> {
    let mut v = vec![RoutingChoice::Entry { label: "Main".into(), value: "main".into() }];
    for b in &s.busses {
        // Never a bus that comes back to this track's input, however many hops away.
        if feeding_loops(s, t, b.id) || (!heard(s, b.id) && t.mixer.output != Route::Bus(b.id)) {
            continue;
        }
        v.push(RoutingChoice::Entry { label: bus_label(s, b.id), value: b.name.clone() });
    }
    push_paths(&mut v, path_choices(io, IoDirection::Output));
    v.push(RoutingChoice::Entry { label: "No output".into(), value: "none".into() });
    v
}

/// The inputs a track can pick: hardware inputs (`In 1`.. or `In 1-2`..) and busses.
pub fn input_choices(s: &Session, t: &Track, device_inputs: usize, io: &IoMap) -> Vec<RoutingChoice> {
    let mut v = Vec::new();
    let n = device_inputs;
    if t.kind == TrackKind::Audio || t.kind == TrackKind::Aux {
        let width = if t.channels() >= 2 { 2 } else { 1 };
        // The map's named paths of the strip's width first; a legacy "In N" label stays only for
        // channels no named path covers, so a renamed path replaces its label.
        let named: Vec<&IoPath> = io.paths_of(IoDirection::Input).filter(|p| p.channels.len() == width).collect();
        v.extend(named.iter().map(|p| RoutingChoice::Entry { label: path_label(p), value: p.name.clone() }));
        let covered = |first: usize| {
            let want: Vec<u16> = (first - 1..first - 1 + width).filter_map(|c| u16::try_from(c).ok()).collect();
            named.iter().any(|p| p.channels == want)
        };
        for i in (1..=n).step_by(width).filter(|&i| i + width - 1 <= n && !covered(i)) {
            let label = if width == 2 { format!("In {i}-{}", i + 1) } else { format!("In {i}") };
            v.push(RoutingChoice::Entry { label: label.clone(), value: label });
        }
    }
    for b in &s.busses {
        if listening_loops(s, t, b.id) || !heard(s, b.id) {
            continue;
        }
        v.push(RoutingChoice::Entry { label: format!("Bus: {}", b.name), value: b.name.clone() });
    }
    // An audio track with no input still records from input 1 (see `route_label`), so only an
    // FX or group channel is offered "No input".
    if t.kind == TrackKind::Aux {
        v.push(RoutingChoice::Entry { label: "No input".into(), value: "none".into() });
    }
    v
}
