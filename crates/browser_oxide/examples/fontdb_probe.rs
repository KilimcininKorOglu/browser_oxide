use browser_oxide::canvas::text::{FontDatabase, ParsedFont};
fn main() {
    for fam in ["monospace", "serif", "Helvetica"] {
        let font = ParsedFont { size_px: 72.0, weight: 400, italic: false, families: vec![fam.to_string()] };
        let m = browser_oxide::canvas::text::measure_text_metrics("mmmmmmmmmmlli字", &font, "macOS");
        println!("{fam}: width={} asc={} desc={}", m.width, m.font_bounding_box_ascent, m.font_bounding_box_descent);
    }
    let db = FontDatabase::get();
    for fam in ["monospace", "serif", "Helvetica"] {
        if let Some(id) = db.query(fam, 400, false, "macOS") {
            let (d, i) = db.face_data(id).unwrap();
            println!("{fam} -> {} bytes idx {}", d.len(), i);
        }
    }
}
