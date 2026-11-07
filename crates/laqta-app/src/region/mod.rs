//! The Region session (ADR 012). It shows the overlay over the frozen frames, loads hover targets in
//! the background, runs the `domain::region` state machine, and crops the confirmed rects out of the
//! composed frames. Tests script the input (`tests.rs`).

#[cfg(test)]
mod tests;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures_channel::{mpsc, oneshot};
use futures_lite::{StreamExt, future};
use laqta_domain::capture::RegionCaptureOptions;
use laqta_domain::region::{HoverTarget, RegionConfig, RegionInput, RegionOutcome, RegionState};
use laqta_domain::{Bitmap, Frame, PhysRect, PixelFormat};
use laqta_imaging::compose_monitors;
use laqta_ports::{AppError, InputCallback, RegionView, Spawner};

use crate::dispatcher::Ports;

/// What a Region session returns. `rects` are desktop pixels. `image` covers the bounding box of
/// `rects`; with several rects, pixels outside every rect are transparent.
#[derive(Clone, Debug)]
pub struct Selection {
    pub rects: Vec<PhysRect>,
    pub target: Option<HoverTarget>,
    pub image: Bitmap,
}

/// Shows the overlay over `frames` and runs the selection until the user confirms (`Ok`) or cancels
/// (`Err(AppError::Cancelled)`). Hover targets load after the overlay is up, and a failure there
/// only means the user draws freely.
#[cfg_attr(not(test), allow(dead_code))] // No caller until the dispatcher routes region actions.
pub(crate) async fn select(ports: &Ports, frames: Vec<Frame>, options: &RegionCaptureOptions) -> Result<Selection, AppError> {
    let bounds = bounds_of(&frames)?;
    let composed = compose(ports.spawner.as_ref(), frames.clone()).await?;
    let mut state = RegionState::new(RegionConfig {
        bounds,
        multi_region: options.multi_region,
        snap_to_targets: options.snap_to_windows,
        min_size: options.min_size,
        magnifier_pixels: options.magnifier_pixels,
        info_text: options.info_text.clone(),
    });

    let (tx, mut inputs) = mpsc::unbounded::<RegionInput>();
    let hover_tx = tx.clone();
    let on_input: InputCallback = Arc::new(move |input| {
        // The session may have ended already. A late input has nowhere to go.
        let _ = tx.unbounded_send(input);
    });
    let first = view(&state, bounds, options, pixel_at(&composed, bounds, state.cursor()));
    ports.overlay.show(frames, first, on_input)?;

    let loaded = async {
        if let Ok(targets) = ports.hover.targets().await {
            let _ = hover_tx.unbounded_send(RegionInput::TargetsLoaded(targets));
        }
        // Drop this sender once the targets are in, so the input channel can end.
        drop(hover_tx);
        future::pending::<Result<Selection, AppError>>().await
    };

    let session = async {
        while let Some(input) = inputs.next().await {
            let changed = state.handle(input);
            if let Some(outcome) = state.outcome().cloned() {
                ports.overlay.close();
                return outcome_selection(outcome, &composed, bounds);
            }
            if changed {
                let pixel = pixel_at(&composed, bounds, state.cursor());
                ports.overlay.update(view(&state, bounds, options, pixel));
            }
        }
        // Every sender is gone, so no input can arrive.
        ports.overlay.close();
        Err(AppError::Cancelled)
    };

    future::or(loaded, session).await
}

/// Crops `rects` (desktop pixels) out of the composed `frames`, without showing an overlay. The
/// dispatcher uses it for custom, last and active-window captures.
#[cfg_attr(not(test), allow(dead_code))] // No caller until the dispatcher routes region actions.
pub(crate) async fn crop_frames(
    ports: &Ports,
    frames: Vec<Frame>,
    rects: Vec<PhysRect>,
    target: Option<HoverTarget>,
) -> Result<Selection, AppError> {
    let bounds = bounds_of(&frames)?;
    let composed = compose(ports.spawner.as_ref(), frames).await?;
    let image = crop_rects(&composed, bounds, &rects)?;
    Ok(Selection { rects, target, image })
}

/// The overlay's view of `state`. `pixel` is the RGB under the cursor, and `bounds` is the desktop
/// area the state was built with. `RegionState` does not expose it.
#[cfg_attr(not(test), allow(dead_code))] // Used only by `select`.
pub(crate) fn view(state: &RegionState, bounds: PhysRect, options: &RegionCaptureOptions, pixel: [u8; 3]) -> RegionView {
    let regions = state.regions().to_vec();
    let active = state.active_region().and_then(|rect| regions.iter().position(|r| *r == rect));
    RegionView {
        bounds,
        regions,
        active,
        drawing: state.drawing_rect(),
        hovered: state.hovered_target().cloned(),
        handles: state.handle_rects(),
        cursor: state.cursor(),
        magnifier: options.show_magnifier.then(|| state.magnifier_rect()),
        info_text: options.show_info.then(|| state.info_text(pixel)),
        show_crosshair: options.show_crosshair,
        dim_alpha: options.dim_alpha,
    }
}

fn outcome_selection(outcome: RegionOutcome, composed: &Bitmap, bounds: PhysRect) -> Result<Selection, AppError> {
    match outcome {
        RegionOutcome::Confirmed { rects, target } => {
            let image = crop_rects(composed, bounds, &rects)?;
            Ok(Selection { rects, target, image })
        }
        RegionOutcome::Cancelled => Err(AppError::Cancelled),
    }
}

/// The union of the monitor rects, each sized to its image. This matches where `compose_monitors`
/// puts the frames.
fn bounds_of(frames: &[Frame]) -> Result<PhysRect, AppError> {
    frames
        .iter()
        .map(|f| PhysRect { x: f.monitor.rect.x, y: f.monitor.rect.y, width: f.image.width(), height: f.image.height() })
        .reduce(PhysRect::union)
        .ok_or_else(|| AppError::Io("no frames to select from".to_owned()))
}

/// Composes the frames on the compute pool. A panic there comes back as a value (ADR 020).
async fn compose(spawner: &dyn Spawner, frames: Vec<Frame>) -> Result<Bitmap, AppError> {
    let (tx, rx) = oneshot::channel::<Result<Bitmap, String>>();
    spawner
        .run_compute(Box::new(move || {
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| compose_monitors(&frames)));
            let _ = tx.send(outcome.map_err(|payload| panic_message(&*payload)));
        }))
        .await;
    match rx.await {
        Ok(Ok(bitmap)) => Ok(bitmap),
        Ok(Err(message)) => Err(AppError::Panic(message)),
        Err(oneshot::Canceled) => Err(AppError::Cancelled),
    }
}

/// The RGB of the composed pixel under the desktop point `at`, or black outside `bounds`.
/// `compose_monitors` always gives RGBA8.
fn pixel_at(composed: &Bitmap, bounds: PhysRect, at: (i32, i32)) -> [u8; 3] {
    let x = i64::from(at.0) - i64::from(bounds.x);
    let y = i64::from(at.1) - i64::from(bounds.y);
    let (width, height) = (i64::from(composed.width()), i64::from(composed.height()));
    if !(0..width).contains(&x) || !(0..height).contains(&y) {
        return [0; 3];
    }
    let i = (y * width + x) as usize * 4;
    composed.pixels().get(i..i + 3).and_then(|rgb| <[u8; 3]>::try_from(rgb).ok()).unwrap_or([0; 3])
}

/// Crops `rects` (desktop pixels) from the composed image. One rect gives its own crop. Several
/// rects give their bounding box, with the pixels outside every rect cleared to transparent.
fn crop_rects(composed: &Bitmap, bounds: PhysRect, rects: &[PhysRect]) -> Result<Bitmap, AppError> {
    let Some(boxed) = rects.iter().copied().reduce(PhysRect::union) else {
        return bitmap(0, 0, composed.format(), Vec::new());
    };
    let in_composed = PhysRect {
        x: boxed.x.saturating_sub(bounds.x),
        y: boxed.y.saturating_sub(bounds.y),
        ..boxed
    };
    let cropped = laqta_imaging::crop(composed, in_composed);
    if rects.len() == 1 {
        return Ok(cropped);
    }

    // `crop` clamps the origin into the composed image. Use the same origin to map each pixel back
    // to the desktop.
    let x0 = i64::from(in_composed.x).clamp(0, i64::from(composed.width()));
    let y0 = i64::from(in_composed.y).clamp(0, i64::from(composed.height()));
    let row_len = cropped.width() as usize * 4;
    let mut pixels = cropped.pixels().to_vec();
    for (row, line) in pixels.chunks_exact_mut(row_len).enumerate() {
        let y = i64::from(bounds.y) + y0 + row as i64;
        for (col, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = i64::from(bounds.x) + x0 + col as i64;
            if !rects.iter().any(|rect| contains(*rect, x, y)) {
                pixel.fill(0);
            }
        }
    }
    bitmap(cropped.width(), cropped.height(), cropped.format(), pixels)
}

fn contains(rect: PhysRect, x: i64, y: i64) -> bool {
    i64::from(rect.x) <= x && x < i64::from(rect.right()) && i64::from(rect.y) <= y && y < i64::from(rect.bottom())
}

fn bitmap(width: u32, height: u32, format: PixelFormat, pixels: Vec<u8>) -> Result<Bitmap, AppError> {
    Bitmap::new(width, height, format, pixels.into()).map_err(|error| AppError::Io(error.to_string()))
}

/// Same as the Task module's copy: a panic payload as text.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic".to_owned()
    }
}
