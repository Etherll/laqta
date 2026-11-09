//! The Region session (ADR 012). It shows the overlay over the frozen frames, loads hover targets in
//! the background, runs the `domain::region` state machine, and crops the confirmed rects out of the
//! composed frames. A session that keeps its region selected (`show_tools_after_select`) also crops the
//! selection for the overlay to draw on, and returns the overlay's annotated crop in place of its own.
//! Tests script the input (`tests.rs`).

mod layout;
#[cfg(test)]
mod tests;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures_channel::{mpsc, oneshot};
use futures_lite::{StreamExt, future};
use laqta_domain::capture::RegionCaptureOptions;
use laqta_domain::region::{HoverTarget, RegionConfig, RegionInput, RegionOutcome, RegionState, SelectionAction};
use laqta_domain::{Bitmap, Frame, PhysRect, PixelFormat};
use laqta_imaging::compose_monitors;
use laqta_ports::{AppError, InputCallback, RegionView, Spawner};

use crate::dispatcher::Ports;
use crate::task::compute;

/// What a Region session returns. `rects` are desktop pixels. `image` covers the bounding box of
/// `rects`; with several rects, pixels outside every rect are transparent. When the overlay sent an
/// annotated crop of the selection, `image` is that crop.
#[derive(Clone, Debug)]
pub struct Selection {
    pub rects: Vec<PhysRect>,
    pub target: Option<HoverTarget>,
    pub image: Bitmap,
    /// Confirmed with the overlay's Annotate button: the capture opens in the editor first.
    pub annotate: bool,
    /// What the user chose to do with the capture; `Default` leaves it to the after-capture steps.
    pub action: SelectionAction,
}

/// Picks an area for a tool that acts on it straight away (recording, scrolling capture, OCR,
/// QR): the selection confirms on release, with no capture toolbar.
pub(crate) async fn pick(ports: &Ports, frames: Vec<Frame>, options: &RegionCaptureOptions) -> Result<Selection, AppError> {
    let options = RegionCaptureOptions { show_tools_after_select: false, show_mode_bar: false, ..options.clone() };
    select(ports, frames, &options).await
}

/// Shows the overlay over `frames` and runs the selection until the user confirms (`Ok`) or cancels
/// (`Err(AppError::Cancelled)`). Hover targets load after the overlay is up, and a failure there
/// only means the user draws freely.
pub(crate) async fn select(ports: &Ports, frames: Vec<Frame>, options: &RegionCaptureOptions) -> Result<Selection, AppError> {
    session(ports, frames, options, false).await
}

/// Picks one pixel for the screen colour picker: a click or Enter confirms the 1×1 rect under the
/// cursor. The screen is not dimmed (it would change the colours the user sees), and the
/// magnifier and pixel info are always on.
pub(crate) async fn pick_color(ports: &Ports, frames: Vec<Frame>, options: &RegionCaptureOptions) -> Result<Selection, AppError> {
    let options = RegionCaptureOptions {
        show_tools_after_select: false,
        show_mode_bar: false,
        show_magnifier: true,
        show_info: true,
        dim_alpha: 0,
        ..options.clone()
    };
    session(ports, frames, &options, true).await
}

async fn session(ports: &Ports, frames: Vec<Frame>, options: &RegionCaptureOptions, pick_point: bool) -> Result<Selection, AppError> {
    let bounds = bounds_of(&frames)?;
    // The overlay takes the frames, so keep each monitor's rect and primary flag for an ActiveMonitor outcome.
    let monitors: Vec<(PhysRect, bool)> = frames.iter().map(|f| (frame_rect(f), f.monitor.primary)).collect();
    // Compose the desktop only after the overlay is up: the overlay paints the frames themselves,
    // and inputs queue in the channel until the composite is ready.
    let compose_frames = frames.clone();
    let mut state = RegionState::new(RegionConfig {
        bounds,
        multi_region: options.multi_region,
        confirm_on_select: !options.show_tools_after_select,
        pick_point,
        snap_to_targets: options.snap_to_windows,
        min_size: options.min_size,
        magnifier_pixels: options.magnifier_pixels,
        info_text: options.info_text.clone(),
        right_click: options.right_click,
        middle_click: options.middle_click,
    });

    let (tx, mut inputs) = mpsc::unbounded::<RegionInput>();
    let hover_tx = tx.clone();
    let on_input: InputCallback = Arc::new(move |input| {
        // The session may have ended already. A late input has nowhere to go.
        let _ = tx.unbounded_send(input);
    });
    let first = view(&state, bounds, options, pixel_in_frames(&frames, state.cursor()));
    ports.overlay.show(frames, first, on_input)?;

    // The session hands the composed desktop over for the region analysis once it has it.
    let (composed_tx, composed_rx) = oneshot::channel::<Bitmap>();
    let detect = options.detect_regions && options.snap_to_windows && !pick_point;
    let loaded = async {
        if let Ok(targets) = ports.hover.targets().await {
            let _ = hover_tx.unbounded_send(RegionInput::TargetsLoaded(targets.clone()));
            if detect && let Ok(composed) = composed_rx.await {
                detect_regions(ports.spawner.as_ref(), &hover_tx, targets, composed, bounds).await;
            }
        }
        // Drop this sender once the targets are in, so the input channel can end.
        drop(hover_tx);
        future::pending::<Result<Selection, AppError>>().await
    };

    let session = async {
        // Composed while the overlay is up and the hover targets load; inputs wait in the channel.
        let composed = match compose(ports.spawner.as_ref(), compose_frames).await {
            Ok(composed) => composed,
            Err(error) => {
                ports.overlay.close();
                return Err(error);
            }
        };
        let _ = composed_tx.send(composed.clone());
        // The selection the overlay was last given a crop for, and the annotated crop it sent since.
        let mut cropped: Option<PhysRect> = None;
        let mut annotated: Option<Bitmap> = None;
        while let Some(input) = inputs.next().await {
            // The overlay's edited crop is the app's to keep; the state has nothing to do with it.
            if let RegionInput::Annotated(bitmap) = input {
                annotated = Some(bitmap);
                continue;
            }
            let changed = state.handle(input);
            if let Some(outcome) = state.outcome().cloned() {
                ports.overlay.close();
                let annotate = state.annotate_requested();
                return outcome_selection(outcome, &composed, bounds, &monitors, state.cursor(), annotated)
                    .map(|s| Selection { annotate, ..s });
            }
            // Crop on a change of the selection only: moving the pointer over it is not one.
            let mut selection_image = None;
            let selected = state.selected();
            if selected != cropped {
                cropped = selected;
                // An annotated crop belongs to the selection it was drawn on.
                annotated = None;
                if let Some(rect) = selected {
                    match crop_selected(ports.spawner.as_ref(), &composed, bounds, rect).await {
                        Ok(image) => selection_image = Some(Arc::new(image)),
                        Err(error) => {
                            ports.overlay.close();
                            return Err(error);
                        }
                    }
                }
            }
            if changed || selection_image.is_some() {
                let pixel = pixel_at(&composed, bounds, state.cursor());
                ports.overlay.update(RegionView { selection_image, ..view(&state, bounds, options, pixel) });
            }
        }
        // Every sender is gone, so no input can arrive.
        ports.overlay.close();
        Err(AppError::Cancelled)
    };

    future::or(loaded, session).await
}

/// Finds the regions inside each window (front to back, so the window the user is most likely
/// over is ready first) and resends the targets with them. Sends nothing when no window has any.
async fn detect_regions(
    spawner: &dyn Spawner,
    tx: &mpsc::UnboundedSender<RegionInput>,
    targets: Vec<HoverTarget>,
    composed: Bitmap,
    bounds: PhysRect,
) {
    let windows = layout::windows(&targets, bounds);
    let mut found: Vec<(usize, Vec<PhysRect>)> = Vec::new();
    let last = windows.len().saturating_sub(1);
    for (n, (index, rect)) in windows.into_iter().enumerate() {
        let image = composed.clone();
        let Ok(rects) = compute(spawner, move || Ok(laqta_imaging::layout::regions(&image, rect))).await else { continue };
        let rects: Vec<PhysRect> = rects.into_iter().map(|r| PhysRect { x: r.x + bounds.x, y: r.y + bounds.y, ..r }).collect();
        if !rects.is_empty() {
            found.push((index, rects));
        }
        // Once after the front window, then once with every window.
        if !found.is_empty()
            && (n == 0 || n == last)
            && tx.unbounded_send(RegionInput::TargetsLoaded(layout::merge(targets.clone(), &found))).is_err()
        {
            return;
        }
    }
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
    Ok(Selection { rects, target, image, annotate: false, action: SelectionAction::Default })
}

/// The overlay's view of `state`. `pixel` is the RGB under the cursor, and `bounds` is the desktop
/// area the state was built with. `RegionState` does not expose it. The view has no selection image:
/// the session adds the crop to the view that follows a change of the selection.
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
        annotate: state.annotate_requested(),
        selected: state.selected(),
        selection_image: None,
        tools_visible: state.tools_visible(),
        locked: state.locked(),
        mode_bar: options.show_mode_bar,
        pick_point: state.picks_point(),
    }
}

/// Turns the session's outcome into a selection. `monitors` holds each frame's rect and primary flag,
/// and `cursor` is the point the session ended on. `annotated` is the overlay's edited crop of the
/// selection, if it sent one: a confirmed selection returns it in place of the plain crop.
fn outcome_selection(
    outcome: RegionOutcome,
    composed: &Bitmap,
    bounds: PhysRect,
    monitors: &[(PhysRect, bool)],
    cursor: (i32, i32),
    annotated: Option<Bitmap>,
) -> Result<Selection, AppError> {
    match outcome {
        RegionOutcome::Confirmed { rects, target, action } => {
            let image = match annotated {
                Some(image) => image,
                None => crop_rects(composed, bounds, &rects)?,
            };
            Ok(Selection { rects, target, image, annotate: false, action })
        }
        RegionOutcome::Fullscreen => whole_rect(composed, bounds, bounds),
        RegionOutcome::ActiveMonitor => {
            let rect = monitor_at(monitors, cursor).ok_or_else(|| AppError::Io("no monitors".to_owned()))?;
            whole_rect(composed, bounds, rect)
        }
        RegionOutcome::Cancelled => Err(AppError::Cancelled),
    }
}

/// A selection of one whole rect, with no hover target.
fn whole_rect(composed: &Bitmap, bounds: PhysRect, rect: PhysRect) -> Result<Selection, AppError> {
    let rects = vec![rect];
    let image = crop_rects(composed, bounds, &rects)?;
    Ok(Selection { rects, target: None, image, annotate: false, action: SelectionAction::Default })
}

/// The rect of the monitor under `at`. A point in a gap of a non-rectangular layout falls back to the
/// primary monitor, then the first one.
fn monitor_at(monitors: &[(PhysRect, bool)], at: (i32, i32)) -> Option<PhysRect> {
    let under = monitors.iter().find(|(rect, _)| contains(*rect, i64::from(at.0), i64::from(at.1)));
    under.or_else(|| monitors.iter().find(|(_, primary)| *primary)).or_else(|| monitors.first()).map(|(rect, _)| *rect)
}

/// A frame's desktop rect: the monitor's origin with the frame's own size. This is where `compose_monitors`
/// puts the frame.
fn frame_rect(frame: &Frame) -> PhysRect {
    PhysRect { x: frame.monitor.rect.x, y: frame.monitor.rect.y, width: frame.image.width(), height: frame.image.height() }
}

/// The union of the frame rects. This matches where `compose_monitors` puts the frames.
fn bounds_of(frames: &[Frame]) -> Result<PhysRect, AppError> {
    frames.iter().map(frame_rect).reduce(PhysRect::union).ok_or_else(|| AppError::Io("no frames to select from".to_owned()))
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

/// Crops one selected `rect` (desktop pixels) out of the composed desktop on the compute pool. A panic
/// there comes back as a value (ADR 020).
async fn crop_selected(spawner: &dyn Spawner, composed: &Bitmap, bounds: PhysRect, rect: PhysRect) -> Result<Bitmap, AppError> {
    let composed = composed.clone();
    compute(spawner, move || crop_rects(&composed, bounds, &[rect])).await
}

/// The RGB under `at` (desktop pixels), read from the frame that covers it. The first view uses it,
/// before the composite exists; frames sit where `compose_monitors` puts them (`frame_rect`), and a
/// later frame paints over an earlier one, so the last covering frame wins.
fn pixel_in_frames(frames: &[Frame], at: (i32, i32)) -> [u8; 3] {
    let (x, y) = (i64::from(at.0), i64::from(at.1));
    let covers = |r: &PhysRect| {
        (i64::from(r.x)..i64::from(r.x) + i64::from(r.width)).contains(&x)
            && (i64::from(r.y)..i64::from(r.y) + i64::from(r.height)).contains(&y)
    };
    let Some(frame) = frames.iter().rev().find(|f| covers(&frame_rect(f))) else { return [0; 3] };
    let rect = frame_rect(frame);
    let i = ((y - i64::from(rect.y)) * i64::from(rect.width) + (x - i64::from(rect.x))) as usize * 4;
    let Some(px) = frame.image.pixels().get(i..i + 4) else { return [0; 3] };
    match frame.image.format() {
        PixelFormat::Rgba8 => [px[0], px[1], px[2]],
        PixelFormat::Bgra8 => [px[2], px[1], px[0]],
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
    let in_composed = PhysRect { x: boxed.x.saturating_sub(bounds.x), y: boxed.y.saturating_sub(bounds.y), ..boxed };
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
