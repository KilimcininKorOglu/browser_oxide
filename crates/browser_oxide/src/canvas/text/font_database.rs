//! Global font database backed by `fontdb`.
//!
//! Loads the bundled TTF faces at startup (once per process) and
//! exposes a `query()` that maps a CSS font-family request + weight +
//! italic to a face ID plus the raw face bytes. Chrome-style family
//! aliases (Arial → Liberation Sans, Times → Liberation Serif, etc.)
//! are set up so a site asking for Arial gets plausible Liberation
//! Sans metrics rather than the DejaVu Sans fallback.
//!
//! The database is built from in-binary font data only — NEVER from
//! the host's system fonts — so output is reproducible across
//! machines and matches the StealthProfile's nominal Chrome-on-X
//! behaviour rather than the developer's laptop.

use fontdb::{Database, Family, Query, Stretch, Style, Weight, ID};
use std::sync::OnceLock;

pub struct FontDatabase {
    inner: Database,
    /// Family names of the bundled (Linux-style) faces. Outside a Linux
    /// persona these must not resolve by NAME: a real Mac or Windows
    /// machine has no Liberation Sans, and "detecting" one is a
    /// fingerprint tell. Generic resolution still falls back to them.
    bundled_families: std::collections::HashSet<String>,
}

/// Bundled font data. `include_bytes!` keeps these in the final binary
/// so the fingerprint is hermetic — no filesystem lookups at runtime.
///
/// The bundled set covers all four Latin style combinations (regular,
/// bold, italic, bold-italic) for sans, serif, and monospace — twelve
/// Liberation faces total, mirroring what Linux Chrome gets from
/// fontconfig. DejaVu Sans is kept as the ultimate-fallback face, and
/// Noto Sans Regular handles Cyrillic / Greek / extended Latin
/// coverage so measurements of Russian / Greek strings don't fall off
/// the Liberation glyph set.
const LIBERATION_SANS_REGULAR: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");
const LIBERATION_SANS_BOLD: &[u8] = include_bytes!("../fonts/LiberationSans-Bold.ttf");
const LIBERATION_SANS_ITALIC: &[u8] = include_bytes!("../fonts/LiberationSans-Italic.ttf");
const LIBERATION_SANS_BOLD_ITALIC: &[u8] = include_bytes!("../fonts/LiberationSans-BoldItalic.ttf");
const LIBERATION_SERIF_REGULAR: &[u8] = include_bytes!("../fonts/LiberationSerif-Regular.ttf");
const LIBERATION_SERIF_BOLD: &[u8] = include_bytes!("../fonts/LiberationSerif-Bold.ttf");
const LIBERATION_SERIF_ITALIC: &[u8] = include_bytes!("../fonts/LiberationSerif-Italic.ttf");
const LIBERATION_SERIF_BOLD_ITALIC: &[u8] =
    include_bytes!("../fonts/LiberationSerif-BoldItalic.ttf");
const LIBERATION_MONO_REGULAR: &[u8] = include_bytes!("../fonts/LiberationMono-Regular.ttf");
const LIBERATION_MONO_BOLD: &[u8] = include_bytes!("../fonts/LiberationMono-Bold.ttf");
const LIBERATION_MONO_ITALIC: &[u8] = include_bytes!("../fonts/LiberationMono-Italic.ttf");
const LIBERATION_MONO_BOLD_ITALIC: &[u8] = include_bytes!("../fonts/LiberationMono-BoldItalic.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("../fonts/DejaVuSans.ttf");
const NOTO_SANS_REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
const NOTO_EMOJI_REGULAR: &[u8] = include_bytes!("../fonts/NotoEmoji-Regular.ttf");

/// Number of faces we bundle and advertise via `document.fonts.size`.
/// Keep in sync with the `load_font_data` calls in `init_bundled`.
pub const BUNDLED_FACE_COUNT: usize = 15;

impl FontDatabase {
    pub fn get() -> &'static FontDatabase {
        static INSTANCE: OnceLock<FontDatabase> = OnceLock::new();
        INSTANCE.get_or_init(Self::init_bundled)
    }

    fn init_bundled() -> FontDatabase {
        let mut db = Database::new();
        // Load bundled faces as binary sources so fontdb can hand back
        // the raw bytes later via `face()`. The order matters for
        // default resolution — the first matching face wins when
        // multiple families carry the same name.
        db.load_font_data(LIBERATION_SANS_REGULAR.to_vec());
        db.load_font_data(LIBERATION_SANS_BOLD.to_vec());
        db.load_font_data(LIBERATION_SANS_ITALIC.to_vec());
        db.load_font_data(LIBERATION_SANS_BOLD_ITALIC.to_vec());
        db.load_font_data(LIBERATION_SERIF_REGULAR.to_vec());
        db.load_font_data(LIBERATION_SERIF_BOLD.to_vec());
        db.load_font_data(LIBERATION_SERIF_ITALIC.to_vec());
        db.load_font_data(LIBERATION_SERIF_BOLD_ITALIC.to_vec());
        db.load_font_data(LIBERATION_MONO_REGULAR.to_vec());
        db.load_font_data(LIBERATION_MONO_BOLD.to_vec());
        db.load_font_data(LIBERATION_MONO_ITALIC.to_vec());
        db.load_font_data(LIBERATION_MONO_BOLD_ITALIC.to_vec());
        db.load_font_data(DEJAVU_SANS.to_vec());
        db.load_font_data(NOTO_SANS_REGULAR.to_vec());
        db.load_font_data(NOTO_EMOJI_REGULAR.to_vec());

        // Chrome-on-Linux family aliases. When a site asks for Arial
        // (which is a Microsoft-licensed face we don't bundle), Linux
        // Chrome falls back to Liberation Sans under the hood. Matching
        // that alias chain here means `measureText("W", "16px Arial")`
        // gets realistic widths out of the box.
        //
        // Chrome's `sans-serif` / `serif` / `monospace` generics also
        // route through fontconfig on Linux to Liberation. We mirror
        // that explicitly rather than trusting fontdb's defaults.
        db.set_sans_serif_family("Liberation Sans");
        db.set_serif_family("Liberation Serif");
        db.set_monospace_family("Liberation Mono");
        db.set_cursive_family("Liberation Sans");
        db.set_fantasy_family("Liberation Sans");

        // macOS: load system fonts for real font metrics. Chrome on macOS
        // uses system fonts (Helvetica, Arial, etc.) — bundled fonts have
        // different metrics, and font measurement differences are a
        // fingerprint vector. Load system font files directly.
        #[cfg(target_os = "macos")]
        {
            // Chrome's macOS default fonts for the generic families are
            // the system faces (monospace = Courier, serif = Times,
            // sans-serif = Helvetica) — real device captures match these.
            db.set_monospace_family("Courier");
            db.set_serif_family("Times");
            db.set_sans_serif_family("Helvetica");
        }
        if cfg!(target_os = "macos") {
            for dir in [
                "/System/Library/Fonts",
                "/System/Library/Fonts/Supplemental",
                "/Library/Fonts",
            ] {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if matches!(ext, "ttf" | "otf" | "ttc") {
                            if let Ok(data) = std::fs::read(&path) {
                                db.load_font_data(data);
                            }
                        }
                    }
                }
            }
        }

        let bundled: std::collections::HashSet<String> = db
            .faces()
            .take(BUNDLED_FACE_COUNT)
            .map(|f| {
                f.families
                    .first()
                    .map(|(name, _)| name.to_lowercase())
                    .unwrap_or_default()
            })
            .collect();
        FontDatabase {
            inner: db,
            bundled_families: bundled,
        }
    }

    /// True when a BY-NAME lookup must miss: the requested family is one
    /// of our bundled Linux faces and the persona is not Linux. The emoji
    /// face is exempt — it is the internal rendering fallback.
    fn name_lookup_refused(&self, family: &str, os_name: &str) -> bool {
        if os_name == "Linux" {
            return false;
        }
        let lower = family.to_lowercase();
        if lower == "noto emoji" {
            return false;
        }
        // Chrome never resolves the color-emoji face for a measured text
        // run: a "Apple Color Emoji, monospace" probe measures like plain
        // monospace (the whole span falls through to the next author
        // family), so by-name lookups must miss the same way.
        if lower == "apple color emoji" || lower == "apple color emoji ui" {
            return true;
        }
        self.bundled_families.contains(&lower)
    }

    /// Look up a face by family name + weight + italic style. Uses
    /// fontdb's selection algorithm which includes weight-closeness and
    /// style-closeness fallbacks.
    pub fn query(&self, family: &str, weight: u16, italic: bool, os_name: &str) -> Option<ID> {
        if self.name_lookup_refused(family, os_name) {
            return None;
        }
        let style = if italic { Style::Italic } else { Style::Normal };
        let families = resolve_family(family, os_name);
        let query = Query {
            families: &families,
            weight: Weight(weight),
            stretch: Stretch::Normal,
            style,
        };
        if let Some(id) = self.inner.query(&query) {
            return Some(id);
        }
        // Final fallback: sans-serif generic. We configured this to
        // point at Liberation Sans above so a misspelled family or an
        // exotic one still renders something plausible. Outside a Linux
        // persona that fallback is refused by name (a real Mac has no
        // Liberation), so the lookup honestly misses.
        // CSS font matching is case-insensitive ("LUCIDA GRANDE" must find
        // the "Lucida Grande" face); fontdb's name match is not. The scan
        // skips bundled families the same way the by-name lookup does.
        let target = family.to_ascii_lowercase();
        let refused = |fam: &str| {
            os_name != "Linux"
                && !fam.eq_ignore_ascii_case("noto emoji")
                && self.bundled_families.contains(&fam.to_ascii_lowercase())
        };
        if let Some(f) = self.inner.faces().find(|f| {
            f.families
                .iter()
                .any(|(name, _)| !refused(name) && name.to_ascii_lowercase() == target)
        }) {
            return Some(f.id);
        }
        if self.name_lookup_refused("Liberation Sans", os_name) {
            return None;
        }
        let fallback = [Family::SansSerif];
        self.inner.query(&Query {
            families: &fallback,
            weight: Weight(weight),
            stretch: Stretch::Normal,
            style,
        })
    }

    /// Strict per-family lookup that does NOT fall back to sans-serif.
    /// Used by `query_chain` so the user-supplied fallback chain is
    /// honoured — `("Wingdings", "serif")` must reach `serif` instead of
    /// short-circuiting on Wingdings's outer fallback.
    /// Public strict lookup (used by the emoji codepoint fallback).
    pub fn query_strict_public(&self, family: &str) -> Option<ID> {
        self.query_strict(family, 400, false, "macOS")
    }

    fn query_strict(&self, family: &str, weight: u16, italic: bool, os_name: &str) -> Option<ID> {
        if self.name_lookup_refused(family, os_name) {
            return None;
        }
        let style = if italic { Style::Italic } else { Style::Normal };
        let families = resolve_family(family, os_name);
        if let Some(id) = self.inner.query(&Query {
            families: &families,
            weight: Weight(weight),
            stretch: Stretch::Normal,
            style,
        }) {
            return Some(id);
        }
        // CSS font matching is case-insensitive ("LUCIDA GRANDE" must find
        // the "Lucida Grande" face); fontdb's name match is not.
        let target = family.to_ascii_lowercase();
        self.inner
            .faces()
            .find(|f| {
                f.families
                    .iter()
                    .any(|(name, _)| name.to_ascii_lowercase() == target)
            })
            .map(|f| f.id)
    }

    /// First-match query across a family fallback chain. Tries each
    /// user-specified family in order; only after the entire chain is
    /// exhausted does the global sans-serif fallback fire.
    pub fn query_chain(
        &self,
        families: &[String],
        weight: u16,
        italic: bool,
        os_name: &str,
    ) -> Option<ID> {
        for fam in families {
            if let Some(id) = self.query_strict(fam, weight, italic, os_name) {
                return Some(id);
            }
        }
        // Whole chain unresolvable → fall back to the DEFAULT font of the
        // generic context the chain names. Real Chrome resolves a missing
        // family to the context's default face (a missing name inside a
        // monospace list measures like Courier, not Helvetica), so the
        // fallback must follow the first generic keyword in the chain.
        let style = if italic { Style::Italic } else { Style::Normal };
        let fallback_generic = families.iter().find_map(|f| match f.as_str() {
            "monospace" => Some(Family::Monospace),
            "serif" => Some(Family::Serif),
            "sans-serif" => Some(Family::SansSerif),
            _ => None,
        });
        let fallback = [fallback_generic.unwrap_or(Family::SansSerif)];
        self.inner.query(&Query {
            families: &fallback,
            weight: Weight(weight),
            stretch: Stretch::Normal,
            style,
        })
    }

    /// Return the raw face bytes + face index for a given face ID.
    /// Returns `None` for faces backed by a file source (we only ever
    /// load binary sources, so this is effectively infallible, but the
    /// `fontdb::Source` enum forces us to handle both).
    /// The primary family name of a face id.
    pub fn family_of(&self, id: ID) -> Option<String> {
        self.inner
            .face(id)
            .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
    }

    /// Debug: every stored family name containing `needle` (case-insensitive).
    pub fn families_matching(&self, needle: &str) -> Vec<String> {
        let lower = needle.to_lowercase();
        let mut out = Vec::new();
        for f in self.inner.faces() {
            for (name, _) in &f.families {
                if name.to_lowercase().contains(&lower) && !out.contains(name) {
                    out.push(name.clone());
                }
            }
        }
        out
    }

    pub fn face_data(&self, id: ID) -> Option<(&[u8], u32)> {
        let face = self.inner.face(id)?;
        // `init_bundled` only loads binary sources, so `face.source`
        // is always `Binary`. The destructure is still necessary to
        // extract the Arc'd bytes.
        let fontdb::Source::Binary(data) = &face.source;
        // `data` is an `Arc<dyn AsRef<[u8]> + Send + Sync>`. The Arc's
        // contents are immutable for the process lifetime, so the
        // slice is safe to hand out with the database's lifetime.
        let slice: &[u8] = (**data).as_ref();
        Some((slice, face.index))
    }
}

/// Resolve a CSS family name to a list of fontdb `Family` entries,
/// handling generic keywords and Chrome-style aliases.
///
/// Unknown families return ONLY the literal name — no implicit
/// `Family::SansSerif` fallback. The outer fallback path (in `query`)
/// catches the case where the user-supplied chain is fully
/// unresolvable; `query_chain` relies on the per-family failure to
/// proceed to the user's next explicit fallback (e.g. `"Wingdings",
/// serif` should resolve to a serif face, not silently to sans-serif).
/// Without this, challenge-vendor-style font detection probes
/// (`Math.abs(measure("X, serif") - measure("serif")) > 0`) would
/// register every unknown family as installed.
fn resolve_family<'a>(name: &'a str, os_name: &str) -> Vec<Family<'a>> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "sans-serif" => vec![Family::SansSerif],
        "serif" => vec![Family::Serif],
        "monospace" => vec![Family::Monospace],
        "cursive" => vec![Family::Cursive],
        "fantasy" => vec![Family::Fantasy],
        // Chrome substitution table for families we don't bundle.
        // On macOS we now load system fonts, so resolve to the REAL font
        // name (Chrome uses CoreText with system fonts on macOS too).
        // On other platforms map to the bundled Liberation set.
        "arial" | "helvetica" | "helvetica neue" | "tahoma" | "verdana" | "segoe ui"
        | "calibri" => {
            if os_name == "macOS" {
                // macOS has real system fonts — use them directly
                vec![Family::Name(name)]
            } else {
                vec![Family::Name("Liberation Sans"), Family::SansSerif]
            }
        }
        "times" | "times new roman" | "georgia" => {
            if os_name == "macOS" {
                // A real Mac resolves these by name (Supplemental holds
                // Times New Roman and Georgia) — no Linux substitution.
                vec![Family::Name(name)]
            } else {
                vec![Family::Name("Liberation Serif"), Family::Serif]
            }
        }
        "courier" | "courier new" | "consolas" | "menlo" | "monaco" => {
            if os_name == "macOS" {
                vec![Family::Name(name)]
            } else {
                vec![Family::Name("Liberation Mono"), Family::Monospace]
            }
        }
        _ => vec![Family::Name(name)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_sans_serif_generic() {
        let db = FontDatabase::get();
        assert!(db.query("sans-serif", 400, false, "Linux").is_some());
    }

    #[test]
    fn resolves_arial_alias_to_liberation_sans() {
        let db = FontDatabase::get();
        let arial_id = db
            .query("Arial", 400, false, "Linux")
            .expect("Arial should resolve");
        let libsans_id = db
            .query("Liberation Sans", 400, false, "Linux")
            .expect("Liberation Sans should resolve");
        assert_eq!(
            arial_id, libsans_id,
            "Arial alias should map to the bundled Liberation Sans face"
        );
    }

    #[test]
    fn resolves_times_alias_to_liberation_serif() {
        let db = FontDatabase::get();
        let id = db
            .query("Times New Roman", 400, false, "Linux")
            .expect("Times should resolve");
        let libserif_id = db
            .query("Liberation Serif", 400, false, "Linux")
            .expect("Liberation Serif should resolve");
        assert_eq!(id, libserif_id);
    }

    #[test]
    fn resolves_bold_weight() {
        let db = FontDatabase::get();
        let reg = db.query("Arial", 400, false, "Linux").unwrap();
        let bold = db.query("Arial", 700, false, "Linux").unwrap();
        assert_ne!(
            reg, bold,
            "regular and bold Arial should map to different faces"
        );
    }

    #[test]
    fn fallback_when_family_unknown() {
        let db = FontDatabase::get();
        // Should still return something via the sans-serif fallback.
        assert!(db
            .query("NonexistentFamily42", 400, false, "Linux")
            .is_some());
    }

    #[test]
    fn face_data_returns_bytes() {
        let db = FontDatabase::get();
        let id = db.query("Arial", 400, false, "Linux").unwrap();
        let (bytes, _idx) = db.face_data(id).expect("bundled face has binary source");
        assert!(
            bytes.len() > 1000,
            "face bytes suspiciously small: {}",
            bytes.len()
        );
        // TTF magic is either 0x00010000 (TrueType) or "OTTO" (CFF).
        assert!(
            matches!(&bytes[0..4], b"\x00\x01\x00\x00" | b"OTTO" | b"true"),
            "not a TTF: first 4 bytes = {:?}",
            &bytes[0..4]
        );
    }

    #[test]
    fn resolves_italic_sans() {
        let db = FontDatabase::get();
        let reg = db.query("Arial", 400, false, "Linux").unwrap();
        let italic = db.query("Arial", 400, true, "Linux").unwrap();
        assert_ne!(
            reg, italic,
            "italic Arial should map to Liberation Sans Italic"
        );
    }

    #[test]
    fn resolves_bold_italic_serif() {
        let db = FontDatabase::get();
        let reg = db.query("Times New Roman", 400, false, "Linux").unwrap();
        let bi = db.query("Times New Roman", 700, true, "Linux").unwrap();
        assert_ne!(
            reg, bi,
            "bold-italic Times should map to Liberation Serif Bold Italic"
        );
    }

    #[test]
    fn bundled_face_count_at_least_advertised() {
        // System fonts are loaded on macOS for real font metrics —
        // the database now contains bundled + system faces. The count
        // must be at least the bundled count.
        let db = FontDatabase::get();
        let loaded = db.inner.faces().count();
        assert!(
            loaded >= BUNDLED_FACE_COUNT,
            "fontdb loaded {loaded} faces, expected at least {BUNDLED_FACE_COUNT}"
        );
    }

    #[test]
    fn query_chain_walks_fallback() {
        let db = FontDatabase::get();
        let chain = vec![
            "NonexistentA".to_string(),
            "NonexistentB".to_string(),
            "Arial".to_string(),
        ];
        let id = db.query_chain(&chain, 400, false, "Linux").unwrap();
        let arial_id = db.query("Arial", 400, false, "Linux").unwrap();
        assert_eq!(id, arial_id);
    }
}
