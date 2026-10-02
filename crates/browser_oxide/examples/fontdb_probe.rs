use browser_oxide::canvas::text::{FontDatabase, ParsedFont};
fn main() {
    let font = ParsedFont { size_px: 40.0, weight: 400, italic: false, families: vec!["Arial".into()] };
    let db = FontDatabase::get();
    let id = db.query("Arial", 400, false, "macOS").unwrap();
    let (data, idx) = db.face_data(id).unwrap();
    let face = ttf_parser::Face::parse(data, idx).unwrap();
    println!("upem={} hmtx H={:?}", face.units_per_em(), {
        let gid = face.glyph_index('H').unwrap();
        face.glyph_hor_advance(gid).map(|a| (a, a as f32 * 40.0 / face.units_per_em() as f32))
    });
    let m = browser_oxide::canvas::text::measure_text_metrics("H", &font, "macOS");
    println!("our H width = {}", m.width);
    if let Some((d, i, run)) = browser_oxide::canvas::text::shape_run("H", &font, "macOS") {
        println!("shaped: bytes={} idx={} glyphs={} width={}", d.len(), i, run.glyphs.len(), run.width);
        for g in &run.glyphs {
            println!("  gid={} x_adv={} x_off={}", g.glyph_id, g.x_advance, g.x_offset);
        }
    }
}
