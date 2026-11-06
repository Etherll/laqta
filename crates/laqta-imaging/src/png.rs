use laqta_domain::{Bitmap, PixelFormat};

#[derive(Debug, Clone, thiserror::Error)]
pub enum EncodeError {
    #[error("png encode failed: {0}")]
    Png(String),
}

/// Encodes an RGBA or BGRA bitmap as an 8-bit RGBA PNG.
///
/// Compression level: `Balanced`, pending the step 3 bench of `Fast` vs `Balanced` on real
/// screenshots (the faster level is kept only if its files are within 2x of `Balanced`).
pub fn encode_png(image: &Bitmap) -> Result<Vec<u8>, EncodeError> {
    debug_assert!(rayon::current_thread_index().is_some(), "encode_png must run inside the compute pool");
    encode_with(image, png::Compression::Balanced)
}

#[doc(hidden)]
pub fn encode_with(image: &Bitmap, compression: png::Compression) -> Result<Vec<u8>, EncodeError> {
    let err = |e: png::EncodingError| EncodeError::Png(e.to_string());
    let rgba: std::borrow::Cow<'_, [u8]> = match image.format() {
        PixelFormat::Rgba8 => image.pixels().into(),
        PixelFormat::Bgra8 => image.pixels().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect::<Vec<u8>>().into(),
    };
    let mut out = Vec::with_capacity(rgba.len() / 4);
    {
        let mut enc = png::Encoder::new(&mut out, image.width(), image.height());
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(compression);
        let mut w = enc.write_header().map_err(err)?;
        w.write_image_data(&rgba).map_err(err)?;
        w.finish().map_err(err)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn round_trip() {
        let px: Vec<u8> = (0..(5 * 3 * 4)).map(|i| i as u8).collect();
        let bmp = Bitmap::new(5, 3, PixelFormat::Rgba8, px.clone().into()).unwrap();
        #[allow(clippy::disallowed_types)]
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let bytes = pool.install(|| encode_png(&bmp)).unwrap();
        let dec = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (5, 3));
        assert_eq!(&buf[..info.buffer_size()], &px[..]);
    }
}
