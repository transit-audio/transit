//! Clip List commands.

use super::sampler_kit_presets::{Holder, holders};
use super::*;
use crate::cmd;
use serde_json::json;
use std::collections::BTreeSet;
use transit_model::SourceId;

const CLEAR: &str = "clip.clear";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "clip.clear",
        "Clear Clips",
        [],
        Some("Cmd+Shift+B"),
        "{clips?: [id], sources?: [id], with_clips?: bool} — removes clips and audio files from the session (files stay on disk); default: the Clip List selection. A file still used on the timeline needs `with_clips: true`, which removes those clips too; a file a Sampler, a freeze or the clipboard still uses is refused (the whole request), naming them",
        has_list_selection,
        clear
    )]
}

fn has_list_selection(e: &Engine) -> std::result::Result<(), String> {
    let ed = &e.session().edit;
    if !e.command_targets.selection && !e.command_targets.sources && ed.selected_clips.is_empty() && ed.selected_sources.is_empty() {
        Err("select clips or files in the Clip List".into())
    } else {
        Ok(())
    }
}

fn ids<'a, T: serde::Deserialize<'a>>(p: &'a Value, key: &str) -> Result<Option<T>> {
    p.get(key).map(|v| T::deserialize(v).map_err(|_| bad(CLEAR, format!("`{key}` must be an array of ids")))).transpose()
}

fn clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session();
    let (clips, sources): (BTreeSet<ClipId>, BTreeSet<SourceId>) = match (ids(p, "clips")?, ids(p, "sources")?) {
        (None, None) => (s.edit.selected_clips.iter().copied().collect(), s.edit.selected_sources.iter().copied().collect()),
        (c, f) => (c.unwrap_or_default(), f.unwrap_or_default()),
    };
    let known: BTreeSet<ClipId> = s.all_clips().map(|c| c.id).collect();
    if let Some(c) = clips.iter().find(|c| !known.contains(c)) {
        return Err(bad(CLEAR, format!("no clip with id {c}")));
    }
    let files = sources.iter().map(|f| s.source(*f).ok_or_else(|| bad(CLEAR, format!("no audio file with id {f}")))).collect::<Result<Vec<_>>>()?;
    if clips.is_empty() && files.is_empty() {
        return Err(bad(CLEAR, "nothing to clear: pass `clips` or `sources`, or select them in the Clip List"));
    }
    // The Sampler's own rule for who still names a file (#488): nothing here clears those holders.
    if let Some((f, held)) = files.iter().map(|f| (f, holders(e, f.id))).find(|(_, h)| !h.is_empty()) {
        return Err(bad(CLEAR, still_held(&f.name, &held)));
    }
    if !bool_or(p, "with_clips", false)
        && let Some((f, n)) = files.iter().map(|f| (f, s.clips_using(f.id).filter(|c| !clips.contains(&c.id)).count())).find(|(_, n)| *n > 0)
    {
        let plural = if n == 1 { "" } else { "s" };
        return Err(bad(CLEAR, format!("`{}` is used by {n} clip{plural} on the timeline; pass `with_clips: true` to remove them too", f.name)));
    }
    let gone: BTreeSet<ClipId> = clips.iter().copied().chain(sources.iter().flat_map(|f| s.clips_using(*f).map(|c| c.id))).collect();
    let s = e.session_mut();
    for pl in s.tracks.iter_mut().flat_map(|t| t.playlists.iter_mut()) {
        pl.clips.retain(|c| !gone.contains(&c.id));
    }
    s.sources.retain(|f| !sources.contains(&f.id));
    for f in &sources {
        s.pool.remove(*f);
    }
    s.edit.selected_clips.retain(|c| !gone.contains(c));
    s.edit.selected_sources.retain(|f| !sources.contains(f));
    Ok(json!({"clips": gone.len(), "sources": sources.len()}))
}

/// The refusal for file `name` still named by `held`: each kind of holder, its tracks by name.
fn still_held(name: &str, held: &[Holder]) -> String {
    let tracks = |want: fn(&Holder) -> Option<&String>| held.iter().filter_map(want).map(|t| format!("`{t}`")).collect::<Vec<_>>();
    let samplers = tracks(|h| if let Holder::Sampler(t) = h { Some(t) } else { None });
    let frozen = tracks(|h| if let Holder::Freeze(t) = h { Some(t) } else { None });
    let mut parts = Vec::new();
    match samplers.as_slice() {
        [] => {}
        [t] => parts.push(format!("the Sampler on track {t}")),
        ts => parts.push(format!("the Samplers on tracks {}", ts.join(", "))),
    }
    match frozen.as_slice() {
        [] => {}
        [t] => parts.push(format!("the frozen track {t}")),
        ts => parts.push(format!("the frozen tracks {}", ts.join(", "))),
    }
    let clipboard = held.contains(&Holder::Clipboard);
    if clipboard {
        parts.push("the clipboard".to_string());
    }
    let list = match parts.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    };
    let copy = if clipboard { " (copy something else to replace the clipboard)" } else { "" };
    format!("`{name}` is still used by {list}; remove it there first{copy}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use transit_audio_io::{AudioBuffer, FileFormat};
    use transit_model::{Clip, TrackId};

    fn kick(e: &Engine) -> (SourceId, Vec<ClipId>) {
        let s = e.session();
        let f = s.sources.iter().find(|x| x.name == "Kick").map(|x| x.id).unwrap();
        (f, s.clips_using(f).map(|c| c.id).collect())
    }

    #[test]
    fn clearing_a_clip_takes_it_off_the_timeline_and_keeps_its_file() {
        let mut e = crate::demo::demo_engine();
        let (f, clips) = kick(&e);
        let r = e.execute("clip.clear", &json!({"clips": [clips[0]]})).unwrap();
        assert_eq!(r, json!({"clips": 1, "sources": 0}));
        assert!(e.session().find_clip(clips[0]).is_none());
        assert!(e.session().source(f).is_some());
        assert!(e.undo());
        assert!(e.session().find_clip(clips[0]).is_some());
    }

    #[test]
    fn a_file_in_use_stays_unless_its_clips_go_too() {
        let mut e = crate::demo::demo_engine();
        e.execute("track.playlist_duplicate", &json!({"track": "Kick"})).unwrap();
        let (f, clips) = kick(&e);
        assert_eq!(clips.len(), 6);
        let err = e.execute("clip.clear", &json!({"sources": [f]})).unwrap_err();
        assert!(err.to_string().contains("used by 6 clips"), "{err}");
        let r = e.execute("clip.clear", &json!({"sources": [f], "with_clips": true})).unwrap();
        assert_eq!(r, json!({"clips": 6, "sources": 1}));
        let s = e.session();
        assert!(s.source(f).is_none() && !s.pool.contains(f));
        assert_eq!(s.clips_using(f).count(), 0);
        assert_eq!(e.undo_label(), Some("Clear Clips"));
        assert!(e.undo());
        assert!(e.session().pool.contains(f));
        assert_eq!(e.session().clips_using(f).count(), 6);
    }

    #[test]
    fn listing_a_files_clips_too_is_enough() {
        let mut e = crate::demo::demo_engine();
        let (f, clips) = kick(&e);
        let r = e.execute("clip.clear", &json!({"clips": clips, "sources": [f]})).unwrap();
        assert_eq!(r, json!({"clips": 3, "sources": 1}));
    }

    #[test]
    fn the_clip_list_selection_is_the_default_target() {
        let mut e = crate::demo::demo_engine();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4_800]] };
        let f = crate::io::add_source(e.session_mut(), "Spare", buf, None, FileFormat::Wav);
        let (_, clips) = kick(&e);
        e.execute("edit.select", &json!({"clips": [clips[0]]})).unwrap();
        let r = e.execute("edit.select", &json!({"sources": [f, f, 999_999]})).unwrap();
        assert_eq!(r["sources"], json!([f]));
        assert_eq!(r["clips"], json!([]));
        let r = e.execute("edit.select", &json!({"clips": [clips[0]]})).unwrap();
        assert_eq!(r["sources"], json!([]));
        e.execute("clip.clear", &json!({})).unwrap();
        assert!(e.session().find_clip(clips[0]).is_none());
        assert!(e.session().edit.selected_clips.is_empty());
        e.execute("edit.select", &json!({"sources": [f]})).unwrap();
        e.execute("clip.clear", &json!({})).unwrap();
        assert!(e.session().source(f).is_none());
        assert!(e.session().edit.selected_sources.is_empty());
    }

    #[test]
    fn bad_targets_are_errors() {
        let mut e = crate::demo::demo_engine();
        for p in [
            json!({}),
            json!({"clips": []}),
            json!({"clips": [999_999]}),
            json!({"sources": [999_999]}),
            json!({"sources": "Kick"}),
            json!({"clips": [-1]}),
        ] {
            assert!(e.execute("clip.clear", &p).is_err(), "{p}");
        }
    }

    /// An instrument track with a new Sampler patch, a source used by nothing (`Spare`) and another
    /// (`Kit`) for the holder under test to name.
    fn sampler_and_files() -> (Engine, TrackId, String, SourceId, SourceId) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
        let tr = e.session().tracks.last().unwrap();
        let (t, name) = (tr.id, tr.name.clone());
        e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
        let buf = || AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4_800]] };
        let spare = crate::io::add_source(e.session_mut(), "Spare", buf(), None, FileFormat::Wav);
        let kit = crate::io::add_source(e.session_mut(), "Kit", buf(), None, FileFormat::Wav);
        (e, t, name, spare, kit)
    }

    /// Clears `p`, expecting a refusal that contains `says`, with the document, the pool and the
    /// undo history untouched.
    fn refused(e: &mut Engine, p: Value, says: &str) {
        let before = e.session_arc();
        let doc = serde_json::to_value(e.session()).unwrap();
        let steps = e.undo_history().len();
        let err = e.execute("clip.clear", &p).unwrap_err().to_string();
        assert!(err.contains(says), "{err}");
        assert!(Arc::ptr_eq(&before, &e.session_arc()), "nothing changed");
        assert_eq!(serde_json::to_value(e.session()).unwrap(), doc);
        assert_eq!(e.undo_history().len(), steps, "no undo step");
    }

    #[test]
    fn a_file_on_a_sampler_pad_is_refused() {
        let (mut e, t, name, _, kit) = sampler_and_files();
        e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "source": kit})).unwrap();
        let says = format!("`Kit` is still used by the Sampler on track `{name}`; remove it there first");
        refused(&mut e, json!({"sources": [kit]}), &says);
        // `with_clips` speaks for timeline clips only.
        refused(&mut e, json!({"sources": [kit], "with_clips": true}), &says);
        // The Clip List selection as the target: the same refusal.
        e.execute("edit.select", &json!({"sources": [kit]})).unwrap();
        refused(&mut e, json!({}), &says);
        assert!(e.session().pool.contains(kit));
    }

    #[test]
    fn a_file_only_the_slice_loop_uses_is_refused() {
        let (mut e, t, name, _, kit) = sampler_and_files();
        e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": kit})).unwrap();
        let p = e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().ready().unwrap().clone();
        assert!(p.zones.is_empty() && p.slicing.is_some(), "only the loop names it");
        refused(&mut e, json!({"sources": [kit]}), &format!("used by the Sampler on track `{name}`"));
    }

    #[test]
    fn a_file_an_opaque_patch_names_is_refused() {
        let (mut e, t, name, _, kit) = sampler_and_files();
        e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap().sampler_patch =
            Some(transit_model::SamplerPatch::from_value(json!({"schema": 9, "sources": [{"source": kit.0}]})));
        refused(&mut e, json!({"sources": [kit]}), &format!("used by the Sampler on track `{name}`"));
    }

    #[test]
    fn a_frozen_tracks_file_is_refused_and_every_holder_is_named() {
        let (mut e, t, name, _, kit) = sampler_and_files();
        let frozen = transit_model::Frozen { source: kit, start: 0, length: 4_800, hash: 0, revision: 0, latency: 0 };
        e.session_mut().track_mut(t).unwrap().freeze = Some(frozen.clone());
        refused(&mut e, json!({"sources": [kit]}), &format!("`Kit` is still used by the frozen track `{name}`; remove it there first"));
        e.session_mut().track_mut(t).unwrap().freeze = None;
        e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "source": kit})).unwrap();
        e.session_mut().track_mut(t).unwrap().freeze = Some(frozen);
        e.clipboard.tracks = vec![vec![Clip::audio(ClipId(1), "Kit", kit, 0, 0, 4_800)]];
        refused(
            &mut e,
            json!({"sources": [kit]}),
            &format!(
                "`Kit` is still used by the Sampler on track `{name}`, the frozen track `{name}` and the clipboard; remove it there first (copy something else to replace the clipboard)"
            ),
        );
    }

    #[test]
    fn a_file_on_the_clipboard_is_refused_even_with_its_clips() {
        let mut e = crate::demo::demo_engine();
        let (f, clips) = kick(&e);
        e.execute("edit.select", &json!({"clips": [clips[0]]})).unwrap();
        let copied = e.session().find_clip(clips[0]).unwrap().1.clone();
        e.clipboard.tracks = vec![vec![copied]];
        refused(&mut e, json!({"clips": clips, "sources": [f], "with_clips": true}), "`Kick` is still used by the clipboard");
        e.clipboard = crate::Clipboard::default();
        let r = e.execute("clip.clear", &json!({"sources": [f], "with_clips": true})).unwrap();
        assert_eq!(r, json!({"clips": 3, "sources": 1}));
    }

    #[test]
    fn one_held_file_refuses_the_whole_request_and_a_free_file_still_clears() {
        let (mut e, t, name, spare, kit) = sampler_and_files();
        e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "source": kit})).unwrap();
        refused(&mut e, json!({"sources": [spare, kit]}), &format!("`Kit` is still used by the Sampler on track `{name}`"));
        assert!(e.session().source(spare).is_some() && e.session().pool.contains(spare), "nothing partial");
        let r = e.execute("clip.clear", &json!({"sources": [spare]})).unwrap();
        assert_eq!(r, json!({"clips": 0, "sources": 1}));
        assert!(e.session().source(spare).is_none() && !e.session().pool.contains(spare));
        assert_eq!(e.undo_label(), Some("Clear Clips"));
    }

    #[test]
    fn deselect_all_clears_the_clip_list_selection() {
        let mut e = crate::demo::demo_engine();
        let (f, _) = kick(&e);
        e.execute("edit.select", &json!({"sources": [f]})).unwrap();
        e.execute("edit.select_none", &json!({})).unwrap();
        assert!(e.session().edit.selected_sources.is_empty());
    }
}
