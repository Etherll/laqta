//! The parts of the native recorder that need no OS: frame pacing, pause-aware timestamps, the bitrate
//! for a quality setting, the part of an area that a monitor shows, the copy of that part into a frame,
//! and the even sizes H.264 needs. Times are in 100 ns units, Media Foundation's unit.

use laqta_domain::record::{Quality, VideoCodec};

/// Media Foundation's time unit: 100 ns.
pub(crate) const TICKS_PER_SECOND: u64 = 10_000_000;

/// The time of sample `index` in a stream of `rate` samples per second, rounded down. It is computed
/// from the index each time, so the rounding never accumulates.
pub(crate) fn time_of(index: u64, rate: u32) -> u64 {
    let ticks = u128::from(index) * u128::from(TICKS_PER_SECOND) / u128::from(rate.max(1));
    u64::try_from(ticks).unwrap_or(u64::MAX)
}

/// One output frame: its time and duration, and whether it repeats the previous frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tick {
    pub(crate) time: u64,
    pub(crate) duration: u64,
    pub(crate) repeat: bool,
}

/// Paces output frames at `fps`. Frame `n` is due once the recording time reaches `time_of(n, fps)`.
/// A due frame takes the newest captured frame. When nothing was captured since the previous frame, it
/// repeats that one. Captures that arrive between two due frames replace each other, so only the newest
/// is written.
#[derive(Debug)]
pub(crate) struct Pacer {
    fps: u32,
    next: u64,
    fresh: bool,
}

impl Pacer {
    pub(crate) fn new(fps: u32) -> Self {
        Pacer { fps: fps.max(1), next: 0, fresh: false }
    }

    /// A frame was captured. The next due frame writes it.
    pub(crate) fn captured(&mut self) {
        self.fresh = true;
    }

    /// The next frame that is due at recording time `now`, or `None` when none is due yet.
    pub(crate) fn next_tick(&mut self, now: u64) -> Option<Tick> {
        let time = time_of(self.next, self.fps);
        if time > now {
            return None;
        }
        let tick = Tick { time, duration: time_of(self.next + 1, self.fps) - time, repeat: !self.fresh };
        self.fresh = false;
        self.next += 1;
        Some(tick)
    }

    /// Recording time left until the next frame is due: 0 once it is due.
    pub(crate) fn until_next(&self, now: u64) -> u64 {
        time_of(self.next, self.fps).saturating_sub(now)
    }
}

/// Recording time: the wall time since the recording started, less the time it spent paused. It stops
/// while paused, so frames written after a resume carry on from where the pause began.
#[derive(Debug)]
pub(crate) struct Timeline {
    start: u64,
    paused_at: Option<u64>,
    paused_total: u64,
}

impl Timeline {
    pub(crate) fn new(start: u64) -> Self {
        Timeline { start, paused_at: None, paused_total: 0 }
    }

    /// Starts a pause. A second pause while paused changes nothing.
    pub(crate) fn pause(&mut self, now: u64) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    pub(crate) fn resume(&mut self, now: u64) {
        if let Some(at) = self.paused_at.take() {
            self.paused_total = self.paused_total.saturating_add(now.saturating_sub(at));
        }
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// The recording time at wall time `now`.
    pub(crate) fn recording_time(&self, now: u64) -> u64 {
        let end = self.paused_at.unwrap_or(now);
        end.saturating_sub(self.start).saturating_sub(self.paused_total)
    }
}

/// The CRF that `Quality::Default` means: the x264 default.
const DEFAULT_CRF: u8 = 23;

/// Bits per pixel per frame, in thousandths, for each CRF bound. A CRF takes the first row whose bound is
/// at least that CRF, so the bitrate steps down as the CRF rises, about halving every 6 steps.
const CRF_BITS: [(u8, u64); 7] = [(12, 320), (18, 160), (23, 80), (28, 40), (34, 20), (40, 10), (51, 4)];

const MIN_CRF_BITRATE: u64 = 250_000;
const MAX_CRF_BITRATE: u64 = 100_000_000;

/// The target bitrate in bits per second for `quality` at this size and rate. A CRF maps through
/// `CRF_BITS`. `Bitrate(kbps)` is used as given. `Default` is CRF 23.
pub(crate) fn bitrate_bps(quality: Quality, width: u32, height: u32, fps: u32) -> u32 {
    let bps = match quality {
        Quality::Bitrate(kbps) if kbps > 0 => u64::from(kbps) * 1000,
        Quality::Crf(crf) => crf_bitrate(crf, width, height, fps),
        Quality::Bitrate(_) | Quality::Default => crf_bitrate(DEFAULT_CRF, width, height, fps),
    };
    u32::try_from(bps).unwrap_or(u32::MAX)
}

fn crf_bitrate(crf: u8, width: u32, height: u32, fps: u32) -> u64 {
    let milli_bits = CRF_BITS.iter().find(|(bound, _)| crf <= *bound).or(CRF_BITS.last()).map_or(80, |&(_, bits)| bits);
    let pixels_per_second = u64::from(width) * u64::from(height) * u64::from(fps.max(1));
    (pixels_per_second * milli_bits / 1000).clamp(MIN_CRF_BITRATE, MAX_CRF_BITRATE)
}

/// `n` rounded down to an even number. H.264 with 4:2:0 chroma needs even sides.
pub(crate) fn even_floor(n: u32) -> u32 {
    n & !1
}

/// The codecs the native recorder writes: H.264 from the software encoder, or from the GPU encoders that
/// the sink writer reaches through its hardware transforms.
pub(crate) fn is_h264(codec: VideoCodec) -> bool {
    matches!(codec, VideoCodec::X264 | VideoCodec::Nvenc | VideoCodec::Amf | VideoCodec::Qsv)
}

/// A rectangle of pixels. `x` and `y` are signed, because an area may start left of or above its monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// The part of an area that a monitor shows: the rectangle to read from the monitor, and where it goes
/// in the recording frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Visible {
    pub(crate) src_x: u32,
    pub(crate) src_y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) dst_x: u32,
    pub(crate) dst_y: u32,
}

/// The part of `area` that lies on a monitor of `target` pixels. `None` when none of it does.
pub(crate) fn visible_part(target: (u32, u32), area: Rect) -> Option<Visible> {
    let left = i64::from(area.x).max(0);
    let top = i64::from(area.y).max(0);
    let right = (i64::from(area.x) + i64::from(area.width)).min(i64::from(target.0));
    let bottom = (i64::from(area.y) + i64::from(area.height)).min(i64::from(target.1));
    if right <= left || bottom <= top {
        return None;
    }
    Some(Visible {
        src_x: u32::try_from(left).ok()?,
        src_y: u32::try_from(top).ok()?,
        width: u32::try_from(right - left).ok()?,
        height: u32::try_from(bottom - top).ok()?,
        dst_x: u32::try_from(left - i64::from(area.x)).ok()?,
        dst_y: u32::try_from(top - i64::from(area.y)).ok()?,
    })
}

/// Copies the visible part of a mapped staging texture into `frame`, a `width` x `height` BGRA image.
/// The staging rows are `pitch` bytes apart, and the visible part sits at its destination position in
/// them. Pixels outside the visible part are black.
pub(crate) fn copy_visible(frame: &mut [u8], staging: &[u8], pitch: usize, width: u32, height: u32, visible: Visible) {
    let row_bytes = width as usize * 4;
    let covers_all = visible.dst_x == 0 && visible.dst_y == 0 && visible.width == width && visible.height == height;
    if !covers_all {
        frame.fill(0);
    }
    let x = visible.dst_x as usize * 4;
    let run = visible.width as usize * 4;
    for row in 0..visible.height as usize {
        let y = visible.dst_y as usize + row;
        let from = y * pitch + x;
        let to = y * row_bytes + x;
        frame[to..to + run].copy_from_slice(&staging[from..from + run]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_of_is_exact_at_each_rate() {
        assert_eq!(time_of(0, 30), 0);
        assert_eq!(time_of(30, 30), TICKS_PER_SECOND);
        assert_eq!(time_of(1, 48_000), 208);
        assert_eq!(time_of(48_000, 48_000), TICKS_PER_SECOND);
        assert_eq!(time_of(3, 30), 1_000_000);
    }

    #[test]
    fn pacer_writes_the_first_capture_then_repeats() {
        let mut pacer = Pacer::new(30);
        pacer.captured();
        assert_eq!(pacer.next_tick(0), Some(Tick { time: 0, duration: 333_333, repeat: false }));
        assert_eq!(pacer.next_tick(0), None, "the next frame is due at 333 333");
        // 0.1 s of recording time makes frames 1, 2 and 3 due; nothing new was captured, so they repeat.
        let due: Vec<Tick> = std::iter::from_fn(|| pacer.next_tick(1_000_000)).collect();
        assert_eq!(due.len(), 3);
        assert!(due.iter().all(|tick| tick.repeat));
        assert_eq!(due[2].time, 1_000_000);
    }

    #[test]
    fn captures_between_ticks_drop_all_but_the_last_use() {
        let mut pacer = Pacer::new(30);
        pacer.captured();
        pacer.captured();
        pacer.captured();
        assert_eq!(pacer.next_tick(0).map(|tick| tick.repeat), Some(false));
        assert_eq!(pacer.next_tick(0), None);
        assert_eq!(pacer.next_tick(333_333).map(|tick| tick.repeat), Some(true));
    }

    #[test]
    fn until_next_counts_down_to_zero() {
        let mut pacer = Pacer::new(10);
        assert_eq!(pacer.until_next(0), 0);
        assert!(pacer.next_tick(0).is_some());
        assert_eq!(pacer.until_next(0), 1_000_000);
        assert_eq!(pacer.until_next(400_000), 600_000);
        assert_eq!(pacer.until_next(2_000_000), 0);
    }

    #[test]
    fn pauses_are_left_out_of_recording_time() {
        let mut timeline = Timeline::new(100);
        assert_eq!(timeline.recording_time(400), 300);
        timeline.pause(400);
        assert!(timeline.is_paused());
        assert_eq!(timeline.recording_time(800), 300, "frozen while paused");
        timeline.resume(1_000);
        assert!(!timeline.is_paused());
        assert_eq!(timeline.recording_time(1_200), 500);
    }

    #[test]
    fn a_second_pause_and_a_resume_without_pause_change_nothing() {
        let mut timeline = Timeline::new(0);
        timeline.resume(50);
        assert_eq!(timeline.recording_time(50), 50);
        timeline.pause(100);
        timeline.pause(300);
        timeline.resume(400);
        assert_eq!(timeline.recording_time(500), 200, "the pause started at 100 and lasted 300");
    }

    #[test]
    fn crf_23_matches_the_table_at_1080p30() {
        assert_eq!(bitrate_bps(Quality::Crf(23), 1920, 1080, 30), 4_976_640);
        assert_eq!(bitrate_bps(Quality::Default, 1920, 1080, 30), 4_976_640);
    }

    #[test]
    fn explicit_bitrate_is_used_as_given() {
        assert_eq!(bitrate_bps(Quality::Bitrate(5_000), 1920, 1080, 30), 5_000_000);
        assert_eq!(bitrate_bps(Quality::Bitrate(0), 1920, 1080, 30), 4_976_640, "zero falls back to the default");
    }

    #[test]
    fn crf_bitrate_falls_as_crf_rises_and_stays_in_bounds() {
        let rates: Vec<u32> = (0..=51).map(|crf| bitrate_bps(Quality::Crf(crf), 1920, 1080, 30)).collect();
        assert!(rates.windows(2).all(|pair| pair[0] >= pair[1]), "non-increasing in CRF");
        assert_eq!(bitrate_bps(Quality::Crf(0), 7680, 4320, 120), 100_000_000, "capped");
        assert_eq!(bitrate_bps(Quality::Crf(51), 16, 16, 1), 250_000, "floored");
    }

    #[test]
    fn even_floor_rounds_down() {
        assert_eq!(even_floor(801), 800);
        assert_eq!(even_floor(800), 800);
        assert_eq!(even_floor(1), 0);
        assert_eq!(even_floor(0), 0);
    }

    #[test]
    fn only_h264_codecs_are_written() {
        assert!(is_h264(VideoCodec::X264));
        assert!(is_h264(VideoCodec::Nvenc));
        assert!(!is_h264(VideoCodec::X265));
        assert!(!is_h264(VideoCodec::Gif));
        assert!(!is_h264(VideoCodec::Vp9));
    }

    fn area(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect { x, y, width, height }
    }

    #[test]
    fn an_area_inside_the_monitor_is_copied_whole() {
        let visible = visible_part((1920, 1080), area(100, 200, 800, 600));
        assert_eq!(visible, Some(Visible { src_x: 100, src_y: 200, width: 800, height: 600, dst_x: 0, dst_y: 0 }));
    }

    #[test]
    fn an_area_starting_off_the_monitor_is_clipped_and_placed_inside_the_frame() {
        let visible = visible_part((1920, 1080), area(-50, -20, 100, 100));
        assert_eq!(visible, Some(Visible { src_x: 0, src_y: 0, width: 50, height: 80, dst_x: 50, dst_y: 20 }));
    }

    #[test]
    fn an_area_running_past_the_right_edge_is_clipped() {
        let visible = visible_part((1920, 1080), area(1800, 0, 400, 300));
        assert_eq!(visible, Some(Visible { src_x: 1800, src_y: 0, width: 120, height: 300, dst_x: 0, dst_y: 0 }));
    }

    #[test]
    fn an_area_off_the_monitor_has_no_visible_part() {
        assert_eq!(visible_part((1920, 1080), area(2000, 0, 100, 100)), None);
        assert_eq!(visible_part((1920, 1080), area(0, -200, 100, 100)), None);
    }

    #[test]
    fn copy_places_the_visible_part_and_blacks_out_the_rest() {
        // A 4 x 2 frame; the staging rows are 16 bytes apart and hold values 1..=32.
        let staging: Vec<u8> = (1..=32).collect();
        let mut frame = vec![9u8; 4 * 2 * 4];
        let visible = Visible { src_x: 0, src_y: 0, width: 2, height: 1, dst_x: 1, dst_y: 0 };
        copy_visible(&mut frame, &staging, 16, 4, 2, visible);
        assert_eq!(&frame[..4], &[0, 0, 0, 0]);
        assert_eq!(&frame[4..12], &staging[4..12]);
        assert_eq!(&frame[12..], &[0; 20], "the rest of the frame is black");
    }

    #[test]
    fn copy_of_the_whole_area_writes_every_row() {
        let staging: Vec<u8> = (0..32).collect();
        let mut frame = vec![0u8; 4 * 2 * 4];
        let visible = Visible { src_x: 0, src_y: 0, width: 4, height: 2, dst_x: 0, dst_y: 0 };
        copy_visible(&mut frame, &staging, 16, 4, 2, visible);
        assert_eq!(frame, staging);
    }
}
