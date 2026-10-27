//! Write a synthetic colour-bar test movie (H.264 in MP4, 320x192, 24 fps).
//!
//! `cargo run -p transit-video --example test_movie -- out.mp4 [seconds]`

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "test_pattern.mp4".into());
    let secs = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);
    match transit_video::synth::test_pattern_mp4(secs).map(|b| std::fs::write(&out, b)) {
        Ok(Ok(())) => println!("wrote {out}"),
        Ok(Err(e)) => eprintln!("{out}: {e}"),
        Err(e) => eprintln!("{e}"),
    }
}
