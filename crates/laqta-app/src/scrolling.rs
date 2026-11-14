//! Scrolling capture: select a region, then capture it, scroll the window under it through
//! `InputSynth`, and stitch each frame with `laqta_imaging::stitch` until the page stops moving.

use std::sync::Arc;
use std::time::Duration;

use laqta_domain::geometry::PhysRect;
use laqta_domain::workflow::{TaskSettings, WorkflowId};
use laqta_domain::{Action, Bitmap};
use laqta_imaging::stitch::{StitchOptions, StitchStep, Stitcher};
use laqta_ports::{AppError, CaptureRequest, PortError, Toast};

use crate::dispatcher::{App, Dispatched};
use crate::region;
use crate::task::{CaptureImage, CaptureInfo, TaskJob, compute};

/// Shown once when the window cannot take injected scrolls (Wayland): the user scrolls by hand.
const MANUAL_NOTICE: &str = "Scroll the page yourself; capture stops when it stops moving";
/// Unchanged frames in a row that end an automatic scroll: the page is at its end.
const AUTO_END_UNCHANGED: u32 = 2;
/// Unchanged frames in a row that end a manual scroll: the user has stopped.
const MANUAL_END_UNCHANGED: u32 = 6;

impl App {
    pub(crate) async fn scrolling_capture(
        self: &Arc<Self>,
        ts: TaskSettings,
        _workflow: Option<WorkflowId>,
    ) -> Result<Dispatched, AppError> {
        let frames = self.frames(&ts).await?;
        let sel = region::pick(&self.ports, frames, &ts.capture.region).await?;
        let rect = sel.rects.iter().copied().reduce(PhysRect::union).ok_or_else(|| AppError::Io("no region selected".to_owned()))?;
        let scrolling = ts.capture.scrolling.clone();
        let request = CaptureRequest {
            show_cursor: ts.capture.show_cursor,
            hdr_tonemap: ts.capture.hdr_tonemap,
            include_own_windows: ts.capture.include_own_windows,
            in_series: true,
        };
        // The wheel and keys go to the window under the middle of the region.
        let (x, y) = (rect.x.saturating_add_unsigned(rect.width / 2), rect.y.saturating_add_unsigned(rect.height / 2));

        self.pause(scrolling.start_delay_ms).await;
        let first = self.capture_rect(rect, request).await?;
        let options =
            StitchOptions { ignore_top: scrolling.ignore_top, ignore_bottom: scrolling.ignore_bottom, ..StitchOptions::default() };
        let mut stitcher = self.on_compute(move || Stitcher::new(&first, options)).await?;

        // `max_frames` counts every frame, the first one included.
        let max_frames = scrolling.max_frames.max(1);
        let mut taken = 1;
        let mut unchanged = 0;
        let mut manual = false;
        while taken < max_frames {
            if !manual {
                match self.ports.input.scroll(x, y, scrolling.method, scrolling.scroll_amount) {
                    Ok(()) => {}
                    Err(PortError::Unsupported(_)) => {
                        manual = true;
                        self.notify_manual(&ts);
                    }
                    Err(error) => return Err(AppError::from(error)),
                }
            }
            self.pause(scrolling.scroll_delay_ms).await;
            let frame = self.capture_rect(rect, request).await?;
            let (next, step) = self.push_frame(stitcher, frame).await?;
            stitcher = next;
            taken += 1;
            match step {
                StitchStep::Added { .. } => unchanged = 0,
                StitchStep::NoChange => {
                    unchanged += 1;
                    let limit = if manual { MANUAL_END_UNCHANGED } else { AUTO_END_UNCHANGED };
                    if unchanged >= limit {
                        break;
                    }
                }
                // The stitcher has joined the frame without an overlap, so the result has a seam there.
                StitchStep::NoMatch => {
                    tracing::warn!(frames = taken, "scrolling capture: no overlap with the last frame, stopping");
                    break;
                }
            }
        }

        let image = self.on_compute(move || stitcher.result()).await?;
        // Same job as `App::submit` for a region capture, so the after-capture tasks run.
        let info = sel
            .target
            .as_ref()
            .map(|t| CaptureInfo { window_title: t.title.clone(), process_name: t.process.clone(), ..Default::default() })
            .unwrap_or_default();
        Ok(Dispatched::Task(self.task.submit(TaskJob::Capture { image: CaptureImage::Image(image), settings: ts, info })))
    }

    /// Waits `ms` on the clock. Zero waits nothing.
    async fn pause(&self, ms: u32) {
        if ms > 0 {
            self.ports.clock.sleep(Duration::from_millis(u64::from(ms))).await;
        }
    }

    /// One fresh capture cropped to `rect`, as a region capture crops its frames.
    async fn capture_rect(&self, rect: PhysRect, request: CaptureRequest) -> Result<Bitmap, AppError> {
        let frames = self.ports.screen.capture(request).await.map_err(AppError::from)?;
        Ok(region::crop_frames(&self.ports, frames, vec![rect], None).await?.image)
    }

    /// Runs `work` on the compute pool. A panic there comes back as an error (ADR 020).
    async fn on_compute<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> Result<T, AppError> {
        compute(self.ports.spawner.as_ref(), move || Ok(work())).await
    }

    /// Joins one frame onto the stitcher. The stitcher goes to the compute pool and comes back.
    async fn push_frame(&self, stitcher: Stitcher, frame: Bitmap) -> Result<(Stitcher, StitchStep), AppError> {
        self.on_compute(move || {
            let mut stitcher = stitcher;
            let step = stitcher.push(&frame);
            (stitcher, step)
        })
        .await
    }

    /// The one notice that the user has to scroll. A failed notice does not stop the capture.
    fn notify_manual(&self, ts: &TaskSettings) {
        let toast = Toast {
            title: Action::ScrollingCapture.label().to_owned(),
            body: MANUAL_NOTICE.to_owned(),
            image: None,
            path: None,
            url: None,
            options: ts.general.toast.clone(),
        };
        if let Err(error) = self.ports.notifier.toast(toast) {
            tracing::warn!(%error, "scrolling capture: the scroll notice was not shown");
        }
    }
}
