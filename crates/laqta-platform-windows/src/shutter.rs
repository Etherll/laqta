//! The built-in capture sound: a short decaying noise burst, written as a WAV file in memory.

/// Sample rate of the sound, in hertz.
const RATE: u32 = 22_050;
/// Length of the burst: 80 ms.
const SAMPLES: u32 = RATE * 80 / 1000;
/// Time constant of the exponential decay, in seconds. The burst is near silent by the end.
const DECAY_SECONDS: f32 = 0.012;

/// The shutter sound as a complete WAV file: RIFF, PCM, mono, 16-bit, 22050 Hz. The noise comes from
/// a fixed-seed generator, so the output is the same on every call.
pub(crate) fn shutter_wav() -> Vec<u8> {
    let data_len = SAMPLES * 2;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    wav.extend_from_slice(&2u16.to_le_bytes()); // bytes per frame
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());

    let mut state: u32 = 0x9E37_79B9; // xorshift32, never zero
    for n in 0..SAMPLES {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let noise = state as f32 / u32::MAX as f32 * 2.0 - 1.0;
        let t = n as f32 / RATE as f32;
        let level = noise * (-t / DECAY_SECONDS).exp() * 0.6;
        let sample = (level * f32::from(i16::MAX)) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(wav: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([wav[at], wav[at + 1], wav[at + 2], wav[at + 3]])
    }

    #[test]
    fn header_describes_the_samples_that_follow() {
        let wav = shutter_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(field(&wav, 4) as usize, wav.len() - 8, "RIFF size");
        assert_eq!(field(&wav, 24), RATE, "sample rate");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(field(&wav, 40) as usize, wav.len() - 44, "data size");
        assert_eq!(wav.len(), 44 + 2 * 1764, "80 ms of 16-bit mono at 22050 Hz");
    }

    #[test]
    fn burst_is_loud_at_the_start_and_quiet_at_the_end() {
        let wav = shutter_wav();
        let peak = |range: std::ops::Range<usize>| {
            wav[44..][range].as_chunks::<2>().0.iter().map(|s| i32::from(i16::from_le_bytes(*s)).abs()).max().unwrap_or(0)
        };
        let head = peak(0..200);
        let tail = peak(wav.len() - 44 - 200..wav.len() - 44);
        assert!(head > 5_000, "head peak {head}");
        assert!(tail < head / 50, "tail peak {tail} against head {head}");
    }

    #[test]
    fn output_is_deterministic() {
        assert_eq!(shutter_wav(), shutter_wav());
    }
}
