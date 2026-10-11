//! Prints what import reads from audio files (tag items, duration, container).
//! `cargo run -p mb-library --example tags_dump -- <file>...`
fn main() {
    for arg in std::env::args_os().skip(1) {
        let path = std::path::PathBuf::from(arg);
        println!("### {}", path.display());
        match mb_library::tags::dump(&path) {
            Ok(lines) => lines.iter().for_each(|l| println!("  {l}")),
            Err(e) => println!("  error: {e}"),
        }
        match mb_library::tags::read_file(&path) {
            Ok(t) => println!("  => {t:?}"),
            Err(e) => println!("  => error: {e}"),
        }
    }
}
