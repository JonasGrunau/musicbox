//! Scores the phrase port against rekordbox's PSSI (.EXT). Usage:
//!   cargo run --release -p mb-analysis --example phrase_score -- [--dump] [filter]
//! (`MB_ORACLE=oracle-b` for the second fixture.)

#[path = "../tests/common/mod.rs"]
mod common;

use mb_anlz::Tag;
use mb_anlz::tags::SongStructure;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dump = args.iter().any(|a| a == "--dump");
    let filter = args.iter().find(|a| !a.starts_with("--")).cloned();
    let fixture = std::env::var("MB_ORACLE").unwrap_or_else(|_| "oracle-a".into());
    let Some(tracks) = common::oracle_tracks(&fixture) else {
        eprintln!("fixtures/{fixture} missing");
        return;
    };
    for t in tracks.iter().filter(|t| filter.as_ref().is_none_or(|f| t.path.to_string_lossy().contains(f.as_str()))) {
        let pssi = t
            .ext
            .as_ref()
            .and_then(|e| e.sections_with(Tag::PSSI).next())
            .and_then(|s| SongStructure::decode(s).ok());
        let name = t.path.display().to_string();
        match &pssi {
            None => println!("{name}: no PSSI"),
            Some(p) => {
                println!(
                    "{name}: mood {} end {} bank {} unk {:?} phrases {}",
                    p.mood,
                    p.end_beat,
                    p.bank,
                    (p.unk_0c, p.unk_14, p.unk_18, p.unk_1d, p.unk_1e, p.unk_1f),
                    p.phrases.len()
                );
                if dump {
                    for ph in &p.phrases {
                        println!("  {:?}", ph.words);
                    }
                }
            }
        }
    }
}
