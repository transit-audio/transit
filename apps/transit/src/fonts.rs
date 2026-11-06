//! The bundled faces (ADR 0045, amending ADR 0010): IBM Plex Sans for interface text and IBM Plex
//! Mono for every number and readout, unmodified from the IBM Plex release under the SIL OFL 1.1.
//! The design's type roles name the families registered here (`transit_ui_studio::tokens`); the
//! shared definitions (egui's built-in fonts, the system fonts, the Hangul fallback) stay behind
//! each family.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;
use transit_ui_studio::tokens::{FAMILY_MONO, FAMILY_MONO_MEDIUM, FAMILY_SANS, FAMILY_SANS_MEDIUM, FAMILY_SANS_SEMIBOLD};

macro_rules! font {
    ($path:literal) => {
        Arc::new(FontData::from_static(include_bytes!(concat!("../assets/fonts/", $path))))
    };
}

/// (face name, file): the five faces the type roles use.
fn faces() -> [(&'static str, Arc<FontData>); 5] {
    [
        ("plex-sans-regular", font!("ibm-plex-sans/IBMPlexSans-Regular.ttf")),
        ("plex-sans-medium", font!("ibm-plex-sans/IBMPlexSans-Medium.ttf")),
        ("plex-sans-semibold", font!("ibm-plex-sans/IBMPlexSans-SemiBold.ttf")),
        ("plex-mono-regular", font!("ibm-plex-mono/IBMPlexMono-Regular.ttf")),
        ("plex-mono-medium", font!("ibm-plex-mono/IBMPlexMono-Medium.ttf")),
    ]
}

/// Plex for the UI and the numbers, the weights as named families (`bold` stays, as an alias of
/// the semibold family, for the code that already names it), the shared definitions behind them.
pub fn definitions() -> FontDefinitions {
    let mut defs = transit_ui_studio::fonts::definitions();
    for (name, data) in faces() {
        defs.font_data.insert(name.into(), data);
    }
    // Each family: its face first, then whatever was there (fallbacks for emoji, icons, Hangul).
    let mut put = |family: FontFamily, face: &str| {
        let rest = defs.families.get(&family).cloned().unwrap_or_default();
        let mut list = vec![face.to_string()];
        list.extend(rest.into_iter().filter(|f| f != face));
        defs.families.insert(family, list);
    };
    put(FontFamily::Proportional, "plex-sans-regular");
    put(FontFamily::Monospace, "plex-mono-regular");
    let proportional = defs.families[&FontFamily::Proportional].clone();
    let monospace = defs.families[&FontFamily::Monospace].clone();
    let mut named = |name: &str, face: &str, base: &[String]| {
        let mut list = vec![face.to_string()];
        list.extend(base.iter().filter(|f| *f != face).cloned());
        defs.families.insert(FontFamily::Name(name.into()), list);
    };
    named(FAMILY_SANS, "plex-sans-regular", &proportional);
    named(FAMILY_SANS_MEDIUM, "plex-sans-medium", &proportional);
    named(FAMILY_SANS_SEMIBOLD, "plex-sans-semibold", &proportional);
    named(FAMILY_MONO, "plex-mono-regular", &monospace);
    named(FAMILY_MONO_MEDIUM, "plex-mono-medium", &monospace);
    named("bold", "plex-sans-semibold", &proportional);
    defs
}

#[cfg(test)]
mod tests {
    use super::*;
    use transit_ui_studio::tokens;

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions());
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| transit_ui_studio::theme::probe_fonts(ui.ctx()));
        out.textures_delta.clear();
        // The fonts apply from the next frame: probe again so the families are seen.
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| transit_ui_studio::theme::probe_fonts(ui.ctx()));
        out.textures_delta.clear();
        ctx
    }

    #[test]
    fn every_type_role_family_is_registered_with_its_plex_face_first() {
        let defs = definitions();
        for (family, face) in [
            (FAMILY_SANS, "plex-sans-regular"),
            (FAMILY_SANS_MEDIUM, "plex-sans-medium"),
            (FAMILY_SANS_SEMIBOLD, "plex-sans-semibold"),
            (FAMILY_MONO, "plex-mono-regular"),
            (FAMILY_MONO_MEDIUM, "plex-mono-medium"),
            ("bold", "plex-sans-semibold"),
        ] {
            let list = &defs.families[&FontFamily::Name(family.into())];
            assert_eq!(list.first().map(String::as_str), Some(face), "{family}");
            assert!(list.len() > 1, "{family} keeps its fallbacks");
        }
        assert_eq!(defs.families[&FontFamily::Proportional][0], "plex-sans-regular");
        assert_eq!(defs.families[&FontFamily::Monospace][0], "plex-mono-regular");
        for (_, role) in tokens::TYPE_ROLES {
            assert!(tokens::FAMILIES.contains(&role.family_name()), "{role:?}");
        }
    }

    #[test]
    fn the_type_roles_resolve_to_the_installed_families() {
        let ctx = ctx();
        assert_eq!(tokens::VALUE.font_id(&ctx).family, FontFamily::Name(FAMILY_MONO_MEDIUM.into()));
        assert_eq!(tokens::READOUT_DISPLAY.font_id(&ctx).family, FontFamily::Name(FAMILY_MONO.into()));
        assert_eq!(tokens::LABEL_SECTION.font_id(&ctx).family, FontFamily::Name(FAMILY_SANS_SEMIBOLD.into()));
        assert_eq!(tokens::LABEL_CONTROL.font_id(&ctx).family, FontFamily::Name(FAMILY_SANS.into()));
        assert_eq!(tokens::LABEL_HERO.font_id(&ctx).family, FontFamily::Name(FAMILY_SANS_MEDIUM.into()));
        assert_eq!(transit_ui_studio::theme::mono(&ctx, 12.0).family, FontFamily::Name(FAMILY_MONO_MEDIUM.into()));
    }

    /// Readouts must not jitter: Plex Sans and Plex Mono both draw every digit at one width, with
    /// no feature freezing (egui does no OpenType shaping).
    #[test]
    fn digits_are_tabular_in_every_bundled_face() {
        let ctx = ctx();
        for family in tokens::FAMILIES {
            let font = egui::FontId::new(13.0, FontFamily::Name(family.into()));
            let widths: Vec<f32> = ctx.fonts_mut(|f| "0123456789".chars().map(|c| f.glyph_width(&font, c)).collect());
            assert!(widths.iter().all(|w| (w - widths[0]).abs() < 1e-3), "{family}: {widths:?}");
        }
    }
}
