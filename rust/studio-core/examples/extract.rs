//! `cargo run -p studio-core --example extract -- <file>`: prints the parts studio-core's `extract` reads from
//! the file (the same JSON as `/api/extract`'s "parts"), or `{"error": "..."}`. Used by testdata/extract_parity.py.

fn main() {
    let path = std::env::args().nth(1).expect("usage: extract <file>");
    let data = std::fs::read(&path).expect("read file");
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match studio_core::extract::extract(&name, &data) {
        Ok(parts) => println!("{parts}"),
        Err(e) => println!("{}", serde_json::json!({ "error": e.0 })),
    }
}
