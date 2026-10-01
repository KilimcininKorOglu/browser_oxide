use browser_oxide::css_values::parse_property;
fn main() {
    let (decls, errs) = browser_oxide::css_parser::parse_declaration_list("font: 24px monospace");
    println!("decls: {:?}", decls.iter().map(|d| d.name.clone()).collect::<Vec<_>>());
    println!("errs: {:?}", errs.len());
    for d in &decls {
        match parse_property(&d.name, &d.value, d.important) {
            Ok(props) => for p in props { println!("  {:?} = {:?}", p.property, p.value); },
            Err(e) => println!("  ERR {e}"),
        }
    }
}
