use browser_oxide::canvas::text::FontDatabase;
fn main() {
    let db = FontDatabase::get();
    for f in ["LUCIDA GRANDE", "Lucida Grande", "PingFang SC", "Arial Hebrew", "Apple Color Emoji", "Liberation Sans"] {
        println!("{f} -> {:?}", db.query(f, 400, false, "macOS").map(|id| db.face_data(id).map(|(d,i)|(d.len(),i)).unwrap()));
    }
}
