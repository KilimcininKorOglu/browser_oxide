//! Real font stack for Canvas 2D text rendering.
//!
//! Pipeline: [`font_shorthand`] parses `ctx.font` strings, [`FontDatabase`]
//! resolves families to concrete faces via the bundled Liberation font
//! set, [`shaper`] produces positioned glyph runs via rustybuzz, and
//! [`raster`] rasterizes individual glyphs via swash. The top-level
//! [`rasterize_text`] / [`measure_text_metrics`] helpers are the
//! entry points `Canvas2D` calls.
//!
//! Replaces the prior ab_glyph-based stub that returned fixed-width
//! metrics regardless of the requested font-family and rendered
//! through a single embedded DejaVu Sans. The new pipeline is
//! Chrome-compatible enough that `measureText("...")` responses on
//! fingerprint-sensitive sites fall within the expected range.

pub mod font_database;
pub mod font_shorthand;
pub mod raster;
pub mod shaper;

pub use font_database::FontDatabase;
pub use font_shorthand::ParsedFont;
pub use raster::GlyphBitmap;
pub use shaper::ShapedRun;

/// Full 13-field `TextMetrics` object as Canvas 2D exposes it.
///
/// Field semantics follow the HTML spec:
/// <https://html.spec.whatwg.org/multipage/canvas.html#textmetrics>
#[derive(Debug, Clone, PartialEq)]
pub struct TextMetrics {
    pub width: f32,
    pub actual_bounding_box_left: f32,
    pub actual_bounding_box_right: f32,
    pub actual_bounding_box_ascent: f32,
    pub actual_bounding_box_descent: f32,
    pub font_bounding_box_ascent: f32,
    pub font_bounding_box_descent: f32,
    pub em_height_ascent: f32,
    pub em_height_descent: f32,
    pub hanging_baseline: f32,
    pub alphabetic_baseline: f32,
    pub ideographic_baseline: f32,
}

impl TextMetrics {
    /// Zero-valued metrics for empty text / unresolved fonts.
    pub fn zero() -> Self {
        Self {
            width: 0.0,
            actual_bounding_box_left: 0.0,
            actual_bounding_box_right: 0.0,
            actual_bounding_box_ascent: 0.0,
            actual_bounding_box_descent: 0.0,
            font_bounding_box_ascent: 0.0,
            font_bounding_box_descent: 0.0,
            em_height_ascent: 0.0,
            em_height_descent: 0.0,
            hanging_baseline: 0.0,
            alphabetic_baseline: 0.0,
            ideographic_baseline: 0.0,
        }
    }
}

/// Resolve a parsed font to concrete face data. Walks the family
/// fallback chain and returns both the raw face bytes and the face
/// index (for TTC collections).
fn resolve_face(font: &ParsedFont, os_name: &str) -> Option<(&'static [u8], u32)> {
    let db = FontDatabase::get();
    let id = db.query_chain(&font.families, font.weight, font.italic, os_name)?;
    db.face_data(id)
}

/// True when the face maps every scalar in `text` to a real glyph.
/// rustybuzz maps misses to glyph 0 (.notdef) — that is the tell.
fn face_covers(face_data: &[u8], face_index: u32, text: &str) -> bool {
    use ttf_parser::Face;
    let Ok(face) = Face::parse(face_data, face_index) else {
        return true;
    };
    let tables = face.tables();
    let Some(cmap) = tables.cmap else {
        return true;
    };
    text.chars().all(|c| cmap_glyph(c as u32, &cmap))
}

/// True when any unicode cmap subtable maps `code` to a glyph.
fn cmap_glyph(code: u32, cmap: &ttf_parser::cmap::Table<'_>) -> bool {
    for st in cmap.subtables {
        if st.is_unicode() && st.glyph_index(code).is_some() {
            return true;
        }
    }
    false
}

/// The bundled emoji face for codepoint fallback.
fn resolve_emoji_face() -> Option<(&'static [u8], u32)> {
    let db = FontDatabase::get();
    let id = db.query_strict_public("Noto Emoji")?;
    db.face_data(id)
}

/// Measure text using the parsed font. Returns zero metrics for empty
/// text or unresolvable fonts — matching Canvas 2D's tolerant behaviour.
pub fn measure_text_metrics(text: &str, font: &ParsedFont, os_name: &str) -> TextMetrics {
    if text.is_empty() {
        return TextMetrics::zero();
    }
    let Some((data, idx)) = resolve_face(font, os_name) else {
        return TextMetrics::zero();
    };
    // Codepoint fallback: when the resolved face misses a codepoint,
    // only the uncovered segments are reshaped with a fallback face.
    // Handing the WHOLE string to the emoji face made every family
    // measure alike whenever one CJK/emoji codepoint was present.
    let mut run = if !face_covers(data, idx, text) {
        shape_with_fallback(text, data, idx, font, os_name)
    } else {
        shaper::shape(text, data, idx, font.size_px)
    };
    // Legacy Apple faces: Chrome (following the system text stack) reports
    // QuickDraw-era vertical metrics for these, NOT their hhea values —
    // constant fractions of the em, linear in size (verified on a real
    // Chrome at 20/40/80px: Courier 0.9/0.25, Times 0.9/0.25,
    // Helvetica 0.925/0.225 — while hhea says 0.753/0.246 etc.).
    if os_name == "macOS" {
        // The override keys on the RESOLVED face (a `serif` request that
        // lands on Times still gets Times's legacy values), never on the
        // requested name — "Times New Roman" is a modern MS face and must
        // keep its hhea metrics.
        let resolved_family = resolve_face(font, os_name)
            .and_then(|(_, _)| {
                let db = FontDatabase::get();
                db.query_chain(&font.families, font.weight, font.italic, os_name)
                    .and_then(|id| db.family_of(id))
            })
            .map(|f| f.to_lowercase());
        if let Some(fam) = resolved_family {
            let (asc, desc): (f32, f32) = match fam.as_str() {
                "courier" | "times" => (0.9, 0.25),
                "helvetica" => (0.925, 0.225),
                _ => (run.ascent / font.size_px, run.descent / font.size_px),
            };
            if asc != run.ascent / font.size_px || desc != run.descent / font.size_px {
                run.ascent = asc * font.size_px;
                run.descent = desc * font.size_px;
            }
        }
    }

    // em-height approximations: CSS spec says em_height_ascent ≈ 0.8 *
    // size and em_height_descent ≈ 0.2 * size for most Latin fonts.
    // Chrome's actual values come from the OS/2 table's sTypoAscender/
    // sTypoDescender, so the ratios aren't exactly 0.8/0.2 — but we're
    // well within the tolerance fingerprint probes allow.
    let em_ascent = font.size_px * 0.8;
    let em_descent = font.size_px * 0.2;

    TextMetrics {
        width: run.width,
        actual_bounding_box_left: -run.bbox_left.min(0.0),
        actual_bounding_box_right: run.bbox_right.max(run.width),
        actual_bounding_box_ascent: run.bbox_ascent,
        actual_bounding_box_descent: run.bbox_descent,
        // Chrome rounds the font-level box to whole pixels (real capture:
        // 40px sans-serif reports ascent 37, descent 9 — integers even
        // though hhea math gives 37.1/8.9). actualBoundingBox* keeps the
        // rasterized sub-pixel values.
        font_bounding_box_ascent: run.ascent.round(),
        font_bounding_box_descent: run.descent.round(),
        em_height_ascent: em_ascent,
        em_height_descent: em_descent,
        // Canvas 2D default textBaseline is "alphabetic" = 0. The other
        // baselines are offsets relative to the alphabetic baseline.
        // Hanging tracks the ROUNDED ascent on a real Chrome (29.6 = 37*0.8).
        hanging_baseline: run.ascent.round() * 0.8,
        alphabetic_baseline: 0.0,
        // Real capture: ideographic = -descent exactly (monospace -9, serif -10).
        ideographic_baseline: -(run.descent.round()),
    }
}

/// Convenience: measure width only (for the simple `measureText(...).width`
/// fingerprint probes).
pub fn measure_text_width(text: &str, font: &ParsedFont, os_name: &str) -> f64 {
    measure_text_metrics(text, font, os_name).width as f64
}

/// One blitted glyph ready to composite onto the canvas pixel buffer.
///
/// Each entry is in "canvas pixel space" — integer x/y top-left
/// coordinates and an alpha-only coverage buffer. The canvas performs
/// the actual premultiplied-alpha blend in
/// `Canvas2D::composite_alpha_mask`.
pub struct PlacedGlyph {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub alpha: f32,
}

/// Resolve the font face + shape `text` via rustybuzz, returning the
/// face bytes, TTC index, and the shaped run. Lets the canvas draw
/// glyphs through Skia's own rasterizer (Chrome-parity by
/// construction — Chrome's 2D-canvas text IS Skia) while keeping our
/// rustybuzz shaping so `measureText` stays consistent.
pub fn shape_run(
    text: &str,
    font: &ParsedFont,
    os_name: &str,
) -> Option<(&'static [u8], u32, shaper::ShapedRun)> {
    if text.is_empty() {
        return None;
    }
    let (data, idx) = resolve_face(font, os_name)?;
    if face_covers(data, idx, text) {
        return Some((data, idx, shaper::shape(text, data, idx, font.size_px)));
    }
    // Per-segment fallback: only the characters the primary face misses
    // (CJK, emoji, symbols) are reshaped with a fallback face. The old
    // behaviour handed the WHOLE string to the emoji face whenever one
    // codepoint was uncovered, which made every family measure alike.
    Some((data, idx, shape_with_fallback(text, data, idx, font, os_name)))
}

/// Shape `text` with the primary face, reshaping uncovered segments with
/// the first fallback face that covers them. Glyph x positions in the
/// merged run stay monotonic — later segments are offset by the width
/// already consumed.
fn shape_with_fallback(
    text: &str,
    data: &'static [u8],
    idx: u32,
    font: &ParsedFont,
    os_name: &str,
) -> shaper::ShapedRun {
    let mut merged = shaper::ShapedRun {
        glyphs: Vec::new(),
        width: 0.0,
        ascent: 0.0,
        descent: 0.0,
        line_gap: 0.0,
        bbox_left: 0.0,
        bbox_right: 0.0,
        bbox_ascent: 0.0,
        bbox_descent: 0.0,
    };
    let mut consumed = 0.0f32;
    // Line box metrics come from the TALLEST face in the run (asc +
    // desc + gap) — a CJK fallback sets a span's offsetHeight even when
    // the primary face is Latin-only.
    let mut tallest = (0.0f32, 0.0f32, 0.0f32);
    let mut bbox_left = f32::INFINITY;
    let mut bbox_ascent = f32::NEG_INFINITY;
    let mut bbox_descent = f32::NEG_INFINITY;
    let mut any_bbox = false;
    for segment in uncovered_segments(text, data, idx) {
        let covered_by_primary = segment.1;
        let run = if covered_by_primary {
            shaper::shape(&segment.0, data, idx, font.size_px)
        } else if let Some((fdata, fidx)) = resolve_fallback_face(&segment.0, font, os_name) {
            shaper::shape(&segment.0, fdata, fidx, font.size_px)
        } else {
            shaper::shape(&segment.0, data, idx, font.size_px)
        };
        if run.ascent + run.descent + run.line_gap > tallest.0 + tallest.1 + tallest.2 {
            tallest = (run.ascent, run.descent, run.line_gap);
        }
        for mut g in run.glyphs {
            g.x_offset += consumed;
            merged.glyphs.push(g);
        }
        if run.bbox_left != f32::INFINITY {
            let seg_left = consumed + run.bbox_left;
            if seg_left < bbox_left {
                bbox_left = seg_left;
            }
            any_bbox = true;
        }
        if run.bbox_ascent > bbox_ascent {
            bbox_ascent = run.bbox_ascent;
        }
        if run.bbox_descent > bbox_descent {
            bbox_descent = run.bbox_descent;
        }
        consumed += run.width;
        merged.width += run.width;
        merged.bbox_right = merged.bbox_right.max(consumed);
    }
    merged.ascent = tallest.0;
    merged.descent = tallest.1;
    merged.line_gap = tallest.2;
    if any_bbox {
        merged.bbox_left = bbox_left;
        merged.bbox_ascent = bbox_ascent;
        merged.bbox_descent = bbox_descent;
    }
    merged
}

/// Split `text` into maximal runs covered (true) and not covered (false)
/// by the given face.
fn uncovered_segments(
    text: &str,
    data: &[u8],
    idx: u32,
) -> Vec<(String, bool)> {
    let mut segments: Vec<(String, bool)> = Vec::new();
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        let s: &'static str;
        // A one-char string; leak is bounded by the text length and these
        // live only for the duration of the call.
        let leaked: &'static str = Box::leak(ch.encode_utf8(&mut buf).to_string().into_boxed_str());
        s = leaked;
        let covered = {
            let one = [ch];
            face_covers(data, idx, &one.iter().collect::<String>())
        };
        match segments.last_mut() {
            Some((acc, acc_covered)) if *acc_covered == covered => acc.push_str(s),
            _ => segments.push((s.to_string(), covered)),
        }
    }
    segments
}

/// A fallback face for a segment the primary face cannot cover: CJK falls
/// to the platform's CJK face, symbol/emoji codepoints to the emoji face,
/// then anything else the database holds under the usual fallback names.
fn resolve_fallback_face(
    segment: &str,
    font: &ParsedFont,
    os_name: &str,
) -> Option<(&'static [u8], u32)> {
    let db = FontDatabase::get();
    let is_symbol = segment
        .chars()
        .any(|c| {
            let u = c as u32;
            (0x1F000..=0x1FAFF).contains(&u)
                || (0x2600..=0x27BF).contains(&u)
                || u == 0xFE0F
        });
    // CSS per-glyph fallback walks the AUTHOR's family list first — a face
    // named in the request that misses the glyph hands off to the next
    // requested family, not to a system default. Measured tell: probing
    // "Apple Color Emoji, monospace" with latin text must measure like
    // monospace (Chrome), not like whatever system face covers latin.
    // Only the primary face (already tried) is skipped; the rest of the
    // author's list comes before the platform defaults.
    let mut chain: Vec<&str> = Vec::new();
    if is_symbol {
        chain.push("Noto Emoji");
    }
    for fam in font.families.iter().skip(1) {
        chain.push(fam.as_str());
    }
    if os_name == "macOS" {
        chain.extend(["PingFang SC", "Hiragino Sans", "Arial Unicode MS", "Apple Symbols", "STHeiti", "Heiti SC"]);
    } else if os_name == "Windows" {
        chain.extend(["Microsoft YaHei", "SimSun", "Segoe UI Symbol"]);
    } else {
        chain.extend(["Noto Sans CJK SC", "Noto Sans", "DejaVu Sans"]);
    }
    // The primary family last — better to reuse it than to fail.
    if let Some(primary) = font.families.first() {
        chain.push(primary.as_str());
    }
    for name in chain {
        if let Some(id) = db.query_strict_public(name) {
            if let Some((fdata, fidx)) = db.face_data(id) {
                if face_covers(fdata, fidx, segment) {
                    return Some((fdata, fidx));
                }
            }
        }
    }
    None
}

/// Shape + rasterize `text` at the given origin, producing a list of
/// placed alpha masks ready for canvas compositing. The `(x, y)`
/// origin is the start of the text run, with the alphabetic baseline
/// at `y` (matching Canvas 2D's default `textBaseline`).
pub fn rasterize_text(
    text: &str,
    x: f32,
    y: f32,
    font: &ParsedFont,
    r: u8,
    g: u8,
    b: u8,
    alpha: f32,
    os_name: &str,
) -> Vec<PlacedGlyph> {
    if text.is_empty() {
        return Vec::new();
    }
    let Some((data, idx)) = resolve_face(font, os_name) else {
        return Vec::new();
    };
    let run = shaper::shape(text, data, idx, font.size_px);

    let mut out = Vec::with_capacity(run.glyphs.len());
    let mut cursor_x = x;
    for glyph in &run.glyphs {
        if let Some(bitmap) = raster::rasterize_glyph(data, idx, glyph.glyph_id, font.size_px) {
            // `left` is the horizontal offset from the pen position to
            // the glyph bitmap's left edge. `top` is the vertical
            // offset from the baseline to the glyph bitmap's top edge
            // (positive means above the baseline).
            let draw_x = cursor_x + glyph.x_offset + bitmap.left as f32;
            let draw_y = y - glyph.y_offset - bitmap.top as f32;
            out.push(PlacedGlyph {
                x: draw_x.round() as i32,
                y: draw_y.round() as i32,
                width: bitmap.width,
                height: bitmap.height,
                coverage: bitmap.pixels,
                r,
                g,
                b,
                alpha,
            });
        }
        cursor_x += glyph.x_advance;
    }
    out
}

/// Adapter that turns `ttf_parser::OutlineBuilder` callbacks into
/// `Path2D` commands, applying the EM→pixel scale and the Y-axis
/// flip (font space is Y-up, canvas is Y-down).
struct Path2DOutlineBuilder<'a> {
    path: &'a mut crate::canvas::path::Path2D,
    /// Origin x in canvas space (where x=0 in font-space lands).
    origin_x: f32,
    /// Origin y in canvas space (the alphabetic baseline of the run).
    origin_y: f32,
    /// EM-units-to-pixels factor = size_px / units_per_em.
    scale: f32,
}

impl Path2DOutlineBuilder<'_> {
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        // Font space: y up, em units. Canvas space: y down, pixels.
        (
            self.origin_x + x * self.scale,
            self.origin_y - y * self.scale,
        )
    }
}

impl rustybuzz::ttf_parser::OutlineBuilder for Path2DOutlineBuilder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (mx, my) = self.map(x, y);
        self.path.move_to(mx, my);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (mx, my) = self.map(x, y);
        self.path.line_to(mx, my);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (cpx, cpy) = self.map(x1, y1);
        let (mx, my) = self.map(x, y);
        self.path.quadratic_curve_to(cpx, cpy, mx, my);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1x, c1y) = self.map(x1, y1);
        let (c2x, c2y) = self.map(x2, y2);
        let (mx, my) = self.map(x, y);
        self.path.bezier_curve_to(c1x, c1y, c2x, c2y, mx, my);
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

/// Shape `text` and append the outline contours of every glyph into
/// `path`, positioned at the given baseline origin. Returns true if any
/// glyph contributed contours (false for empty text or all-bitmap fonts
/// — color-emoji fonts have no `glyf` table and silently produce no
/// outline).
///
/// Used by `Canvas2D::stroke_text` to build a stroke-able path. The
/// shaping pipeline matches `rasterize_text` so stroked text aligns
/// pixel-for-pixel with what filled text would have rendered.
pub fn append_text_outline_to_path(
    path: &mut crate::canvas::path::Path2D,
    text: &str,
    x: f32,
    y: f32,
    font: &ParsedFont,
    os_name: &str,
) -> bool {
    if text.is_empty() {
        return false;
    }
    let Some((data, idx)) = resolve_face(font, os_name) else {
        return false;
    };
    // Parse a fresh ttf_parser::Face for outline access. rustybuzz's
    // Face wraps this internally but doesn't expose outline_glyph
    // directly — re-parsing is cheap (header-only, no glyph data
    // copied).
    let Ok(ttf_face) = rustybuzz::ttf_parser::Face::parse(data, idx) else {
        return false;
    };
    let upem = ttf_face.units_per_em() as f32;
    if upem <= 0.0 {
        return false;
    }
    let scale = font.size_px / upem;
    let run = shaper::shape(text, data, idx, font.size_px);

    let mut any_outline = false;
    let mut cursor_x = x;
    for glyph in &run.glyphs {
        // Per-glyph origin: the cursor + the shaper's per-glyph offset.
        // y_offset is in pixels (already-scaled), which matches our
        // origin-in-canvas-space convention; subtract because canvas
        // y grows downward.
        let glyph_origin_x = cursor_x + glyph.x_offset;
        let glyph_origin_y = y - glyph.y_offset;
        let mut builder = Path2DOutlineBuilder {
            path,
            origin_x: glyph_origin_x,
            origin_y: glyph_origin_y,
            scale,
        };
        if ttf_face
            .outline_glyph(
                rustybuzz::ttf_parser::GlyphId(glyph.glyph_id as u16),
                &mut builder,
            )
            .is_some()
        {
            any_outline = true;
        }
        cursor_x += glyph.x_advance;
    }
    any_outline
}

/// Composite a single PlacedGlyph onto a premultiplied-RGBA pixel
/// buffer. Uses SRC_OVER with the glyph colour premultiplied by the
/// per-pixel coverage × global alpha.
pub fn composite_glyph(
    glyph: &PlacedGlyph,
    pixels: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
) {
    for gy in 0..glyph.height {
        for gx in 0..glyph.width {
            let px = glyph.x + gx as i32;
            let py = glyph.y + gy as i32;
            if px < 0 || py < 0 || px >= canvas_width as i32 || py >= canvas_height as i32 {
                continue;
            }

            let cov_idx = (gy * glyph.width + gx) as usize;
            if cov_idx >= glyph.coverage.len() {
                continue;
            }
            let cov = glyph.coverage[cov_idx] as f32 / 255.0;
            if cov < 0.001 {
                continue;
            }

            let a = cov * glyph.alpha;
            if a <= 0.0 {
                continue;
            }

            let dst_idx = ((py as u32 * canvas_width + px as u32) * 4) as usize;
            if dst_idx + 3 >= pixels.len() {
                continue;
            }

            // Premultiplied source-over. `src` components are the
            // glyph colour scaled by coverage-adjusted alpha; `dst`
            // components are whatever is already in the buffer.
            let src_r = glyph.r as f32 * a;
            let src_g = glyph.g as f32 * a;
            let src_b = glyph.b as f32 * a;
            let src_a = 255.0 * a;

            let dst_r = pixels[dst_idx] as f32;
            let dst_g = pixels[dst_idx + 1] as f32;
            let dst_b = pixels[dst_idx + 2] as f32;
            let dst_a = pixels[dst_idx + 3] as f32;

            let inv_src_a = 1.0 - a;
            let out_r = (src_r + dst_r * inv_src_a).min(255.0);
            let out_g = (src_g + dst_g * inv_src_a).min(255.0);
            let out_b = (src_b + dst_b * inv_src_a).min(255.0);
            let out_a = (src_a + dst_a * inv_src_a).min(255.0);

            pixels[dst_idx] = out_r as u8;
            pixels[dst_idx + 1] = out_g as u8;
            pixels[dst_idx + 2] = out_b as u8;
            pixels[dst_idx + 3] = out_a as u8;
        }
    }
}

// ---------------------------------------------------------------------------
// Backwards-compatible size-only helpers.
//
// A few call sites currently use a `font_size: f32` field without a
// full parsed font. The thin wrappers below let them migrate at their
// own pace — internally they construct a default `ParsedFont` with the
// given size and hand off to the real pipeline.
// ---------------------------------------------------------------------------

/// Legacy convenience: measure width using a default sans-serif face
/// at the given size. New code should use `measure_text_width`.
pub fn measure_text_width_size_only(text: &str, font_size: f32, os_name: &str) -> f64 {
    let font = ParsedFont {
        size_px: font_size,
        ..ParsedFont::default_font()
    };
    measure_text_width(text, &font, os_name)
}

/// Legacy convenience for `fillText` paths that only know the font
/// size. Uses the default sans-serif chain (Liberation Sans via the
/// bundled font database).
#[allow(
    clippy::too_many_arguments,
    reason = "text-shaping fn takes many layout args; struct-wrapping adds churn without clarity"
)]
pub fn rasterize_text_size_only(
    text: &str,
    x: f32,
    y: f32,
    font_size: f32,
    r: u8,
    g: u8,
    b: u8,
    alpha: f32,
    os_name: &str,
) -> Vec<PlacedGlyph> {
    let font = ParsedFont {
        size_px: font_size,
        ..ParsedFont::default_font()
    };
    rasterize_text(text, x, y, &font, r, g, b, alpha, os_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_hello_world_at_16px_arial() {
        let font = ParsedFont::parse("16px Arial").unwrap();
        let metrics = measure_text_metrics("Hello, World!", &font, "Linux");
        assert!(metrics.width > 30.0 && metrics.width < 200.0);
        assert!(metrics.font_bounding_box_ascent > 10.0);
    }

    #[test]
    fn different_sizes_different_widths() {
        let small = ParsedFont::parse("10px Arial").unwrap();
        let big = ParsedFont::parse("40px Arial").unwrap();
        let w1 = measure_text_width("Hello", &small, "Linux");
        let w2 = measure_text_width("Hello", &big, "Linux");
        assert!(w2 > w1 * 3.0);
    }

    #[test]
    fn mono_wider_than_sans_for_narrow_text() {
        // For the narrow character "i" repeated, a proportional sans
        // font measures much less than a fixed-width monospace font.
        let sans = ParsedFont::parse("14px Arial").unwrap();
        let mono = ParsedFont::parse("14px monospace").unwrap();
        let sans_w = measure_text_width("iiiiii", &sans, "Linux");
        let mono_w = measure_text_width("iiiiii", &mono, "Linux");
        assert!(
            mono_w > sans_w * 1.3,
            "mono should be much wider for narrow chars: sans={sans_w} mono={mono_w}"
        );
    }

    #[test]
    fn rasterize_produces_glyphs() {
        let font = ParsedFont::parse("24px Arial").unwrap();
        let glyphs = rasterize_text("A", 0.0, 20.0, &font, 0, 0, 0, 1.0, "Linux");
        assert!(!glyphs.is_empty());
        let g = &glyphs[0];
        assert!(g.width > 0 && g.height > 0);
        assert!(g.coverage.iter().any(|&c| c > 0));
    }

    #[test]
    fn rasterize_empty_returns_empty() {
        let font = ParsedFont::parse("16px Arial").unwrap();
        let glyphs = rasterize_text("", 0.0, 0.0, &font, 0, 0, 0, 1.0, "Linux");
        assert!(glyphs.is_empty());
    }
}

/// Measure a text run the way the layout engine needs it: width plus the
/// line box height of the face actually used (ascent + descent), because a
/// fallback glyph (CJK, emoji) rides a taller font than the primary face.
///
/// `families` are the element's computed `font-family` list; generic
/// families map to the same names the font database resolves. Returns
/// `None` when the caller should fall back to its own estimate (no face
/// resolved, or an empty family list).
pub fn measure_for_layout(
    text: &str,
    size_px: f32,
    families: &[crate::css_values::types::font::FontFamily],
) -> Option<(f64, f64)> {
    use crate::css_values::types::font::{FontFamily, GenericFamily};
    if text.is_empty() || families.is_empty() {
        return None;
    }
    let os_name = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        _ => "Linux",
    };
    let names: Vec<String> = families
        .iter()
        .map(|f| match f {
            FontFamily::Named(n) => n.clone(),
            FontFamily::Generic(g) => match g {
                GenericFamily::Serif | GenericFamily::UiSerif => "serif".to_string(),
                GenericFamily::SansSerif | GenericFamily::SystemUi | GenericFamily::UiSansSerif => {
                    "sans-serif".to_string()
                }
                GenericFamily::Monospace | GenericFamily::UiMonospace => "monospace".to_string(),
                GenericFamily::Cursive => "cursive".to_string(),
                GenericFamily::Fantasy | GenericFamily::UiRounded => "fantasy".to_string(),
                GenericFamily::Emoji => "emoji".to_string(),
                GenericFamily::Math | GenericFamily::Fangsong => "serif".to_string(),
            },
        })
        .collect();
    let font = ParsedFont {
        size_px,
        weight: 400,
        italic: false,
        families: names,
    };
    let m = measure_text_metrics(text, &font, os_name);
    let run = match shape_run(text, &font, os_name) {
        Some((_, _, run)) => run,
        None => return Some((m.width as f64, m.font_bounding_box_ascent as f64)),
    };
    Some((
        m.width as f64,
        (run.ascent + run.descent + run.line_gap) as f64,
    ))
}
