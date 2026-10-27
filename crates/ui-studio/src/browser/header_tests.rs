use super::*;

#[test]
fn audio_rows_show_length_and_channels_read_from_the_header() {
    let mut t = T::new();
    t.add_and_expand();
    let r = t.row_named("loop 90.wav");
    assert_eq!(r.info.as_deref(), Some("2.0 s · stereo"), "{r:?}");
    t.click(mid(t.row(&t.path("Kicks")).rect.unwrap()));
    t.settle();
    assert_eq!(t.row_named("Kick 01.wav").info.as_deref(), Some("0.5 s · mono"));
    assert!(t.app.browser.layout.headers_read >= 3);
}

#[test]
fn parse_header_reads_wav_aiff_and_flac_lengths() {
    let w = parse_header(&wav(2, 44_100, 22_050)).unwrap();
    assert_eq!((w.channels, w.rate, w.secs), (2, 44_100, 0.5));
    // Only the header: the data chunk is cut off.
    let cut = wav(1, 48_000, 96_000);
    assert_eq!(parse_header(&cut[..200]).unwrap().secs, 2.0);
    // AIFF: COMM with 48 kHz as an 80-bit float.
    let mut a = b"FORM\0\0\0\x2eAIFFCOMM\0\0\0\x12".to_vec();
    a.extend_from_slice(&1u16.to_be_bytes());
    a.extend_from_slice(&24_000u32.to_be_bytes());
    a.extend_from_slice(&16u16.to_be_bytes());
    a.extend_from_slice(&[0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]);
    let i = parse_header(&a).unwrap();
    assert_eq!((i.channels, i.rate, i.secs), (1, 48_000, 0.5));
    // FLAC STREAMINFO: 44.1 kHz, 2 channels, 88 200 samples.
    let mut f = b"fLaC\0\0\0\x22".to_vec();
    let mut si = [0u8; 34];
    let rate = 44_100u32;
    si[10] = (rate >> 12) as u8;
    si[11] = (rate >> 4) as u8;
    si[12] = ((rate & 0xF) << 4) as u8 | (1 << 1);
    si[14..18].copy_from_slice(&88_200u32.to_be_bytes());
    f.extend_from_slice(&si);
    let i = parse_header(&f).unwrap();
    assert_eq!((i.channels, i.rate, i.secs), (2, 44_100, 2.0));
    assert!(parse_header(b"ID3 not a header").is_none());
}

/// A WAV with `pad` bytes of LIST chunk before `fmt `.
fn wav_with_list(pad: usize) -> Vec<u8> {
    let plain = wav(2, 48_000, 24_000);
    let mut b = plain[..12].to_vec();
    b.extend_from_slice(b"LIST");
    b.extend_from_slice(&(pad as u32).to_le_bytes());
    b.extend(std::iter::repeat_n(7u8, pad));
    b.extend_from_slice(&plain[12..]);
    b
}

#[test]
fn a_wav_with_a_200_kb_chunk_before_fmt_still_shows_its_length() {
    let mut t = T::new();
    t.dir.file("Big Header.wav", &wav_with_list(200 * 1024));
    t.dir.file("Huge Header.wav", &wav_with_list(5 * 1024 * 1024));
    t.dir.file("Cut.wav", &wav_with_list(200 * 1024)[..1000]);
    t.add_and_expand();
    assert_eq!(t.row_named("Big Header.wav").info.as_deref(), Some("0.5 s · stereo"));
    assert_eq!(t.row_named("Huge Header.wav").info.as_deref(), Some("–"), "beyond the 4 MB walk bound");
    assert_eq!(t.row_named("Cut.wav").info.as_deref(), Some("–"), "truncated files are safe");
    assert_eq!(t.row_named("loop 90.wav").info.as_deref(), Some("2.0 s · stereo"));
}
