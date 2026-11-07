#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Tests for the M5 tool logic: the pure formats and maths, and the file, media, OCR and window tools over fakes.
//! `laqta-imaging` asserts that its pixel work runs in a rayon pool, as the compute pool does, so each test body runs
//! inside one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures_lite::future::block_on;
use laqta_app::{
    ColorFormats, ColorHistory, HashKind, IndexFormat, IndexOptions, Ports, WindowToggles, combine_files, compare_files, compare_hash,
    convert_files, convert_video, hash_file, hash_text, index_folder, inspect_active, make_gif, make_icon, measure, ocr_image, parse_color,
    px_to_cm, px_to_inches, qr_decode_file, qr_decode_image, qr_decode_screen, qr_image, read_metadata, resize_files, split_file,
    strip_metadata_files, thumbnail_files, toggle_borderless_active, toggle_topmost_active, trim_video, video_thumbnails, watermark_files,
};
use laqta_domain::color::Rgba;
use laqta_domain::effects::draw::Placement;
use laqta_domain::geometry::{MonitorId, PhysRect};
use laqta_domain::record::{Quality, RecordSpec, VideoCodec};
use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_domain::{Bitmap, Frame, ImageFormat, Monitor, PixelFormat};
use laqta_imaging::tools::combine::{Align, Orientation};
use laqta_imaging::tools::icon::decode_ico;
use laqta_imaging::{Codecs, encode_png, sniff};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeEditorHost, FakeHoverSource, FakeNotifier, FakeOcr, FakeOverlay, FakePrompt,
    FakeScreen, FakeScreenStream, FakeShell, FakeToolHost, FakeUploaders, FakeWindowOps, InlineSpawner, ManualClock,
};
use laqta_ports::{BoxFuture, HostInfo, Media, MediaInfo, OcrLine, PortError, RecordingHandle, RecordingInput, TranscodeSpec};
use tempfile::TempDir;

/// Runs `body` on a rayon pool thread, where the compute pool's work runs.
fn on_pool<R: Send>(body: impl FnOnce() -> R + Send) -> R {
    rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap().install(body)
}

/// Records every transcode and probe. The duration of a probed file is `duration`.
#[derive(Debug)]
struct SpyMedia {
    transcodes: Mutex<Vec<(PathBuf, PathBuf, TranscodeSpec)>>,
    probes: Mutex<Vec<PathBuf>>,
    duration: f64,
}

impl Default for SpyMedia {
    fn default() -> Self {
        SpyMedia { transcodes: Mutex::new(Vec::new()), probes: Mutex::new(Vec::new()), duration: 3.0 }
    }
}

impl Media for SpyMedia {
    fn start(
        &self,
        _input: RecordingInput,
        _spec: RecordSpec,
        _output: PathBuf,
    ) -> BoxFuture<'_, Result<Box<dyn RecordingHandle>, PortError>> {
        Box::pin(async { Err(PortError::Unsupported("recording".into())) })
    }

    fn transcode(&self, input: &Path, output: &Path, spec: TranscodeSpec) -> BoxFuture<'_, Result<(), PortError>> {
        self.transcodes.lock().unwrap().push((input.to_path_buf(), output.to_path_buf(), spec));
        Box::pin(async { Ok(()) })
    }

    fn probe(&self, input: &Path) -> BoxFuture<'_, Result<MediaInfo, PortError>> {
        self.probes.lock().unwrap().push(input.to_path_buf());
        let duration_secs = self.duration;
        Box::pin(async move {
            Ok(MediaInfo {
                duration_secs,
                width: 640,
                height: 480,
                fps: 30.0,
                video_codec: "h264".into(),
                audio_codec: None,
                size_bytes: 1000,
            })
        })
    }
}

/// Ports made of fakes, with the ones a test inspects kept reachable.
struct Harness {
    ports: Ports,
    media: Arc<SpyMedia>,
    hover: Arc<FakeHoverSource>,
    window_ops: Arc<FakeWindowOps>,
    ocr: Arc<FakeOcr>,
}

fn harness(frames: Vec<Frame>) -> Harness {
    let media = Arc::new(SpyMedia::default());
    let hover = Arc::new(FakeHoverSource::new(Vec::new()));
    let window_ops = Arc::new(FakeWindowOps::default());
    let ocr = Arc::new(FakeOcr::default());
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(frames)),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: Arc::new(InlineSpawner::default()),
        overlay: Arc::new(FakeOverlay::default()),
        tools: Arc::new(FakeToolHost::default()),
        prompt: Arc::new(FakePrompt::default()),
        editor: Arc::new(FakeEditorHost::default()),
        media: media.clone(),
        stream: Arc::new(FakeScreenStream::default()),
        ocr: ocr.clone(),
        window_ops: window_ops.clone(),
        uploaders: Arc::new(FakeUploaders::default()),
        hover: hover.clone(),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: Codecs::builtin(),
    };
    Harness { ports, media, hover, window_ops, ocr }
}

fn write(path: PathBuf, bytes: &[u8]) -> PathBuf {
    std::fs::write(&path, bytes).unwrap();
    path
}

fn solid(width: u32, height: u32, pixel: [u8; 4]) -> Bitmap {
    let pixels: Vec<u8> = std::iter::repeat_n(pixel, (width * height) as usize).flatten().collect();
    Bitmap::new(width, height, PixelFormat::Rgba8, pixels.into()).unwrap()
}

fn png(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
    encode_png(&solid(width, height, pixel)).unwrap()
}

fn decode(bytes: &[u8]) -> Bitmap {
    Codecs::builtin().decode(bytes).unwrap()
}

#[test]
fn colour_formats_of_a_known_colour() {
    let formats = ColorFormats::from_rgba(Rgba::rgb(0x33, 0x66, 0x99));
    assert_eq!(formats.hex, "#336699");
    assert_eq!(formats.rgb, "rgb(51, 102, 153)");
    assert_eq!(formats.rgba, "rgba(51, 102, 153, 1)");
    assert_eq!(formats.hsl, "hsl(210, 50%, 40%)");
    assert_eq!(formats.hsv, "hsv(210, 67%, 60%)");
    assert_eq!(formats.cmyk, "cmyk(67%, 33%, 0%, 40%)");
    assert_eq!(formats.decimal, "3368601");
    assert_eq!(formats.linear, "linear(0.0331, 0.1329, 0.3185)");
}

#[test]
fn colour_formats_of_the_edges() {
    let black = ColorFormats::from_rgba(Rgba::rgb(0, 0, 0));
    assert_eq!(black.hsl, "hsl(0, 0%, 0%)");
    assert_eq!(black.cmyk, "cmyk(0%, 0%, 0%, 100%)");

    let white = ColorFormats::from_rgba(Rgba::WHITE);
    assert_eq!(white.hsv, "hsv(0, 0%, 100%)");
    assert_eq!(white.cmyk, "cmyk(0%, 0%, 0%, 0%)");
    assert_eq!(white.decimal, "16777215");

    let translucent = ColorFormats::from_rgba(Rgba { r: 0x33, g: 0x66, b: 0x99, a: 128 });
    assert_eq!(translucent.hex, "#33669980");
    assert_eq!(translucent.rgba, "rgba(51, 102, 153, 0.502)");
}

#[test]
fn parse_reads_every_format_the_picker_writes() {
    let sample = Rgba::rgb(0x33, 0x66, 0x99);
    let formats = ColorFormats::from_rgba(sample);
    assert_eq!(parse_color(&formats.hex), Some(sample));
    assert_eq!(parse_color(&formats.rgb), Some(sample));
    assert_eq!(parse_color(&formats.hsl), Some(sample));
    assert_eq!(parse_color(&formats.rgba), Some(sample));
    assert_eq!(parse_color("336699"), Some(sample));

    for r in (0..=255).step_by(15) {
        for g in (0..=255).step_by(15) {
            for b in (0..=255).step_by(15) {
                let color = Rgba::rgb(r, g, b);
                let formats = ColorFormats::from_rgba(color);
                assert_eq!(parse_color(&formats.hex), Some(color));
                assert_eq!(parse_color(&formats.rgb), Some(color));
            }
        }
    }
}

#[test]
fn parse_reads_short_forms_alpha_names_and_rejects_the_rest() {
    assert_eq!(parse_color("#fff"), Some(Rgba::WHITE));
    assert_eq!(parse_color("#0008"), Some(Rgba { r: 0, g: 0, b: 0, a: 0x88 }));
    assert_eq!(parse_color("rgba(0, 0, 0, 50%)"), Some(Rgba { r: 0, g: 0, b: 0, a: 128 }));
    assert_eq!(parse_color("hsla(120, 100%, 50%, 0.5)"), Some(Rgba { r: 0, g: 255, b: 0, a: 128 }));
    assert_eq!(parse_color("  Teal "), Some(Rgba::rgb(0, 128, 128)));
    assert_eq!(parse_color("navy"), Some(Rgba::rgb(0, 0, 128)));

    for text in ["nope", "#12", "#gggggg", "rgb(256, 0, 0)", "hsl(10, 200%, 50%)", "rgb(1, 2)", ""] {
        assert_eq!(parse_color(text), None, "{text}");
    }
}

#[test]
fn colour_history_keeps_the_newest_32_colours_once_each() {
    let mut history = ColorHistory::default();
    assert!(history.is_empty());
    for i in 0..40u8 {
        history.push(Rgba::rgb(i, 0, 0));
    }
    assert_eq!(history.len(), ColorHistory::CAPACITY);
    assert_eq!(history.iter().next(), Some(Rgba::rgb(39, 0, 0)));

    history.push(Rgba::rgb(20, 0, 0));
    assert_eq!(history.len(), ColorHistory::CAPACITY);
    assert_eq!(history.iter().next(), Some(Rgba::rgb(20, 0, 0)));
    assert_eq!(history.iter().filter(|color| *color == Rgba::rgb(20, 0, 0)).count(), 1);
}

#[test]
fn ruler_measures_the_span_of_two_points() {
    let m = measure((10, 20), (13, 24));
    assert_eq!((m.width, m.height), (3.0, 4.0));
    assert!((m.diagonal - 5.0).abs() < 1e-9);
    assert!((m.angle_deg - 53.130_102).abs() < 1e-4);
    assert_eq!((m.area, m.perimeter), (12, 14));

    let back = measure((13, 24), (10, 20));
    assert!((back.angle_deg - 233.130_102).abs() < 1e-4);
    assert_eq!(back.area, 12);
    assert_eq!(measure((5, 5), (5, 5)).diagonal, 0.0);
}

#[test]
fn ruler_converts_pixels_at_a_dpi() {
    assert_eq!(px_to_inches(192.0, 96.0), Some(2.0));
    assert!((px_to_cm(96.0, 96.0).unwrap() - 2.54).abs() < 1e-12);
    assert_eq!(px_to_inches(10.0, 0.0), None);
    assert_eq!(px_to_cm(10.0, f64::NAN), None);
}

#[test]
fn text_digests_match_the_standard_vectors() {
    assert_eq!(hash_text("abc", HashKind::Md5), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(hash_text("abc", HashKind::Sha1), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(hash_text("abc", HashKind::Sha256), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(hash_text("abc", HashKind::Crc32), "352441c2");
    assert_eq!(hash_text("", HashKind::Md5), "d41d8cd98f00b204e9800998ecf8427e");
    assert!(compare_hash(" 352441C2 ", &hash_text("abc", HashKind::Crc32)));
}

#[test]
fn file_digests_match_text_digests_across_chunks() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            // 2.5 MB of text: three 1 MiB chunks and a remainder.
            let text: String = (0..250_000).map(|i| format!("{i:09}\n")).collect();
            let path = write(dir.path().join("big.txt"), text.as_bytes());

            let digests = hash_file(&h.ports, &path, &HashKind::ALL).await.unwrap();
            assert_eq!(digests.len(), HashKind::ALL.len());
            for (kind, digest) in digests {
                assert_eq!(digest, hash_text(&text, kind), "{kind:?}");
            }

            let empty = write(dir.path().join("empty.bin"), b"");
            let md5 = hash_file(&h.ports, &empty, &[HashKind::Md5]).await.unwrap();
            assert_eq!(md5, vec![(HashKind::Md5, "d41d8cd98f00b204e9800998ecf8427e".to_owned())]);

            assert!(hash_file(&h.ports, &empty, &[]).await.unwrap().is_empty());
            assert!(hash_file(&h.ports, &dir.path().join("missing"), &[HashKind::Md5]).await.is_err());
        })
    });
}

/// `root` holds `a/` (with `deep/d.txt` and `inner.txt`), `b.txt`, `x&y.txt`, `q&'.txt` and, when `hidden`, `.hidden`.
fn sample_tree(root: &Path, hidden: bool) {
    std::fs::create_dir_all(root.join("a").join("deep")).unwrap();
    std::fs::write(root.join("a").join("inner.txt"), b"xyz").unwrap();
    std::fs::write(root.join("a").join("deep").join("d.txt"), b"d").unwrap();
    std::fs::write(root.join("b.txt"), b"bb").unwrap();
    std::fs::write(root.join("x&y.txt"), b"yy").unwrap();
    std::fs::write(root.join("q&'.txt"), b"qq").unwrap();
    if hidden {
        std::fs::write(root.join(".hidden"), b"h").unwrap();
    }
}

fn index(root: &Path, format: IndexFormat, options: IndexOptions) -> String {
    let h = harness(Vec::new());
    block_on(index_folder(&h.ports, root, format, options)).unwrap()
}

#[test]
fn text_index_is_an_indented_tree_with_sizes() {
    on_pool(|| {
        let dir = TempDir::new().unwrap();
        sample_tree(dir.path(), true);
        let text = index(dir.path(), IndexFormat::Text, IndexOptions::default());
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].ends_with('/'), "{text}");
        assert_eq!(
            &lines[1..],
            [
                "  a/",
                "    deep/",
                "      d.txt (1 bytes)",
                "    inner.txt (3 bytes)",
                "  b.txt (2 bytes)",
                "  q&'.txt (2 bytes)",
                "  x&y.txt (2 bytes)",
            ]
        );

        let hidden = index(dir.path(), IndexFormat::Text, IndexOptions { include_hidden: true, ..IndexOptions::default() });
        assert!(hidden.contains("\n  .hidden (1 bytes)\n"), "{hidden}");

        let shallow = index(
            dir.path(),
            IndexFormat::Text,
            IndexOptions { max_depth: Some(1), include_files: false, show_sizes: false, ..IndexOptions::default() },
        );
        assert_eq!(shallow.lines().skip(1).collect::<Vec<_>>(), vec!["  a/"]);
    });
}

#[test]
fn json_index_is_a_nested_object() {
    on_pool(|| {
        let dir = TempDir::new().unwrap();
        sample_tree(dir.path(), false);
        let json: serde_json::Value = serde_json::from_str(&index(dir.path(), IndexFormat::Json, IndexOptions::default())).unwrap();
        assert_eq!(json["type"], "folder");
        assert_eq!(json["children"][0]["name"], "a");
        assert_eq!(json["children"][0]["children"][1]["name"], "inner.txt");
        assert_eq!(json["children"][0]["children"][1]["size"], 3);
        assert!(json["children"][0]["children"][0].get("size").is_none());
    });
}

#[test]
fn xml_index_escapes_names() {
    on_pool(|| {
        let dir = TempDir::new().unwrap();
        sample_tree(dir.path(), false);
        let xml = index(dir.path(), IndexFormat::Xml, IndexOptions::default());
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"), "{xml}");
        assert!(xml.contains("    <file name=\"inner.txt\" size=\"3\"/>\n"), "{xml}");
        assert!(xml.contains("  <file name=\"q&amp;&apos;.txt\" size=\"2\"/>\n"), "{xml}");
        assert!(xml.trim_end().ends_with("</index>"), "{xml}");
    });
}

#[test]
fn html_index_is_a_nested_list_with_a_title() {
    on_pool(|| {
        let dir = TempDir::new().unwrap();
        sample_tree(dir.path(), false);
        let title = dir.path().file_name().unwrap().to_string_lossy().into_owned();
        let page = index(dir.path(), IndexFormat::Html, IndexOptions::default());
        assert!(page.contains(&format!("<title>{title}</title>")), "{page}");
        assert!(page.contains("<li>a/"), "{page}");
        assert!(page.contains("<li>inner.txt (3 bytes)</li>"), "{page}");
        assert!(page.contains("<li>x&amp;y.txt (2 bytes)</li>"), "{page}");
    });
}

fn frame(width: u32, height: u32, pixel: [u8; 4]) -> Frame {
    Frame {
        monitor: Monitor {
            id: MonitorId("m".into()),
            name: "M".into(),
            rect: PhysRect { x: 0, y: 0, width, height },
            scale: 1.0,
            primary: true,
        },
        image: solid(width, height, pixel),
    }
}

#[test]
fn combine_joins_images_and_refuses_an_input_as_output() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let a = write(dir.path().join("a.png"), &png(4, 4, [255, 0, 0, 255]));
            let b = write(dir.path().join("b.png"), &png(2, 4, [0, 0, 255, 255]));
            let out = dir.path().join("strip.png");

            let written = combine_files(&h.ports, &[a.clone(), b.clone()], Orientation::Horizontal, Align::Start, 1, &out).await.unwrap();
            assert_eq!(written, out);
            let image = decode(&std::fs::read(&out).unwrap());
            assert_eq!((image.width(), image.height()), (7, 4));

            assert!(combine_files(&h.ports, &[a.clone(), b], Orientation::Vertical, Align::Center, 0, &a).await.is_err());
            assert!(combine_files(&h.ports, &[a], Orientation::Vertical, Align::Center, 0, &dir.path().join("out.xyz")).await.is_err());
        })
    });
}

#[test]
fn split_writes_numbered_tiles_in_the_input_format() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let src = write(dir.path().join("grid.png"), &png(4, 4, [9, 9, 9, 255]));
            let out_dir = dir.path().join("tiles");

            let tiles = split_file(&h.ports, &src, 2, 2, &out_dir).await.unwrap();
            let names: Vec<String> = tiles.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
            assert_eq!(names, vec!["grid_001.png", "grid_002.png", "grid_003.png", "grid_004.png"]);
            for tile in &tiles {
                let image = decode(&std::fs::read(tile).unwrap());
                assert_eq!((image.width(), image.height()), (2, 2));
            }
        })
    });
}

#[test]
fn resize_and_thumbnail_scale_into_named_outputs() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let src = write(dir.path().join("wide.png"), &png(8, 4, [1, 2, 3, 255]));

            let exact = resize_files(&h.ports, std::slice::from_ref(&src), 4, 4, false, dir.path()).await.unwrap();
            assert!(exact[0].ends_with("wide_resized.png"));
            let image = decode(&std::fs::read(&exact[0]).unwrap());
            assert_eq!((image.width(), image.height()), (4, 4));

            let fitted = resize_files(&h.ports, std::slice::from_ref(&src), 4, 100, true, dir.path()).await.unwrap();
            let image = decode(&std::fs::read(&fitted[0]).unwrap());
            assert_eq!((image.width(), image.height()), (4, 2));

            let thumbs = thumbnail_files(&h.ports, std::slice::from_ref(&src), 4, 4, dir.path()).await.unwrap();
            assert!(thumbs[0].ends_with("wide_thumb.png"));
            let image = decode(&std::fs::read(&thumbs[0]).unwrap());
            assert!(image.width() <= 4 && image.height() <= 4);
        })
    });
}

#[test]
fn convert_writes_new_files_and_never_overwrites_the_input() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let src = write(dir.path().join("pic.png"), &png(3, 3, [200, 100, 50, 255]));

            let jpeg = convert_files(&h.ports, std::slice::from_ref(&src), ImageFormat::Jpeg, 80, dir.path()).await.unwrap();
            assert!(jpeg[0].ends_with("pic.jpg"));
            assert_eq!(sniff(&std::fs::read(&jpeg[0]).unwrap()), Some(ImageFormat::Jpeg));

            let same = convert_files(&h.ports, std::slice::from_ref(&src), ImageFormat::Png, 90, dir.path()).await.unwrap();
            assert!(same[0].ends_with("pic_2.png"), "{:?}", same[0]);
            assert_eq!(sniff(&std::fs::read(&src).unwrap()), Some(ImageFormat::Png));
        })
    });
}

#[test]
fn watermark_draws_the_mark_over_each_image() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let base = write(dir.path().join("base.png"), &png(4, 4, [0, 0, 0, 255]));
            let mark = write(dir.path().join("mark.png"), &png(2, 2, [255, 255, 255, 255]));

            let out = watermark_files(&h.ports, std::slice::from_ref(&base), &mark, Placement::TopLeft, 1.0, dir.path()).await.unwrap();
            assert!(out[0].ends_with("base_watermark.png"));
            let image = decode(&std::fs::read(&out[0]).unwrap());
            assert_eq!((image.width(), image.height()), (4, 4));
            assert_eq!(&image.pixels()[0..4], &[255, 255, 255, 255]);
            assert_eq!(&image.pixels()[(3 * 4 + 3) * 4..(3 * 4 + 3) * 4 + 4], &[0, 0, 0, 255]);
        })
    });
}

#[test]
fn compare_counts_changed_pixels() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let a = write(dir.path().join("a.png"), &png(4, 4, [255, 0, 0, 255]));
            let same = write(dir.path().join("same.png"), &png(4, 4, [255, 0, 0, 255]));
            let diff = compare_files(&h.ports, &a, &same, 0).await.unwrap();
            assert_eq!((diff.changed, diff.total, diff.max_delta), (0, 16, 0));
            assert!(diff.psnr.is_infinite());

            let mut pixels = solid(4, 4, [255, 0, 0, 255]).pixels().to_vec();
            pixels[0..4].copy_from_slice(&[0, 0, 0, 255]);
            let edited = Bitmap::new(4, 4, PixelFormat::Rgba8, pixels.into()).unwrap();
            let edited_png = write(dir.path().join("edited.png"), &encode_png(&edited).unwrap());
            let diff = compare_files(&h.ports, &a, &edited_png, 10).await.unwrap();
            assert_eq!((diff.changed, diff.max_delta), (1, 255));
        })
    });
}

#[test]
fn icon_writes_one_entry_per_size() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let src = write(dir.path().join("logo.png"), &png(8, 8, [0, 128, 0, 255]));
            let out = dir.path().join("logo.ico");

            make_icon(&h.ports, &src, &[16, 32], &out).await.unwrap();
            let sizes: Vec<(u32, u32)> =
                decode_ico(&std::fs::read(&out).unwrap()).unwrap().iter().map(|i| (i.width(), i.height())).collect();
            assert_eq!(sizes, vec![(16, 16), (32, 32)]);
            assert!(make_icon(&h.ports, &src, &[16], &src).await.is_err());
        })
    });
}

/// A PNG with a `tEXt` comment, which metadata stripping removes.
fn png_with_comment(comment: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.add_text_chunk("Comment".to_owned(), comment.to_owned()).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0u8; 16]).unwrap();
        writer.finish().unwrap();
    }
    bytes
}

#[test]
fn strip_removes_text_chunks_from_a_png() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let src = write(dir.path().join("photo.png"), &png_with_comment("secret location"));
            assert!(std::fs::read(&src).unwrap().windows(4).any(|w| w == b"tEXt"), "the fixture has a text chunk");

            let out = strip_metadata_files(&h.ports, std::slice::from_ref(&src), dir.path()).await.unwrap();
            assert!(out[0].ends_with("photo_stripped.png"));
            let stripped = std::fs::read(&out[0]).unwrap();
            assert!(!stripped.windows(b"secret".len()).any(|w| w == b"secret"));
            assert!(!stripped.windows(4).any(|w| w == b"tEXt"));
            assert_eq!(sniff(&stripped), Some(ImageFormat::Png));
        })
    });
}

#[test]
fn metadata_of_an_image_is_its_exif_and_of_other_files_is_probed() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let picture = write(dir.path().join("p.png"), &png(2, 2, [0, 0, 0, 255]));
            assert!(read_metadata(&h.ports, &picture).await.unwrap().is_empty());
            assert!(h.media.probes.lock().unwrap().is_empty());

            let clip = write(dir.path().join("clip.mp4"), b"not really a movie");
            let rows = read_metadata(&h.ports, &clip).await.unwrap();
            assert!(rows.contains(&("Duration".to_owned(), "3.00 s".to_owned())), "{rows:?}");
            assert!(rows.contains(&("Dimensions".to_owned(), "640 x 480".to_owned())), "{rows:?}");
            assert!(rows.contains(&("Video codec".to_owned(), "h264".to_owned())), "{rows:?}");
            assert!(!rows.iter().any(|(name, _)| name == "Audio codec"), "{rows:?}");
            assert_eq!(h.media.probes.lock().unwrap().as_slice(), std::slice::from_ref(&clip));
        })
    });
}

#[test]
fn video_wrappers_send_the_specs_they_choose() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let input = dir.path().join("in.mkv");
            let output = dir.path().join("nested").join("out.mp4");

            let spec = TranscodeSpec {
                codec: Some(VideoCodec::X265),
                quality: Quality::Crf(23),
                fps: Some(30),
                width: Some(1280),
                ..Default::default()
            };
            convert_video(&h.ports, &input, &output, spec.clone()).await.unwrap();
            assert!(dir.path().join("nested").is_dir(), "the output folder is created");

            trim_video(&h.ports, &input, &output, 1.5, 2.0).await.unwrap();
            let gif = dir.path().join("clip.gif");
            make_gif(&h.ports, &input, &gif, 10, 320).await.unwrap();
            make_gif(&h.ports, &input, &dir.path().join("full.gif"), 12, 0).await.unwrap();

            let calls = h.media.transcodes.lock().unwrap().clone();
            assert_eq!(calls.len(), 4);
            assert_eq!(calls[0].2, spec);
            assert_eq!(calls[1].2, TranscodeSpec { start_secs: Some(1.5), duration_secs: Some(2.0), ..Default::default() });
            assert_eq!(calls[2].2, TranscodeSpec { codec: Some(VideoCodec::Gif), fps: Some(10), width: Some(320), ..Default::default() });
            assert_eq!(calls[3].2, TranscodeSpec { codec: Some(VideoCodec::Gif), fps: Some(12), width: None, ..Default::default() });
        })
    });
}

#[test]
fn video_thumbnails_take_evenly_spaced_frames() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let dir = TempDir::new().unwrap();
            let input = dir.path().join("clip.webm");
            let out_dir = dir.path().join("frames");

            assert!(video_thumbnails(&h.ports, &input, &out_dir, 0).await.unwrap().is_empty());
            assert!(h.media.probes.lock().unwrap().is_empty());

            let frames = video_thumbnails(&h.ports, &input, &out_dir, 3).await.unwrap();
            let names: Vec<String> = frames.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
            assert_eq!(names, vec!["clip_frame001.png", "clip_frame002.png", "clip_frame003.png"]);
            let calls = h.media.transcodes.lock().unwrap().clone();
            let starts: Vec<f64> = calls.iter().map(|(_, _, spec)| spec.start_secs.unwrap()).collect();
            assert_eq!(starts, vec![0.75, 1.5, 2.25]);
            assert!(calls.iter().all(|(_, _, spec)| spec.single_frame));
        })
    });
}

#[test]
fn video_thumbnails_need_a_duration() {
    on_pool(|| {
        block_on(async {
            let mut h = harness(Vec::new());
            h.ports.media = Arc::new(SpyMedia { duration: 0.0, ..SpyMedia::default() });
            let dir = TempDir::new().unwrap();
            assert!(video_thumbnails(&h.ports, &dir.path().join("still.png"), dir.path(), 2).await.is_err());
        })
    });
}

#[test]
fn ocr_returns_the_recognised_lines() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            h.ocr.result.lock().unwrap().lines =
                vec![OcrLine { text: "Hello".into(), ..Default::default() }, OcrLine { text: "world".into(), ..Default::default() }];
            let result = ocr_image(&h.ports, solid(2, 2, [255, 255, 255, 255]), "en").await.unwrap();
            assert_eq!(result.text(), "Hello\nworld");
        })
    });
}

#[test]
fn qr_codes_round_trip_through_images_files_and_the_screen() {
    on_pool(|| {
        block_on(async {
            let text = "https://example.com/qr";
            let code = qr_image(text, 4).unwrap();
            let h = harness(vec![frame(code.width(), code.height(), [255, 255, 255, 255])]);
            assert_eq!(qr_decode_image(&h.ports, code.clone()).await.unwrap(), vec![text.to_owned()]);

            let dir = TempDir::new().unwrap();
            let path = write(dir.path().join("code.png"), &encode_png(&code).unwrap());
            assert_eq!(qr_decode_file(&h.ports, &path).await.unwrap(), vec![text.to_owned()]);

            let blank = write(dir.path().join("blank.png"), &png(16, 16, [255, 255, 255, 255]));
            assert!(qr_decode_file(&h.ports, &blank).await.unwrap().is_empty());

            let on_screen = harness(vec![Frame {
                monitor: Monitor {
                    id: MonitorId("m".into()),
                    name: "M".into(),
                    rect: PhysRect { x: 0, y: 0, width: code.width(), height: code.height() },
                    scale: 1.0,
                    primary: true,
                },
                image: code,
            }]);
            assert_eq!(qr_decode_screen(&on_screen.ports).await.unwrap(), vec![text.to_owned()]);
        })
    });
}

fn window(handle: u64, title: &str) -> HoverTarget {
    HoverTarget {
        rect: PhysRect { x: 0, y: 0, width: 100, height: 100 },
        handle,
        title: title.to_owned(),
        process: "notes.exe".to_owned(),
        kind: TargetKind::Window,
    }
}

#[test]
fn window_toggles_flip_and_remember_their_state() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            *h.hover.active.lock().unwrap() = Some(window(7, "Notes"));
            let toggles = WindowToggles::default();

            assert!(toggle_borderless_active(&h.ports, &toggles).await.unwrap());
            assert!(!toggle_borderless_active(&h.ports, &toggles).await.unwrap());
            assert!(toggle_topmost_active(&h.ports, &toggles).await.unwrap());

            let calls = h.window_ops.calls.lock().unwrap().clone();
            assert_eq!(calls, vec!["borderless Notes true", "borderless Notes false", "topmost Notes true"]);

            // A window without a handle is tracked by its title and process.
            *h.hover.active.lock().unwrap() = Some(window(0, "Scratch"));
            assert!(toggle_borderless_active(&h.ports, &toggles).await.unwrap());
            assert!(!toggle_borderless_active(&h.ports, &toggles).await.unwrap());
        })
    });
}

#[test]
fn window_tools_need_an_active_window() {
    on_pool(|| {
        block_on(async {
            let h = harness(Vec::new());
            let toggles = WindowToggles::default();
            assert!(toggle_borderless_active(&h.ports, &toggles).await.is_err());
            assert!(toggle_topmost_active(&h.ports, &toggles).await.is_err());
            assert!(inspect_active(&h.ports).await.is_err());
            assert!(h.window_ops.calls.lock().unwrap().is_empty());

            *h.hover.active.lock().unwrap() = Some(window(3, "Editor"));
            assert_eq!(inspect_active(&h.ports).await.unwrap(), vec![("Title".to_owned(), "Editor".to_owned())]);
        })
    });
}
