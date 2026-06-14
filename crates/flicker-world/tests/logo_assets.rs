#[test]
fn both_logos_decode() {
    for p in [
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/elideus_productions_yellow.png"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clay_engine_infinity_grey.png"),
    ] {
        match image::open(p) {
            Ok(img) => { let rgba = img.to_rgba8(); let (w,h)=rgba.dimensions(); eprintln!("OK {p}: {w}x{h}"); }
            Err(e) => eprintln!("FAIL {p}: {e}"),
        }
    }
}
