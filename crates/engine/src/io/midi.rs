//! MIDI file import and export.

use super::*;

/// Most meter changes a MIDI file may bring into the tempo map; scores have a handful.
pub(super) const MAX_MIDI_METERS: usize = 10_000;

/// Import a Standard MIDI File as MIDI tracks (one per SMF track).
pub fn import_midi_bytes(e: &mut Engine, bytes: &[u8], at: Samples, tempo_map: bool) -> Result<Vec<TrackId>> {
    let smf = transit_midi::read_smf(bytes).map_err(|err| EngineError::Io(err.to_string()))?;
    let s = e.session_mut();
    if tempo_map {
        let old = s.tempo.clone();
        // Each meter is placed by walking the ones before it.
        if smf.meters.len() > MAX_MIDI_METERS {
            return Err(crate::cmd::bad("file.import_midi", format!("MIDI file: more than {MAX_MIDI_METERS} meter changes")));
        }
        for (tick, bpm) in &smf.tempos {
            let _ = s.tempo.set_tempo(*tick, *bpm);
        }
        for (tick, n, d) in &smf.meters {
            let _ = s.tempo.set_meter(*tick, u32::from(*n), u32::from(*d));
        }
        // The file's tempo map replaces the session's: material already on ticks tracks keeps its bars.
        crate::retime::retime_to_tempo(s, &old).map_err(|m| crate::cmd::bad("file.import_midi", format!("MIDI file tempo map: {m}")))?;
    }
    let base_tick = s.tempo.samples_to_ticks(at, s.sample_rate);
    let mut out = Vec::new();
    for (i, tr) in smf.tracks.iter().enumerate() {
        if tr.sequence.notes.is_empty() {
            continue;
        }
        let mut name = if tr.name.trim().is_empty() { format!("MIDI {}", i + 1) } else { tr.name.clone() };
        transit_model::truncate_chars(&mut name, transit_model::MAX_NAME_CHARS);
        let id = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&name));
        if let Some(t) = s.track_mut(id) {
            t.instrument = Some(transit_model::Insert::new(transit_model::DEFAULT_INSTRUMENT));
        }
        let end_tick = base_tick.saturating_add(tr.sequence.end_tick().max(1));
        let len = s.tempo.tick_to_samples(end_tick, s.sample_rate).saturating_sub(at);
        if len > s.sample_rate.samples(86_400.0) {
            return Err(crate::cmd::bad("file.import_midi", format!("MIDI track {}: longer than 24 hours", i + 1)));
        }
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, id, Clip::midi(cid, name, at, len.max(1), tr.sequence.clone()));
        out.push(id);
    }
    Ok(out)
}

/// Export MIDI clips as a format-1 SMF. `only` limits the file to those tracks.
pub fn export_midi(e: &Engine, only: Option<&[TrackId]>) -> Result<Vec<u8>> {
    export_midi_session(e.session(), only)
}

fn session_smf(s: &Session) -> transit_midi::Smf {
    transit_midi::Smf {
        format: 1,
        tracks: Vec::new(),
        tempos: s.tempo.tempos().iter().map(|t| (t.tick, t.bpm)).collect(),
        meters: s.tempo.meters().iter().map(|m| (m.tick, u8::try_from(m.numerator).unwrap_or(4), u8::try_from(m.denominator).unwrap_or(4))).collect(),
        markers: s.markers.iter().map(|m| (s.tempo.samples_to_ticks(m.start, s.sample_rate), m.name.clone())).collect(),
        key_sigs: Vec::new(),
    }
}

/// The session tick a MIDI file export starts at (ADR 0052 decision 10): the first tick of the bar
/// holding the session start, so the file's first bar is the pre-count zone's first bar; 0 (bar 1)
/// without a zone. The consolidated export starts its stems and MIDI files here too.
pub(super) fn export_origin(s: &Session) -> i64 {
    let start = s.start();
    if start >= 0 {
        return 0;
    }
    let tick = s.tempo.samples_to_ticks(start, s.sample_rate);
    s.tempo.bar_start_tick(s.tempo.bar_beat_at_tick(tick).bar).min(0)
}

/// The conductor written from `origin` (a tick at or before 0): the tempo and meter in effect
/// there, which are bar 1's extended backward, at file tick 0, and every later tempo, meter and
/// marker moved later by `-origin`.
fn conductor_from(s: &Session, origin: i64) -> transit_midi::Smf {
    let mut smf = session_smf(s);
    if origin >= 0 {
        return smf;
    }
    let shift = origin.saturating_neg();
    let place = |i: usize, tick: i64| if i == 0 { 0 } else { tick.saturating_add(shift) };
    for (i, t) in smf.tempos.iter_mut().enumerate() {
        t.0 = place(i, t.0);
    }
    for (i, m) in smf.meters.iter_mut().enumerate() {
        m.0 = place(i, m.0);
    }
    for m in &mut smf.markers {
        m.0 = m.0.saturating_add(shift);
    }
    smf
}

/// Every unmuted MIDI clip of the chosen tracks as it sounds ([`transit_mix::clip_as_played`]), from the
/// bar holding the session start ([`export_origin`]).
pub(super) fn export_midi_session(s: &Session, only: Option<&[TrackId]>) -> Result<Vec<u8>> {
    let origin = export_origin(s);
    let mut smf = conductor_from(s, origin);
    let chosen: Vec<_> = s.tracks.iter().filter(|t| t.kind.is_midi() && only.is_none_or(|ids| ids.contains(&t.id))).collect();
    if only.is_some() && chosen.is_empty() {
        return Err(EngineError::BadParams("file.export_midi".into(), "none of the requested tracks hold MIDI".into()));
    }
    for t in chosen {
        let mut seq = transit_midi::Sequence::default();
        // A file cannot mark a clip muted, so a muted clip is left out (it would sound wherever the
        // file is opened); a muted track is still written.
        for c in t.clips().iter().filter(|c| !c.muted && matches!(c.content, ClipContent::Midi { .. })) {
            let Some(sequence) = transit_mix::clip_as_played(s, c) else {
                return Err(EngineError::BadParams(
                    "file.export_midi".into(),
                    format!("{}: a looped MIDI clip would unroll to too many events", t.name),
                ));
            };
            let base = s.tempo.samples_to_ticks(c.start, s.sample_rate).saturating_sub(origin);
            for n in &sequence.notes {
                let mut n = *n;
                n.start = n.start.saturating_add(base);
                seq.notes.push(n);
            }
            for ev in &sequence.ctrls {
                let mut ev = *ev;
                ev.tick = ev.tick.saturating_add(base);
                seq.ctrls.push(ev);
            }
        }
        seq.sort();
        smf.tracks.push(transit_midi::SmfTrack { name: t.name.clone(), sequence: seq });
    }
    transit_midi::write_smf(&smf).map_err(|err| EngineError::Io(err.to_string()))
}

/// One track as it sounds, for a consolidated interchange package: over the consolidated range
/// ([`super::consolidated::consolidated_range`]), written from the bar holding the session start
/// with the conductor initialized at file tick 0, as the plain export is (ADR 0052 decision 10),
/// so file tick 0 is the stems' first sample.
pub(super) fn export_consolidated_midi_track(s: &Session, track: TrackId) -> Result<Vec<u8>> {
    let Some(t) = s.track(track).filter(|t| t.kind.is_midi()) else {
        return Err(EngineError::BadParams("file.bounce_consolidated".into(), "the MIDI track no longer exists".into()));
    };
    let origin = export_origin(s);
    let range = super::consolidated::consolidated_range(s);
    let Some(mut sequence) = transit_mix::sequence_as_played(s, t, range) else {
        return Err(EngineError::BadParams(
            "file.bounce_consolidated".into(),
            format!("{}: a looped MIDI clip would unroll to too many events", t.name),
        ));
    };
    if origin < 0 {
        let shift = origin.saturating_neg();
        for n in &mut sequence.notes {
            n.start = n.start.saturating_add(shift).max(0);
        }
        for ev in &mut sequence.ctrls {
            ev.tick = ev.tick.saturating_add(shift).max(0);
        }
    }
    let mut smf = conductor_from(s, origin);
    smf.tracks.push(transit_midi::SmfTrack { name: t.name.clone(), sequence });
    transit_midi::write_smf(&smf).map_err(|err| EngineError::Io(err.to_string()))
}

/// The session's tempo, meter and markers as a conductor-only SMF, from the same origin as the
/// consolidated export's other MIDI files.
pub(super) fn export_tempo_map(s: &Session) -> Result<Vec<u8>> {
    transit_midi::write_smf(&conductor_from(s, export_origin(s))).map_err(|err| EngineError::Io(err.to_string()))
}
