//! `cargo run -p mb-player --example play -- <file> [start_ms]`
//! Plays 5 seconds of `<file>` at 0.3 volume on the default output device, printing the clock.

use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: play <file> [start_ms]");
        std::process::exit(2);
    };
    let start_ms: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let player = match mb_player::Player::open(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let info = player.info();
    println!(
        "{:?} via {:?}: {} Hz (file {} Hz), {:.1} s; device {} Hz",
        info.format,
        info.backend,
        info.sample_rate,
        info.file_rate,
        info.duration_secs(),
        player.device_rate()
    );
    player.set_volume(0.3);
    player.play_from_ms(start_ms);
    let clock = player.clock();
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(500));
        println!("position {:9.1} ms (latency {:.1} ms)", clock.position_ms(), clock.latency_ms());
        if clock.finished() {
            break;
        }
    }
    player.pause();
}
