fn main() {
    let report = artex_tools::coverage();
    let text = serde_json::to_string_pretty(&report).unwrap();
    if let Some(path) = std::env::args().nth(1) {
        std::fs::write(path, text).unwrap();
    } else {
        println!("{text}");
    }
}
