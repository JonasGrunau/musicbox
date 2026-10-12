//! For each `click_128_offset1234` probe in oracle-a: the first PWV3 column rekordbox drew with a
//! visible height, next to the column of the first loud sample in our decode.

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let tracks = common::oracle_tracks("oracle-a").expect("fixture");
    for t in tracks.iter().filter(|t| t.path.to_string_lossy().contains("probes/") && t.path.file_stem().unwrap() == "click_128_offset1234") {
        let w = t.wave_scroll().unwrap();
        let first_rb = w.data.iter().position(|&b| (b & 0x1f) > 2).unwrap();
        let mut d = mb_audio::open(&t.path).unwrap();
        let pcm = d.decode_all();
        let rate = d.info().sample_rate;
        let spc = (rate / 150) as usize;
        let first = pcm.iter().position(|s| s[0].abs().max(s[1].abs()) > 0.05).unwrap();
        let heights: Vec<u8> = w.data[first_rb.saturating_sub(2)..first_rb + 3].iter().map(|b| b & 0x1f).collect();
        println!("{:40} rb_col {first_rb:5} ours_col {:5} (sample {first}) heights {heights:?}", t.corpus_rel().unwrap(), first / spc);
    }
}
