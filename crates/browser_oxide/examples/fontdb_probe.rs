use browser_oxide::canvas::text::FontDatabase;
fn main() {
    let db = FontDatabase::get();
    for f in ["Courier", "Helvetica", "Menlo", "Verdana", "Arial"] {
        if let Some(id) = db.query(f, 400, false, "macOS") {
            let (d, i) = db.face_data(id).unwrap();
            if let Ok(face) = ttf_parser::Face::parse(d, i) {
                let os2 = face.tables().os2;
                println!(
                    "{f:<10} upem={} hhea=({},{}) os2win={:?} os2typo={:?} xheight={:?}",
                    face.units_per_em(),
                    face.ascender(),
                    face.descender(),
                    os2.map(|o| (o.windows_ascender(), o.windows_descender())),
                    os2.map(|o| (o.typographic_ascender(), o.typographic_descender())),
                    os2.map(|o| o.use_typographic_metrics()),
                );
            }
        }
    }
}
