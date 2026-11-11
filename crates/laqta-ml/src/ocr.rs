//! The `Ocr` port backed by PP-OCR models (family O1). Inference runs on a plain thread, and the
//! result comes back over a oneshot channel. The caller's executor never blocks on a model.

use std::path::Path;
use std::sync::Arc;

use futures_channel::oneshot;
use laqta_domain::{Bitmap, PhysRect};
use laqta_ports::{BoxFuture, Ocr, OcrLine, OcrResult, OcrWord, PortError};
use ndarray::Array4;
use ort::inputs;
use ort::session::Session;
use ort::value::TensorRef;

use crate::catalog::{Task, catalog};
use crate::error::MlError;
use crate::families::o1;
use crate::runtime::{Runtime, ort_error};
use crate::tensor::BgrImage;

/// OCR with a detection model and a recognition model from the catalog.
#[derive(Debug)]
pub struct ModelOcr {
    runtime: Arc<Runtime>,
    det: String,
    rec: String,
}

impl ModelOcr {
    /// `det` and `rec` are catalog ids. `rec` is the default recogniser for the empty language.
    pub fn new(runtime: Arc<Runtime>, det: impl Into<String>, rec: impl Into<String>) -> Self {
        ModelOcr { runtime, det: det.into(), rec: rec.into() }
    }

    /// Checks that both models are installed and picks the recogniser for `language`. The empty
    /// language uses the default recogniser. Any other tag or script name picks the first installed
    /// recogniser that lists it.
    fn prepare(&self, language: &str) -> Result<(String, String), MlError> {
        let det = self.installed(&self.det)?;
        let rec = if language.is_empty() {
            self.installed(&self.rec)?
        } else {
            let wanted = language.to_ascii_lowercase();
            catalog()
                .models_for(Task::OcrRec)
                .find(|model| model.languages.iter().any(|lang| lang.eq_ignore_ascii_case(&wanted)) && self.runtime.is_installed(model))
                .map(|model| model.id.clone())
                .ok_or_else(|| MlError::NoModelForLanguage(language.to_owned()))?
        };
        Ok((det, rec))
    }

    fn installed(&self, id: &str) -> Result<String, MlError> {
        let entry = catalog().model(id).ok_or_else(|| MlError::UnknownModel(id.to_owned()))?;
        if self.runtime.is_installed(entry) { Ok(id.to_owned()) } else { Err(MlError::NotInstalled(id.to_owned())) }
    }
}

impl Ocr for ModelOcr {
    fn recognize(&self, image: Bitmap, language: String) -> BoxFuture<'_, Result<OcrResult, PortError>> {
        let (det, rec) = match self.prepare(&language) {
            Ok(models) => models,
            Err(error) => return Box::pin(async move { Err(PortError::from(error)) }),
        };
        let (sender, receiver) = oneshot::channel();
        let runtime = self.runtime.clone();
        let spawned = std::thread::Builder::new().name("laqta-ocr".into()).spawn(move || {
            // The receiver is gone only if the caller stopped waiting, so the result is not needed.
            // A run cannot be stopped midway, but one nobody waits for ends before the next model
            // run instead of keeping the next read waiting on the session.
            let result = recognise(&runtime, &det, &rec, &image, &|| sender.is_canceled());
            let _ = sender.send(result);
        });
        if let Err(error) = spawned {
            return Box::pin(async move { Err(PortError::Failed(format!("cannot start the OCR thread: {error}"))) });
        }
        Box::pin(async move {
            match receiver.await {
                Ok(result) => result.map_err(PortError::from),
                Err(_) => Err(PortError::Failed("the OCR thread stopped without a result".into())),
            }
        })
    }

    fn languages(&self) -> Vec<String> {
        let mut languages: Vec<String> = catalog()
            .models_for(Task::OcrRec)
            .filter(|model| self.runtime.is_installed(model))
            .flat_map(|model| model.languages.iter().cloned())
            .collect();
        languages.sort();
        languages.dedup();
        languages
    }
}

/// Runs detection, then recognition on each region, on the calling thread. It gives up with
/// `Cancelled` before any model run once `gone` says nobody wants the answer.
fn recognise(runtime: &Runtime, det: &str, rec: &str, bitmap: &Bitmap, gone: &dyn Fn() -> bool) -> Result<OcrResult, MlError> {
    if gone() {
        return Err(MlError::Cancelled);
    }
    if bitmap.width() == 0 || bitmap.height() == 0 {
        return Ok(OcrResult::default());
    }
    let image = BgrImage::from_bitmap(bitmap);
    // Each detector has its own DB thresholds; a model without any uses PP-OCRv5's.
    let params = catalog().model(det).and_then(|entry| entry.db).unwrap_or_default();
    let regions = detect(runtime, det, &params, &image, gone)?;
    if regions.is_empty() {
        return Ok(OcrResult::default());
    }
    let dictionary = read_dictionary(&runtime.dict_path(rec))?;
    // Crops in reading order. They are read in batches of similar width, as PaddleOCR reads them,
    // so a batch pads little; the lines are put back in reading order afterwards.
    let crops: Vec<(PhysRect, u32, Vec<f32>)> = o1::reading_order(&regions)
        .into_iter()
        .filter_map(|index| o1::rec_crop(&image, regions[index]).map(|(scaled, data)| (regions[index], scaled, data)))
        .collect();
    let mut by_width: Vec<usize> = (0..crops.len()).collect();
    by_width.sort_by_key(|&i| crops[i].1);
    let mut read: Vec<Option<(String, f32, Vec<OcrWord>)>> = vec![None; crops.len()];
    let mut table: Option<Vec<String>> = None;
    runtime.with_session(Task::OcrRec, rec, |session| {
        for batch in by_width.chunks(o1::REC_BATCH) {
            if gone() {
                return Err(MlError::Cancelled);
            }
            let widest = batch.iter().map(|&i| crops[i].1).max().unwrap_or(0);
            let width = o1::rec_input_width(widest);
            let mut data = Vec::with_capacity(batch.len() * 3 * o1::REC_HEIGHT as usize * width as usize);
            for &i in batch {
                data.extend(o1::rec_pad(crops[i].1, &crops[i].2, width));
            }
            let input = Array4::from_shape_vec((batch.len(), 3, o1::REC_HEIGHT as usize, width as usize), data)
                .map_err(|error| MlError::Inference(error.to_string()))?;
            let (shape, scores) = run(session, &input)?;
            if shape.len() != 3 || shape[0] != batch.len() {
                return Err(MlError::Inference(format!("the recogniser output has shape {shape:?} for a batch of {}", batch.len())));
            }
            let (steps, classes) = (shape[1], shape[2]);
            if table.is_none() {
                table = Some(o1::ctc_classes(&dictionary, classes).map_err(MlError::Inference)?);
            }
            let Some(classes_table) = table.as_deref() else { continue };
            for (k, &i) in batch.iter().enumerate() {
                let rows = &scores[k * steps * classes..(k + 1) * steps * classes];
                let best = o1::best_per_step(rows, classes);
                let (text, confidence) = o1::ctc_greedy(&best, classes_table);
                // The time steps run along the padded input, so each character's step tells where
                // in the line it sits: that gives every word a box (ADR 031). A line read on its
                // side has none.
                let (rect, scaled, _) = &crops[i];
                let words = if o1::rec_turns(*rect) {
                    Vec::new()
                } else {
                    o1::ctc_words(&o1::ctc_positions(&best, classes_table), *scaled as f32 / width as f32, *rect)
                        .into_iter()
                        .map(|(rect, text)| OcrWord { rect, text })
                        .collect()
                };
                read[i] = Some((text, confidence, words));
            }
        }
        Ok(())
    })?;
    let lines = crops
        .iter()
        .zip(read)
        .filter_map(|((rect, _, _), read)| {
            let (text, confidence, words) = read?;
            (!text.trim().is_empty()).then_some(OcrLine { rect: *rect, text, confidence, words })
        })
        .collect();
    Ok(OcrResult { lines })
}

/// Text regions in source pixels, from the detector.
fn detect(
    runtime: &Runtime,
    det: &str,
    params: &o1::DbParams,
    image: &BgrImage,
    gone: &dyn Fn() -> bool,
) -> Result<Vec<PhysRect>, MlError> {
    let (width, height, data) = o1::det_input(image);
    let input =
        Array4::from_shape_vec((1, 3, height as usize, width as usize), data).map_err(|error| MlError::Inference(error.to_string()))?;
    // The check is inside the session, so a read that waited for it behind another one can still give up.
    let (shape, map) =
        runtime.with_session(Task::OcrDet, det, |session| if gone() { Err(MlError::Cancelled) } else { run(session, &input) })?;
    let (map_height, map_width) = map_dims(&shape, map.len())?;
    let boxes = o1::db_boxes(&map, map_width, map_height, params);
    Ok(boxes.into_iter().map(|boxed| o1::to_source_rect(boxed, map_width, map_height, image.width(), image.height())).collect())
}

/// Runs one input and copies the output out, so the session can be used again.
fn run(session: &mut Session, input: &Array4<f32>) -> Result<(Vec<usize>, Vec<f32>), MlError> {
    let outputs = session.run(inputs![TensorRef::from_array_view(input.view()).map_err(ort_error)?]).map_err(ort_error)?;
    let output = outputs[0].try_extract_array::<f32>().map_err(ort_error)?;
    Ok((output.shape().to_vec(), output.iter().copied().collect()))
}

/// The height and width of a detector output, which is `[1, 1, H, W]` or `[H, W]`.
fn map_dims(shape: &[usize], len: usize) -> Result<(usize, usize), MlError> {
    let rank = shape.len();
    if rank < 2 {
        return Err(MlError::Inference(format!("the detector output has rank {rank}")));
    }
    let (height, width) = (shape[rank - 2], shape[rank - 1]);
    if height * width != len || len == 0 {
        return Err(MlError::Inference(format!("the detector output {shape:?} does not match its data length {len}")));
    }
    Ok((height, width))
}

fn read_dictionary(path: &Path) -> Result<Vec<String>, MlError> {
    let text = std::fs::read_to_string(path).map_err(|error| MlError::Io(format!("{}: {error}", path.display())))?;
    Ok(o1::parse_dictionary(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_dims_reads_the_last_two_axes() {
        assert_eq!(map_dims(&[1, 1, 4, 6], 24).expect("valid"), (4, 6));
        assert_eq!(map_dims(&[4, 6], 24).expect("valid"), (4, 6));
        assert!(map_dims(&[1, 1, 4, 6], 23).is_err());
        assert!(map_dims(&[6], 6).is_err());
    }

    #[test]
    fn languages_are_empty_with_no_models_installed() {
        let runtime = Arc::new(Runtime::new("/nonexistent-lib.so", "/nonexistent-models", crate::ExecutionProvider::Cpu));
        let ocr = ModelOcr::new(runtime, "ppocr-v5-mobile-det", "ppocr-v5-mobile-rec-multi");
        assert!(ocr.languages().is_empty());
    }

    #[test]
    fn a_read_nobody_waits_for_stops_before_running_a_model() {
        let runtime = Runtime::new("/nonexistent-lib.so", "/nonexistent-models", crate::ExecutionProvider::Cpu);
        let bitmap = Bitmap::new(2, 2, laqta_domain::PixelFormat::Bgra8, vec![0; 16].into()).expect("valid bitmap");
        let gone = || true;
        assert!(matches!(recognise(&runtime, "ppocr-v5-mobile-det", "ppocr-v5-mobile-rec-multi", &bitmap, &gone), Err(MlError::Cancelled)));
    }

    #[test]
    fn prepare_reports_a_missing_detector() {
        let runtime = Arc::new(Runtime::new("/nonexistent-lib.so", "/nonexistent-models", crate::ExecutionProvider::Cpu));
        let ocr = ModelOcr::new(runtime, "ppocr-v5-mobile-det", "ppocr-v5-mobile-rec-multi");
        assert!(matches!(ocr.prepare(""), Err(MlError::NotInstalled(id)) if id == "ppocr-v5-mobile-det"));
    }

    #[test]
    fn prepare_with_a_detector_but_no_recogniser_names_the_missing_model() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("ppocr-v5-mobile-det.onnx"), b"not a model").expect("write placeholder");
        let runtime = Arc::new(Runtime::new("/nonexistent-lib.so", dir.path(), crate::ExecutionProvider::Cpu));
        let ocr = ModelOcr::new(runtime, "ppocr-v5-mobile-det", "ppocr-v5-mobile-rec-multi");
        assert!(matches!(ocr.prepare(""), Err(MlError::NotInstalled(id)) if id == "ppocr-v5-mobile-rec-multi"));
        assert!(matches!(ocr.prepare("klingon"), Err(MlError::NoModelForLanguage(language)) if language == "klingon"));
    }
}
