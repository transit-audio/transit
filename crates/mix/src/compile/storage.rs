//! Prepared plan storage accounting.
use super::*;

pub(super) fn over(what: &'static str, need: usize, limit: usize) -> Result<(), CompileError> {
    if need > limit { Err(CompileError::ResourceLimit { what, need, limit }) } else { Ok(()) }
}

/// `a * b`, or the overflow error.
fn mul(a: usize, b: usize) -> Result<usize, CompileError> {
    a.checked_mul(b).ok_or(CompileError::ResourceLimit { what: "prepared_bytes", need: usize::MAX, limit: usize::MAX })
}

fn add(a: usize, b: usize) -> Result<usize, CompileError> {
    a.checked_add(b).ok_or(CompileError::ResourceLimit { what: "prepared_bytes", need: usize::MAX, limit: usize::MAX })
}

/// Bytes one strip's prepared storage takes: `buf`, `pre`, `outbuf` and `clipbuf` (channels ×
/// block each), `scratch` (block), two delay lines (channels × (`cap` + block); 0 when the strip
/// keeps its live lines), its insert positions (filled or not), its clip-effect entries, its
/// meter, and the strip itself. (An instrument's event state is its slot's, see `plan_slot`.)
pub(crate) fn strip_bytes(ch: usize, block: usize, cap: usize, inserts: usize, clips: usize) -> Result<usize, CompileError> {
    let f = size_of::<f32>();
    let bufs = mul(mul(mul(ch, block)?, 4)?, f)?;
    let scratch = mul(block, f)?;
    let delays = if cap == 0 { 0 } else { mul(mul(mul(ch, add(cap, block)?)?, 2)?, f)? };
    let slots = mul(inserts, size_of::<Option<PluginSlot>>())?;
    let entries = mul(clips, size_of::<(u64, ClipFx)>())?;
    let meter = mul(ch, f)?;
    [scratch, delays, slots, entries, meter, size_of::<Strip>()].into_iter().try_fold(bufs, add)
}

/// Checks every limit and the prepared bytes, with each strip's delay capacity from `cap_of`
/// (0: the strip keeps its live lines). Allocates nothing.
pub(super) fn measure(
    s: &Session,
    l: &PlanLimits,
    block: usize,
    cap_of: &dyn Fn(&Track, usize) -> usize,
    key_cap_of: &dyn Fn(&Track, usize) -> usize,
) -> Result<(), CompileError> {
    over("tracks", s.tracks.len(), l.tracks.into())?;
    over("buses", s.busses.len(), l.buses.into())?;
    let main_ch = main_channels(s);
    let mut bytes = 0usize;
    let mut cells = 0usize;
    for t in &s.tracks {
        let ch = strip_channels(t, main_ch);
        over("slots_per_strip", t.mixer.inserts.len(), l.slots_per_strip.into())?;
        let clips = if t.kind == TrackKind::Audio {
            t.clips().iter().filter(|c| c.is_audio()).filter(|c| clip_fx_modules(s, c.id.0) != (false, false)).count()
        } else {
            0
        };
        over("clip_fx_per_strip", clips, l.clip_fx_per_strip.into())?;
        let slots = add(add(t.mixer.inserts.iter().flatten().count(), usize::from(t.instrument.is_some()))?, mul(clips, 2)?)?;
        cells = add(add(cells, 1)?, slots)?;
        bytes = add(bytes, strip_bytes(ch, block, cap_of(t, ch), t.mixer.inserts.len(), clips)?)?;
        if live(t)
            && is_aux(t)
            && let Route::Bus(id) = t.mixer.input
            && let Some(bus) = s.busses.iter().find(|b| b.id == id)
        {
            let channels = bus.format.channels().clamp(1, MAX_CHANNELS);
            bytes = add(bytes, mul(mul(channels, block)?, size_of::<f32>())?)?;
            bytes = add(bytes, add(size_of::<(usize, Option<(BusId, BusBuf)>)>(), mul(channels, size_of::<Vec<f32>>())?)?)?;
        }
        bytes = add(bytes, mul(add(t.mixer.sends.len(), 1)?, size_of::<Option<usize>>())?)?;
        // Each keyed insert holds two delay lines (its signal's and the key's), as wide as the strip.
        let keyed = t.mixer.inserts.iter().flatten().filter(|i| i.key.is_some()).count();
        if keyed > 0 {
            let line = mul(mul(ch, add(key_cap_of(t, ch), block)?)?, size_of::<f32>())?;
            bytes = add(bytes, mul(mul(keyed, 2)?, line)?)?;
        }
        // A sample ring for each insert that asks for one (the Tuner).
        let tuners = t.mixer.inserts.iter().flatten().filter(|i| transit_dsp::wants_sample_tap(&i.plugin)).count();
        bytes = add(bytes, mul(tuners, SampleTap::BYTES)?)?;
        // A note carry for each built-in insert that reads the track's notes (hosted and hardware
        // inserts never do); the compiler makes one only for those (`PluginSlot::new`).
        let readers = t.mixer.inserts.iter().flatten().filter(|i| transit_dsp::builtin_reads_track_notes(&i.plugin)).count();
        bytes = add(bytes, mul(readers, crate::note_delay::NoteDelay::BYTES)?)?;
        // A scope ring for each insert and the instrument.
        bytes = add(bytes, mul(add(t.mixer.inserts.iter().flatten().count(), usize::from(t.instrument.is_some()))?, ScopeTap::BYTES)?)?;
        for ins in t.mixer.inserts.iter().flatten().chain(t.instrument.iter()) {
            if needs_storage(ins.params.len()) {
                bytes = add(bytes, mul(ins.params.len(), size_of::<f32>())?)?;
            }
        }
    }
    for b in &s.busses {
        bytes = add(bytes, mul(mul(b.format.channels().clamp(1, MAX_CHANNELS), block)?, size_of::<f32>())?)?;
        bytes = add(bytes, size_of::<(BusId, BusBuf)>())?;
    }
    bytes = add(bytes, mul(mul(main_ch, block)?, size_of::<f32>())?)?;
    bytes = add(bytes, mul(main_ch, size_of::<f32>())?)?;
    bytes = add(bytes, mul(cells, size_of::<CellLoc>() + 2 * size_of::<bool>())?)?;
    bytes = add(bytes, mul(s.tracks.len(), size_of::<(TrackId, u16)>() + size_of::<BusId>())?)?;
    // Conservative tail table/run and edge storage bound, including deduplication scratch.
    // Every pair of aux rows can contribute input, key and destination ordering edges.
    let auxes = s.tracks.iter().filter(|t| is_aux(t)).count();
    let tasks = add(add(s.tracks.len(), auxes)?, 1)?;
    bytes = add(bytes, mul(tasks, 256)?)?;
    let mut edge_bound = add(mul(auxes, auxes)?, tasks)?;
    for t in &s.tracks {
        edge_bound = add(edge_bound, add(mul(t.mixer.inserts.len(), 2)?, add(t.mixer.sends.len(), 2)?)?)?;
    }
    // Two edge Vecs coexist during construction, each with less than twice the final
    // edge capacity (24 bytes/edge on 64-bit), plus the graph's compact output arrays.
    bytes = add(bytes, mul(edge_bound, 128)?)?;
    over("cells", cells, usize::from(u16::MAX))?;
    over("prepared_bytes", bytes, l.prepared_bytes)
}

#[cfg(test)]
mod tail_tests {
    use super::*;
    #[test]
    fn every_live_reader_snapshot_is_charged_before_allocation() {
        let mut s = Session::default();
        let id = BusId(s.alloc());
        s.busses.push(transit_model::Bus { id, name: "wide".into(), format: ChannelFormat::Stereo });
        let t = s.add_track(TrackKind::Aux, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.input = Route::Bus(id);
        let limits = PlanLimits { prepared_bytes: 0, ..PlanLimits::default() };
        let need = |s: &Session| match measure(s, &limits, 128, &|_, _| 0, &|_, _| 0) {
            Err(CompileError::ResourceLimit { what: "prepared_bytes", need, .. }) => need,
            other => panic!("{other:?}"),
        };
        let live_need = need(&s);
        s.track_mut(t).unwrap().inactive = true;
        let inactive_need = need(&s);
        assert_eq!(live_need - inactive_need, 2 * 128 * size_of::<f32>() + size_of::<(usize, Option<(BusId, BusBuf)>)>() + 2 * size_of::<Vec<f32>>());
        s.track_mut(t).unwrap().inactive = false;
        let limits = PlanLimits { prepared_bytes: live_need - 1, ..limits };
        assert!(measure(&s, &limits, 128, &|_, _| 0, &|_, _| 0).is_err());
        let limits = PlanLimits { prepared_bytes: live_need, ..limits };
        assert!(measure(&s, &limits, 128, &|_, _| 0, &|_, _| 0).is_ok());
    }
}
