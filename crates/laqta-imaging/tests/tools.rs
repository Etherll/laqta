//! Tests for the pixel and file tools (M5 D1). Every tool runs inside a compute pool, as `main` runs it.
#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

// Tests live in `mod tools` so that `cargo nextest run -p laqta-imaging tools` selects them.
mod tools {
    use std::borrow::Cow;

    use laqta_domain::color::Rgba;
    use laqta_domain::effects::draw::Placement;
    use laqta_domain::{Bitmap, ImageFormat, PixelFormat};
    use laqta_imaging::tools::combine::{Align, Orientation, combine};
    use laqta_imaging::tools::compare::compare;
    use laqta_imaging::tools::convert::{convert, resize_bytes};
    use laqta_imaging::tools::icon::{decode_ico, encode_ico};
    use laqta_imaging::tools::metadata::{read_exif, strip_metadata};
    use laqta_imaging::tools::split::{split_grid, split_size};
    use laqta_imaging::tools::thumbnail::thumbnails;
    use laqta_imaging::tools::watermark::watermark_image;
    use laqta_imaging::{CodecError, Codecs, EncodeOptions, sniff};

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    fn pool() -> rayon::ThreadPool {
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
    }

    fn in_pool<R: Send>(f: impl FnOnce() -> R + Send) -> R {
        pool().install(f)
    }

    /// An Rgba8 bitmap whose pixel (x, y) is `px(x, y)`.
    fn rgba(w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) -> Bitmap {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                out.extend_from_slice(&px(x, y));
            }
        }
        Bitmap::new(w, h, PixelFormat::Rgba8, out.into()).unwrap()
    }

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Bitmap {
        rgba(w, h, |_, _| px)
    }

    /// Opaque colour ramp, smooth enough for JPEG.
    fn gradient(w: u32, h: u32) -> Bitmap {
        rgba(w, h, |x, y| [(x * 16) as u8, (y * 24) as u8, ((x + y) * 8) as u8, 255])
    }

    fn at(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * b.width() + x) * 4) as usize;
        b.pixels()[i..i + 4].try_into().unwrap()
    }

    fn to_bgra(img: &Bitmap) -> Bitmap {
        let px: Vec<u8> = img.pixels().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        Bitmap::new(img.width(), img.height(), PixelFormat::Bgra8, px.into()).unwrap()
    }

    fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
        a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tol)
    }

    fn worst_error(a: &Bitmap, b: &Bitmap) -> u8 {
        a.pixels().iter().zip(b.pixels()).enumerate().filter(|(i, _)| i % 4 != 3).map(|(_, (x, y))| x.abs_diff(*y)).max().unwrap_or(0)
    }

    fn opts(format: ImageFormat) -> EncodeOptions {
        EncodeOptions { format, jpeg_quality: 90 }
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    // ---- combine ----

    #[test]
    fn combine_horizontal_places_images_with_spacing_and_background() {
        let out = in_pool(|| {
            combine(&[solid(2, 2, RED), solid(3, 1, BLUE)], Orientation::Horizontal, Align::Start, 1, Rgba::rgb(0, 255, 0), false)
        });
        assert_eq!((out.width(), out.height()), (6, 2));
        assert_eq!(out.format(), PixelFormat::Rgba8);
        assert_eq!(at(&out, 0, 0), RED);
        assert_eq!(at(&out, 1, 1), RED);
        assert_eq!(at(&out, 2, 0), GREEN, "gap");
        assert_eq!(at(&out, 2, 1), GREEN, "gap");
        assert_eq!(at(&out, 3, 0), BLUE);
        assert_eq!(at(&out, 5, 0), BLUE);
        assert_eq!(at(&out, 3, 1), GREEN, "the shorter image sits at the top");
    }

    #[test]
    fn combine_end_alignment_moves_short_images_to_the_bottom() {
        let out = in_pool(|| {
            combine(&[solid(2, 2, RED), solid(3, 1, BLUE)], Orientation::Horizontal, Align::End, 0, Rgba::rgb(0, 255, 0), false)
        });
        assert_eq!((out.width(), out.height()), (5, 2));
        assert_eq!(at(&out, 2, 0), GREEN, "above the short image");
        assert_eq!(at(&out, 2, 1), BLUE, "the short image sits at the bottom");
        assert_eq!(at(&out, 4, 1), BLUE);
        assert_eq!(at(&out, 4, 0), GREEN);
    }

    #[test]
    fn combine_vertical_centres_narrow_images() {
        let out = in_pool(|| {
            combine(&[solid(2, 2, RED), solid(4, 1, BLUE)], Orientation::Vertical, Align::Center, 0, Rgba::rgb(0, 255, 0), false)
        });
        assert_eq!((out.width(), out.height()), (4, 3));
        assert_eq!(at(&out, 0, 0), GREEN, "left margin of the red image");
        assert_eq!(at(&out, 1, 0), RED);
        assert_eq!(at(&out, 2, 1), RED);
        assert_eq!(at(&out, 3, 1), GREEN);
        assert_eq!(at(&out, 0, 2), BLUE);
        assert_eq!(at(&out, 3, 2), BLUE);
    }

    #[test]
    fn combine_auto_fill_scales_smaller_images_to_the_largest_cross_size() {
        let (filled, plain) = in_pool(|| {
            let images = [solid(4, 2, RED), solid(2, 1, BLUE)];
            (
                combine(&images, Orientation::Horizontal, Align::Start, 0, Rgba::rgb(0, 255, 0), true),
                combine(&images, Orientation::Horizontal, Align::Start, 0, Rgba::rgb(0, 255, 0), false),
            )
        });
        assert_eq!((filled.width(), filled.height()), (8, 2));
        for y in 0..2 {
            for x in 4..8 {
                assert!(near(at(&filled, x, y), BLUE, 1), "({x}, {y}) = {:?}", at(&filled, x, y));
            }
        }
        assert_eq!((plain.width(), plain.height()), (6, 2));
        assert_eq!(at(&plain, 4, 1), GREEN, "without auto fill the short image keeps its height");
    }

    #[test]
    fn combine_nothing_is_one_transparent_pixel() {
        let out = in_pool(|| combine(&[], Orientation::Vertical, Align::Start, 4, Rgba::TRANSPARENT, true));
        assert_eq!((out.width(), out.height()), (1, 1));
        assert_eq!(at(&out, 0, 0), CLEAR);
    }

    #[test]
    fn combine_transparent_pixels_show_the_background() {
        let img = rgba(2, 1, |x, _| if x == 0 { CLEAR } else { RED });
        let out = in_pool(|| combine(&[img], Orientation::Horizontal, Align::Start, 0, Rgba::rgb(0, 255, 0), false));
        assert_eq!(at(&out, 0, 0), GREEN);
        assert_eq!(at(&out, 1, 0), RED);
    }

    #[test]
    fn combine_accepts_bgra_and_matches_rgba() {
        let a = rgba(3, 2, |x, y| [(x * 60) as u8, (y * 90) as u8, 40, 128]);
        let b = solid(2, 2, BLUE);
        let (from_rgba, from_bgra) = in_pool(|| {
            let bg = Rgba::rgb(9, 9, 9);
            (
                combine(&[a.clone(), b.clone()], Orientation::Vertical, Align::Start, 1, bg, false),
                combine(&[to_bgra(&a), to_bgra(&b)], Orientation::Vertical, Align::Start, 1, bg, false),
            )
        });
        assert_eq!(from_rgba.pixels(), from_bgra.pixels());
        assert_eq!((from_rgba.width(), from_rgba.height()), (3, 5));
    }

    // ---- split ----

    #[test]
    fn split_grid_gives_the_remainder_to_the_last_row_and_column() {
        let img = gradient(13, 11);
        let tiles = in_pool(|| split_grid(&img, 3, 4));
        assert_eq!(tiles.len(), 12);
        let widths = [3, 3, 3, 4];
        let heights = [3, 3, 5];
        for (i, tile) in tiles.iter().enumerate() {
            assert_eq!((tile.width(), tile.height()), (widths[i % 4], heights[i / 4]), "tile {i}");
        }
    }

    #[test]
    fn split_grid_round_trip_restores_the_image() {
        let img = gradient(13, 11);
        let tiles = in_pool(|| split_grid(&img, 3, 4));
        let widths = [3usize, 3, 3, 4];
        let heights = [3usize, 3, 5];
        let mut canvas = vec![0u8; 13 * 11 * 4];
        let mut y0 = 0usize;
        for (r, &th) in heights.iter().enumerate() {
            let mut x0 = 0usize;
            for (c, &tw) in widths.iter().enumerate() {
                let tile = &tiles[r * 4 + c];
                for (y, line) in tile.pixels().chunks_exact(tw * 4).enumerate() {
                    let d = ((y0 + y) * 13 + x0) * 4;
                    canvas[d..d + tw * 4].copy_from_slice(line);
                }
                x0 += tw;
            }
            y0 += th;
        }
        assert_eq!(canvas, img.pixels());
    }

    #[test]
    fn split_grid_clamps_counts_to_the_image() {
        let img = gradient(4, 4);
        let whole = in_pool(|| split_grid(&img, 0, 0));
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].pixels(), img.pixels());
        let slices = in_pool(|| split_grid(&img, 100, 2));
        assert_eq!(slices.len(), 8);
        assert!(slices.iter().all(|t| t.width() == 2 && t.height() == 1));
    }

    #[test]
    fn split_size_tiles_with_smaller_edges() {
        let img = gradient(10, 7);
        let tiles = in_pool(|| split_size(&img, 4, 3));
        assert_eq!(tiles.len(), 9);
        let widths = [4, 4, 2];
        let heights = [3, 3, 1];
        for (i, tile) in tiles.iter().enumerate() {
            assert_eq!((tile.width(), tile.height()), (widths[i % 3], heights[i / 3]), "tile {i}");
        }
        assert_eq!(in_pool(|| split_size(&img, 0, 3)).len(), 3, "a zero width keeps the full width");
    }

    #[test]
    fn split_of_an_empty_image_has_no_tiles() {
        let empty = Bitmap::new(0, 0, PixelFormat::Rgba8, Vec::new().into()).unwrap();
        assert!(split_grid(&empty, 2, 2).is_empty());
        assert!(split_size(&empty, 2, 2).is_empty());
    }

    // ---- watermark ----

    #[test]
    fn watermark_blends_at_the_given_opacity() {
        let base = solid(8, 8, BLACK);
        let mark = solid(2, 2, WHITE);
        let out = in_pool(|| watermark_image(&base, &mark, Placement::BottomRight, (0, 0), 0.5, 100.0));
        for y in 6..8 {
            for x in 6..8 {
                assert!(near(at(&out, x, y), [128, 128, 128, 255], 1), "({x}, {y}) = {:?}", at(&out, x, y));
            }
        }
        assert_eq!(at(&out, 5, 5), BLACK);
        assert_eq!(at(&out, 0, 0), BLACK);
    }

    #[test]
    fn watermark_offset_moves_the_mark_and_clips_it() {
        let base = solid(4, 4, BLACK);
        let mark = solid(2, 2, WHITE);
        let moved = in_pool(|| watermark_image(&base, &mark, Placement::TopLeft, (1, 2), 1.0, 100.0));
        assert_eq!(at(&moved, 0, 2), BLACK);
        assert_eq!(at(&moved, 1, 2), WHITE);
        assert_eq!(at(&moved, 2, 3), WHITE);
        assert_eq!(at(&moved, 3, 3), BLACK);
        let clipped = in_pool(|| watermark_image(&base, &mark, Placement::TopLeft, (-1, -1), 1.0, 100.0));
        assert_eq!(at(&clipped, 0, 0), WHITE, "only the mark's bottom-right pixel lands on the picture");
        assert_eq!(at(&clipped, 1, 0), BLACK);
        assert_eq!(at(&clipped, 0, 1), BLACK);
    }

    #[test]
    fn watermark_scale_resamples_the_mark() {
        let base = solid(8, 8, BLACK);
        let mark = solid(4, 4, WHITE);
        let out = in_pool(|| watermark_image(&base, &mark, Placement::BottomRight, (0, 0), 1.0, 50.0));
        assert!(near(at(&out, 6, 6), WHITE, 1));
        assert!(near(at(&out, 7, 7), WHITE, 1));
        assert_eq!(at(&out, 5, 5), BLACK, "the mark is 2x2 after scaling to 50%");
    }

    #[test]
    fn watermark_with_a_transparent_mark_changes_nothing() {
        let base = gradient(6, 5);
        let out = in_pool(|| watermark_image(&base, &solid(3, 3, CLEAR), Placement::MiddleCenter, (0, 0), 1.0, 100.0));
        assert_eq!(out.pixels(), base.pixels());
    }

    #[test]
    fn watermark_places_all_nine_anchors() {
        let base = solid(9, 9, BLACK);
        let mark = solid(3, 3, WHITE);
        let cases = [
            (Placement::TopLeft, (0, 0)),
            (Placement::TopCenter, (3, 0)),
            (Placement::TopRight, (6, 0)),
            (Placement::MiddleLeft, (0, 3)),
            (Placement::MiddleCenter, (3, 3)),
            (Placement::MiddleRight, (6, 3)),
            (Placement::BottomLeft, (0, 6)),
            (Placement::BottomCenter, (3, 6)),
            (Placement::BottomRight, (6, 6)),
        ];
        for (placement, (ox, oy)) in cases {
            let out = in_pool(|| watermark_image(&base, &mark, placement, (0, 0), 1.0, 100.0));
            assert_eq!(at(&out, ox, oy), WHITE, "{placement:?}");
            let white = out.pixels().as_chunks::<4>().0.iter().filter(|p| **p == WHITE).count();
            assert_eq!(white, 9, "{placement:?}");
        }
    }

    #[test]
    fn watermark_accepts_bgra_and_returns_rgba_of_the_picture_size() {
        let base = to_bgra(&solid(5, 4, BLACK));
        let out = in_pool(|| watermark_image(&base, &to_bgra(&solid(1, 1, WHITE)), Placement::TopLeft, (0, 0), 1.0, 100.0));
        assert_eq!((out.width(), out.height(), out.format()), (5, 4, PixelFormat::Rgba8));
        assert_eq!(at(&out, 0, 0), WHITE);
    }

    // ---- thumbnail ----

    #[test]
    fn thumbnails_bound_each_image_and_keep_order() {
        let images = [solid(100, 50, RED), solid(10, 10, BLUE), solid(200, 20, GREEN)];
        let out = in_pool(|| thumbnails(&images, 20, 20));
        let dims: Vec<(u32, u32)> = out.iter().map(|b| (b.width(), b.height())).collect();
        assert_eq!(dims, [(20, 10), (10, 10), (20, 2)]);
        assert!(near(at(&out[0], 0, 0), RED, 1));
        assert!(near(at(&out[2], 0, 0), GREEN, 1));
    }

    // ---- compare ----

    #[test]
    fn compare_identical_images_have_no_changes() {
        let a = gradient(6, 4);
        let diff = in_pool(|| compare(&a, &a, 0));
        assert_eq!((diff.changed, diff.total, diff.max_delta), (0, 24, 0));
        assert!(diff.psnr.is_infinite() && diff.psnr > 0.0);
        assert_eq!((diff.image.width(), diff.image.height()), (6, 4));
        assert!(diff.image.pixels().as_chunks::<4>().0.iter().all(|p| *p != RED), "nothing is marked red");
    }

    #[test]
    fn compare_counts_pixels_over_the_tolerance() {
        let a = solid(4, 4, BLACK);
        let b = rgba(4, 4, |x, y| if (x, y) == (1, 1) { [10, 0, 0, 255] } else { BLACK });
        let strict = compare(&a, &b, 5);
        assert_eq!((strict.changed, strict.total, strict.max_delta), (1, 16, 10));
        assert_eq!(at(&strict.image, 1, 1), RED);
        let loose = compare(&a, &b, 10);
        assert_eq!((loose.changed, loose.max_delta), (0, 10));
    }

    #[test]
    fn compare_different_sizes_counts_the_uncovered_area_as_changed() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = solid(2, 3, [10, 20, 30, 255]);
        let diff = in_pool(|| compare(&a, &b, 0));
        assert_eq!((diff.total, diff.changed, diff.max_delta), (16, 10, 0));
        assert_eq!((diff.image.width(), diff.image.height()), (4, 4));
        assert_eq!(at(&diff.image, 3, 0), RED, "uncovered by b");
        assert_ne!(at(&diff.image, 0, 0), RED);
        assert!(diff.psnr.is_infinite(), "the overlap is identical");
    }

    #[test]
    fn compare_psnr_of_black_against_white_is_about_1_25_db() {
        let diff = compare(&solid(1, 1, BLACK), &solid(1, 1, WHITE), 0);
        assert_eq!(diff.changed, 1);
        assert_eq!(diff.max_delta, 255);
        // Three channels differ by 255 and alpha matches: mse = 3 * 255^2 / 4.
        assert!((diff.psnr - 1.249_387).abs() < 1e-4, "psnr {}", diff.psnr);
    }

    #[test]
    fn compare_reads_bgra_as_rgba() {
        let a = gradient(5, 3);
        let diff = compare(&a, &to_bgra(&a), 0);
        assert_eq!(diff.changed, 0);
    }

    // ---- icon ----

    #[test]
    fn ico_round_trip_has_each_size_and_letterboxes_non_square_images() {
        let img = gradient(40, 20);
        let sizes = [16, 32, 48, 256];
        let ico = in_pool(|| encode_ico(&img, &sizes)).unwrap();
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), 4);
        let decoded = decode_ico(&ico).unwrap();
        assert_eq!(decoded.len(), 4);
        for (bitmap, s) in decoded.iter().zip(sizes) {
            assert_eq!((bitmap.width(), bitmap.height()), (s, s));
        }
        // 40x20 fits 16x16 as 16x8 centred vertically, so the top corner is transparent.
        assert_eq!(at(&decoded[0], 0, 0)[3], 0);
        assert_eq!(at(&decoded[0], 8, 8)[3], 255);
        assert_eq!(at(&decoded[3], 0, 0)[3], 0);
        assert_eq!(at(&decoded[3], 128, 128)[3], 255);
    }

    #[test]
    fn ico_encode_rejects_bad_sizes_and_empty_images() {
        let img = gradient(4, 4);
        in_pool(|| {
            assert!(matches!(encode_ico(&img, &[]), Err(CodecError::Encode(_))));
            assert!(matches!(encode_ico(&img, &[0]), Err(CodecError::Encode(_))));
            assert!(matches!(encode_ico(&img, &[257]), Err(CodecError::Encode(_))));
            let empty = Bitmap::new(0, 0, PixelFormat::Rgba8, Vec::new().into()).unwrap();
            assert!(matches!(encode_ico(&empty, &[16]), Err(CodecError::Encode(_))));
        });
    }

    /// An ICO file with one 2x2 32-bit BMP entry. Pixels are BGRA, bottom row first, and the AND mask marks the top-right
    /// pixel transparent. `xor_alpha` is the alpha byte written with the pixels.
    fn bmp_ico(xor_alpha: u8) -> Vec<u8> {
        let mut ico = vec![0, 0, 1, 0, 1, 0];
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&4i32.to_le_bytes()); // image plus AND mask
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        // Bottom row first: blue, white. Then top row: red, green.
        dib.extend_from_slice(&[255, 0, 0, xor_alpha, 255, 255, 255, xor_alpha]);
        dib.extend_from_slice(&[0, 0, 255, xor_alpha, 0, 255, 0, xor_alpha]);
        // AND mask, 4 bytes a row, bit 7 is the left pixel and a set bit is transparent.
        dib.extend_from_slice(&[0x00, 0, 0, 0]);
        dib.extend_from_slice(&[0x40, 0, 0, 0]);
        ico.extend_from_slice(&[2, 2, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&dib);
        ico
    }

    #[test]
    fn ico_decodes_bmp_entries_with_their_and_mask() {
        let decoded = decode_ico(&bmp_ico(0)).unwrap();
        assert_eq!(decoded.len(), 1);
        let img = &decoded[0];
        assert_eq!((img.width(), img.height()), (2, 2));
        assert_eq!(at(img, 0, 0), RED);
        assert_eq!(at(img, 1, 0)[3], 0, "the mask makes the top-right pixel transparent");
        assert_eq!(at(img, 0, 1), BLUE);
        assert_eq!(at(img, 1, 1), WHITE);
    }

    #[test]
    fn ico_bmp_entry_alpha_wins_over_the_mask() {
        let img = &decode_ico(&bmp_ico(255)).unwrap()[0];
        assert_eq!(at(img, 1, 0)[3], 255);
    }

    #[test]
    fn ico_decode_rejects_non_icon_bytes() {
        assert!(matches!(decode_ico(b"not an icon"), Err(CodecError::Decode(_))));
    }

    // ---- convert and resize ----

    #[test]
    fn convert_png_to_jpeg_and_back() {
        let codecs = Codecs::builtin();
        let img = gradient(12, 10);
        let jpeg = in_pool(|| {
            let png = codecs.encode(&img, &opts(ImageFormat::Png)).unwrap();
            convert(&png, &codecs, opts(ImageFormat::Jpeg)).unwrap()
        });
        assert_eq!(sniff(&jpeg), Some(ImageFormat::Jpeg));
        let back = codecs.decode(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (12, 10));
        assert!(worst_error(&img, &back) <= 12);
    }

    #[test]
    fn convert_to_bmp_is_lossless() {
        let codecs = Codecs::builtin();
        let img = gradient(7, 3);
        let bmp = in_pool(|| {
            let png = codecs.encode(&img, &opts(ImageFormat::Png)).unwrap();
            convert(&png, &codecs, opts(ImageFormat::Bmp)).unwrap()
        });
        assert_eq!(sniff(&bmp), Some(ImageFormat::Bmp));
        assert_eq!(codecs.decode(&bmp).unwrap().pixels(), img.pixels());
    }

    #[test]
    fn resize_bytes_fits_or_sets_the_exact_size() {
        let codecs = Codecs::builtin();
        let img = gradient(40, 20);
        let png = in_pool(|| codecs.encode(&img, &opts(ImageFormat::Png))).unwrap();
        let size = |bytes: Result<Vec<u8>, CodecError>| {
            let b = codecs.decode(&bytes.unwrap()).unwrap();
            (b.width(), b.height())
        };
        in_pool(|| {
            let p = opts(ImageFormat::Png);
            assert_eq!(size(resize_bytes(&png, &codecs, 10, 10, true, p)), (10, 5), "fit inside 10x10");
            assert_eq!(size(resize_bytes(&png, &codecs, 80, 80, true, p)), (80, 40), "fit may enlarge");
            assert_eq!(size(resize_bytes(&png, &codecs, 0, 0, true, p)), (40, 20), "unbounded keeps the size");
            assert_eq!(size(resize_bytes(&png, &codecs, 7, 3, false, p)), (7, 3), "exact size");
            assert_eq!(size(resize_bytes(&png, &codecs, 0, 10, false, p)), (40, 10), "a zero side keeps the source size");
        });
    }

    // ---- metadata ----

    fn segment(code: u8, payload: &[u8]) -> Vec<u8> {
        let mut s = vec![0xFF, code];
        s.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        s.extend_from_slice(payload);
        s
    }

    /// An APP1 segment holding one EXIF tag, Make = "Acme", as a little-endian TIFF.
    fn exif_app1() -> Vec<u8> {
        let mut tiff = b"II*\0".to_vec();
        tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 at offset 8
        tiff.extend_from_slice(&1u16.to_le_bytes()); // one entry
        tiff.extend_from_slice(&0x010Fu16.to_le_bytes()); // Make
        tiff.extend_from_slice(&2u16.to_le_bytes()); // ASCII
        tiff.extend_from_slice(&5u32.to_le_bytes()); // "Acme" and its NUL
        tiff.extend_from_slice(&26u32.to_le_bytes()); // value offset
        tiff.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        tiff.extend_from_slice(b"Acme\0");
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        segment(0xE1, &payload)
    }

    /// Inserts `segment` right after the SOI marker of a JPEG.
    fn insert_after_soi(jpeg: &[u8], segment: &[u8]) -> Vec<u8> {
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(segment);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    /// Marker codes from the start of a JPEG up to its start of scan.
    fn jpeg_markers(b: &[u8]) -> Vec<u8> {
        let mut codes = Vec::new();
        let mut i = 2;
        while i + 1 < b.len() && b[i] == 0xFF {
            let code = b[i + 1];
            codes.push(code);
            if code == 0xDA || code == 0xD9 {
                break;
            }
            i += 2 + u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        }
        codes
    }

    fn jpeg_of(img: &Bitmap) -> Vec<u8> {
        in_pool(|| Codecs::builtin().encode(img, &EncodeOptions { format: ImageFormat::Jpeg, jpeg_quality: 90 })).unwrap()
    }

    #[test]
    fn read_exif_finds_tags_in_an_app1_segment() {
        let jpeg = insert_after_soi(&jpeg_of(&gradient(8, 8)), &exif_app1());
        let fields = read_exif(&jpeg);
        assert!(fields.iter().any(|(tag, value)| tag == "Make" && value.contains("Acme")), "{fields:?}");
        assert!(read_exif(&jpeg_of(&gradient(8, 8))).is_empty(), "no EXIF, no fields");
        assert!(read_exif(b"plain text").is_empty());
    }

    #[test]
    fn strip_removes_exif_and_comments_and_keeps_the_icc_profile_and_pixels() {
        let img = gradient(8, 8);
        let icc = segment(0xE2, b"ICC_PROFILE\0\x01\x01 colour-profile");
        let with_meta = insert_after_soi(&insert_after_soi(&jpeg_of(&img), &exif_app1()), &segment(0xFE, b"hello comment"));
        let input = insert_after_soi(&with_meta, &icc);
        assert!(!read_exif(&input).is_empty());

        let stripped = strip_metadata(&input).unwrap();
        assert!(read_exif(&stripped).is_empty());
        assert!(!contains(&stripped, b"Exif\0\0"));
        assert!(!contains(&stripped, b"hello comment"));
        let markers = jpeg_markers(&stripped);
        assert!(!markers.contains(&0xE1) && !markers.contains(&0xFE), "{markers:x?}");
        assert!(markers.contains(&0xE2), "the ICC profile stays: {markers:x?}");
        assert!(markers.contains(&0xDB) && markers.contains(&0xC0), "the image tables stay: {markers:x?}");
        assert!(contains(&stripped, b"JFIF\0"), "the JFIF header stays");

        let back = Codecs::builtin().decode(&stripped).unwrap();
        assert_eq!((back.width(), back.height()), (8, 8));
        assert!(worst_error(&img, &back) <= 12);
    }

    /// CRC-32 over `data`, as PNG chunks need.
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }

    /// A chunk with a correct length and CRC.
    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[4..]);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }

    /// A PNG with text, compressed text, international text, EXIF and a time chunk.
    fn png_with_metadata(img: &Bitmap) -> Vec<u8> {
        let mut info = png::Info::with_size(img.width(), img.height());
        info.color_type = png::ColorType::Rgba;
        info.bit_depth = png::BitDepth::Eight;
        info.exif_metadata = Some(Cow::Owned(b"Exif\0\0secret".to_vec()));
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::with_info(&mut out, info).unwrap();
            enc.add_text_chunk("Comment".into(), "secret text".into()).unwrap();
            enc.add_ztxt_chunk("Software".into(), "secret z".into()).unwrap();
            enc.add_itxt_chunk("Title".into(), "secret i".into()).unwrap();
            let mut writer = enc.write_header().unwrap();
            writer.write_image_data(img.pixels()).unwrap();
            writer.finish().unwrap();
        }
        // Add a tIME chunk just before IEND, which is the last 12 bytes.
        let iend = out.len() - 12;
        let time = png_chunk(b"tIME", &[0x07, 0xE6, 1, 2, 3, 4, 5]);
        let mut with_time = out[..iend].to_vec();
        with_time.extend_from_slice(&time);
        with_time.extend_from_slice(&out[iend..]);
        with_time
    }

    fn png_chunk_names(b: &[u8]) -> Vec<String> {
        let mut names = Vec::new();
        let mut i = 8;
        while i + 8 <= b.len() {
            let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
            names.push(String::from_utf8_lossy(&b[i + 4..i + 8]).into_owned());
            i += 12 + len;
        }
        names
    }

    #[test]
    fn strip_removes_png_text_exif_and_time_chunks() {
        let img = gradient(6, 5);
        let input = png_with_metadata(&img);
        let before = png_chunk_names(&input);
        for kind in ["tEXt", "zTXt", "iTXt", "eXIf", "tIME"] {
            assert!(before.iter().any(|n| n == kind), "fixture has {kind}: {before:?}");
        }

        let stripped = strip_metadata(&input).unwrap();
        let after = png_chunk_names(&stripped);
        assert_eq!(after, ["IHDR", "IDAT", "IEND"]);
        assert!(!contains(&stripped, b"secret"));
        assert_eq!(Codecs::builtin().decode(&stripped).unwrap().pixels(), img.pixels());
    }

    /// A lossless WebP with EXIF and XMP chunks added in an extended (VP8X) container.
    fn webp_with_metadata(img: &Bitmap) -> Vec<u8> {
        let simple = in_pool(|| Codecs::builtin().encode(img, &opts(ImageFormat::WebP))).unwrap();
        let mut body = b"WEBP".to_vec();
        let mut vp8x = vec![0x08 | 0x04, 0, 0, 0];
        vp8x.extend_from_slice(&(img.width() - 1).to_le_bytes()[..3]);
        vp8x.extend_from_slice(&(img.height() - 1).to_le_bytes()[..3]);
        body.extend_from_slice(b"VP8X");
        body.extend_from_slice(&(vp8x.len() as u32).to_le_bytes());
        body.extend_from_slice(&vp8x);
        // Copy the image chunks, dropping any VP8X the encoder wrote.
        let mut i = 12;
        while i + 8 <= simple.len() {
            let size = u32::from_le_bytes([simple[i + 4], simple[i + 5], simple[i + 6], simple[i + 7]]) as usize;
            let end = (i + 8 + size + (size & 1)).min(simple.len());
            if &simple[i..i + 4] != b"VP8X" {
                body.extend_from_slice(&simple[i..end]);
            }
            i = end;
        }
        for (kind, data) in [(b"EXIF", &b"Exif\0\0odd"[..]), (b"XMP ", &b"<x:xmpmeta/>"[..])] {
            body.extend_from_slice(kind);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn webp_chunk_names(b: &[u8]) -> Vec<String> {
        let mut names = Vec::new();
        let mut i = 12;
        while i + 8 <= b.len() {
            let size = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
            names.push(String::from_utf8_lossy(&b[i..i + 4]).into_owned());
            i += 8 + size + (size & 1);
        }
        names
    }

    #[test]
    fn strip_removes_webp_exif_and_xmp_and_clears_their_flags() {
        let img = gradient(9, 6);
        let input = webp_with_metadata(&img);
        assert_eq!(webp_chunk_names(&input), ["VP8X", "VP8L", "EXIF", "XMP "]);

        let stripped = strip_metadata(&input).unwrap();
        assert_eq!(webp_chunk_names(&stripped), ["VP8X", "VP8L"]);
        assert_eq!(stripped[20] & 0x0C, 0, "EXIF and XMP flags are cleared");
        assert_eq!(u32::from_le_bytes([stripped[4], stripped[5], stripped[6], stripped[7]]) as usize, stripped.len() - 8);
        let back = Codecs::builtin().decode(&stripped).unwrap();
        assert_eq!(back.pixels(), img.pixels());
    }

    #[test]
    fn strip_rejects_formats_it_does_not_handle() {
        let img = gradient(4, 4);
        let gif = in_pool(|| Codecs::builtin().encode(&img, &opts(ImageFormat::Gif))).unwrap();
        assert!(matches!(strip_metadata(&gif), Err(CodecError::Unsupported(ImageFormat::Gif))));
        assert!(matches!(strip_metadata(b"not an image"), Err(CodecError::Unknown)));
    }
}
