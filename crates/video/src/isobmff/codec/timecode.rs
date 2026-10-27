/// QuickTime timecode sample description (`tmcd`) plus the start frame read from its first sample.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TimecodeConfig {
    /// Bit 0 drop frame, bit 1 24-hour max, bit 2 negative times OK, bit 3 counter.
    pub flags: u32,
    pub timescale: u32,
    pub frame_duration: u32,
    /// Nominal frames per second (`numberOfFrames`), e.g. 30 for 29.97.
    pub frames_per_second: u8,
    /// Frame number of the first timecode sample (filled in by the demuxer).
    pub start_frame: Option<u32>,
    /// Source reel name (`name` box) if present.
    pub name: Option<String>,
}

impl TimecodeConfig {
    pub fn drop_frame(&self) -> bool {
        self.flags & 1 != 0
    }
    /// Format a frame number as `HH:MM:SS:FF` (`;` before the frames when drop-frame).
    pub fn format_frame(&self, frame: u32) -> String {
        let fps = self.frames_per_second.max(1) as u64;
        let mut f = frame as u64;
        if self.drop_frame() && fps >= 30 {
            let drop = (fps / 30) * 2; // 2 frames per minute at 30, 4 at 60
            let per_min = fps * 60 - drop;
            let per_10 = per_min * 10 + drop;
            let d = f / per_10;
            let m = f % per_10;
            f += drop * 9 * d + if m > drop { drop * ((m - drop) / per_min) } else { 0 };
        }
        let ff = f % fps;
        let s = (f / fps) % 60;
        let mi = (f / fps / 60) % 60;
        let h = (f / fps / 3600) % 24;
        let sep = if self.drop_frame() { ';' } else { ':' };
        format!("{h:02}:{mi:02}:{s:02}{sep}{ff:02}")
    }
}
