//! Import donor tracks and remap their session-local ids and media paths.

use super::*;
use crate::cmd::bad;
use transit_model::Route;

pub(crate) struct ImportedSessionData {
    pub tracks: Vec<TrackId>,
    pub sources: usize,
    pub missing: Vec<String>,
}

pub(crate) fn import_session_data(e: &mut Engine, path: &str, names: Option<Vec<String>>) -> Result<ImportedSessionData> {
    let id = "file.import_session_data";
    let text = read_text(path)?;
    let mut other = Session::from_json(&text).map_err(|err| bad(id, format!("{path}: {err}")))?;
    let donor_rate = std::mem::replace(&mut other.sample_rate, e.session().sample_rate);
    let frozen_notes = super::frozen_recording::load(&mut other, Path::new(path));
    other.sample_rate = donor_rate;
    if other.load_notes.iter().any(|n| n == transit_model::legacy::VIDEO_REMOVED_NOTE) {
        e.message(transit_model::legacy::VIDEO_REMOVED_NOTE.to_string());
    }
    let picked: Vec<transit_model::Track> = match &names {
        Some(ns) => {
            let mut v = Vec::new();
            for n in ns {
                v.push(other.track_by_name(n).cloned().ok_or_else(|| bad(id, format!("`{path}` has no track named `{n}`")))?);
            }
            v
        }
        None => other.tracks.iter().filter(|t| t.kind != TrackKind::Master).cloned().collect(),
    };
    if picked.is_empty() {
        return Err(bad(id, "nothing to import"));
    }
    // A donor track that had the retired one-sample Sampler came in as an empty kit on parse
    // (#455): the donor's own line for it, under the donor's name, and the summary.
    let dropped: Vec<String> =
        picked.iter().filter_map(|t| other.load_notes.iter().find(|n| transit_model::legacy_sampler::is_track_line(n, &t.name))).cloned().collect();
    if !dropped.is_empty() {
        let n = dropped.len();
        for line in dropped {
            e.message(line);
        }
        e.message(transit_model::legacy_sampler::summary(n));
    }
    let mut needed: Vec<SourceId> =
        picked.iter().flat_map(|t| t.playlists.iter().flat_map(|pl| pl.clips.iter().filter_map(|c| c.source()))).collect();
    // A Sampler patch's sources are session audio too (ADR 0041), an opaque patch's included.
    needed.extend(
        picked.iter().filter_map(|t| t.instrument.as_ref().and_then(|i| i.sampler_patch.as_ref())).flat_map(transit_model::SamplerPatch::media),
    );
    needed.sort_by_key(|x| x.0);
    needed.dedup();
    let sr = e.session().sample_rate.hz();
    let s = e.session_mut();
    let mut files = crate::io::DecodedFiles::new();
    let mut map: std::collections::BTreeMap<u64, SourceId> = Default::default();
    let mut missing = frozen_notes;
    let sampler_sources = super::session::sampler_sources(&other);
    for src in needed.iter().filter_map(|sid| other.source(*sid)) {
        let rate = if sampler_sources.contains(&src.id) { src.sample_rate } else { sr };
        let mut ns = src.clone();
        ns.id = SourceId(s.alloc());
        match crate::io::stored_path(&src.path, Path::new(&path)) {
            Ok(abs) => {
                ns.path = abs.to_string_lossy().into_owned();
                match crate::io::decode_file_streamed(&mut files, &abs, rate, src.disk) {
                    Ok(audio) => {
                        ns.frames = audio.frames() as u64;
                        ns.disk = audio.is_disk();
                        ns.sample_rate = rate;
                        // A disk-backed source plays its file where it is; anything else is copied on save.
                        ns.unsaved = !ns.disk;
                        s.pool.insert(ns.id, audio);
                    }
                    Err(_) => {
                        ns.unsaved = false;
                        missing.push(ns.path.clone());
                    }
                }
            }
            Err(why) => {
                ns.unsaved = false;
                missing.push(format!("{}: {why}", ns.path));
            }
        }
        map.insert(src.id.0, ns.id);
        s.sources.push(ns);
    }
    let mut out = Vec::new();
    for mut tr in picked {
        tr.id = TrackId(s.alloc());
        tr.name = s.unique_track_name(&tr.name);
        tr.folder = None;
        tr.mixer.vca = None;
        for pl in &mut tr.playlists {
            for c in &mut pl.clips {
                c.id = s.new_clip_id();
                if let ClipContent::Audio { source, .. } = &mut c.content
                    && let Some(n) = map.get(&source.0)
                {
                    *source = *n;
                }
            }
        }
        // The donor's source ids mean nothing here.
        if let Some(ins) = tr.instrument.as_mut() {
            // A patch keeps every reference: one that did not come along gets an id of its own that
            // names nothing here (silent, reported), never another source of this session.
            if let Some(patch) = ins.sampler_patch.as_mut() {
                patch.remap_media(|sid| map.get(&sid.0).copied().unwrap_or_else(|| SourceId(s.alloc())));
            }
        }
        // Busses are matched by name (created when missing).
        let remap = |s: &mut Session, r: &Route| -> Route {
            match r {
                Route::Bus(b) => other.bus(*b).map_or(Route::None, |ob| Route::Bus(s.add_bus(&ob.name, ob.format))),
                x => x.clone(),
            }
        };
        tr.mixer.input = remap(s, &tr.mixer.input);
        tr.mixer.output = remap(s, &tr.mixer.output);
        for slot in tr.mixer.sends.iter_mut().flatten() {
            slot.target = remap(s, &slot.target);
        }
        let pos = s.tracks.iter().position(|t| t.kind == TrackKind::Master).unwrap_or(s.tracks.len());
        out.push(tr.id);
        s.tracks.insert(pos, tr);
    }
    s.edit.selected_tracks = out.clone();
    // An imported hardware insert whose send path is taken here comes in disabled.
    let disabled = transit_model::disable_hardware_conflicts(s);
    e.messages.extend(disabled);
    Ok(ImportedSessionData { tracks: out, sources: map.len(), missing })
}
