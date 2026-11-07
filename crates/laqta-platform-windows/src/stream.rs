//! The Windows `ScreenStream` (ADR 016, M4 D1 and D6). A region is described as a DXGI output and an
//! offset on it, so `laqta-media` can run `ddagrab` on the adapter that owns the output. System sound
//! is a WASAPI loopback `Pcm` input (`loopback.rs`). Microphones are passed through by their friendly
//! name for `dshow`. The output lookup is pure and is tested on every target.

use laqta_domain::geometry::PhysRect;
use laqta_ports::PortError;
use laqta_ports::recording::OsScreen;

#[cfg(windows)]
use laqta_domain::record::AudioDevice;
#[cfg(windows)]
use laqta_ports::recording::{AudioInput, OsScreenRef, RecordingInput, VideoSource};
#[cfg(windows)]
use laqta_ports::{BoxFuture, ScreenStream};
#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

#[cfg(windows)]
use crate::loopback::{WasapiLoopback, list_devices};

/// The screen the area is on, as an OS reference. `outputs` holds `(adapter index, output index,
/// desktop rect)` for each attached display. The display is the one that holds the area's top-left
/// corner. The area is clipped to that display, and each side is rounded down to an even count.
pub(crate) fn screen_for_area(outputs: &[(u32, u32, PhysRect)], area: PhysRect) -> Result<OsScreen, PortError> {
    let &(adapter_index, output_index, output) = outputs
        .iter()
        .find(|(_, _, rect)| holds_point(*rect, area.x, area.y))
        .ok_or_else(|| PortError::Failed(format!("no display holds the point ({}, {})", area.x, area.y)))?;
    let width = even_extent(area.x, area.right().min(output.right()));
    let height = even_extent(area.y, area.bottom().min(output.bottom()));
    if width == 0 || height == 0 {
        return Err(PortError::Failed("the area is smaller than two pixels on its display".into()));
    }
    Ok(OsScreen::Windows {
        adapter_index,
        output_index,
        offset_x: offset(area.x, output.x)?,
        offset_y: offset(area.y, output.y)?,
        width,
        height,
        desktop_x: area.x,
        desktop_y: area.y,
    })
}

/// True when the point is inside `rect`. The right and bottom edges are outside it.
fn holds_point(rect: PhysRect, x: i32, y: i32) -> bool {
    (rect.x..rect.right()).contains(&x) && (rect.y..rect.bottom()).contains(&y)
}

/// The distance from `from` to `to`, rounded down to an even count, or 0 when `to` is not past `from`.
fn even_extent(from: i32, to: i32) -> u32 {
    u32::try_from(i64::from(to) - i64::from(from)).unwrap_or(0) & !1
}

/// `from - origin`, the offset of a point inside a display.
fn offset(from: i32, origin: i32) -> Result<i32, PortError> {
    i32::try_from(i64::from(from) - i64::from(origin)).map_err(|_| PortError::Failed("area offset is out of range".into()))
}

/// Every attached display as `(adapter index, output index, desktop rect)`. The indexes follow DXGI
/// enumeration (`EnumAdapters1`, then `EnumOutputs` on each adapter), which is what `ddagrab` takes.
/// An adapter with no outputs still uses up its index.
#[cfg(windows)]
fn enumerate_outputs() -> Result<Vec<(u32, u32, PhysRect)>, PortError> {
    // SAFETY: plain DXGI calls on objects this function owns.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| PortError::Failed(format!("DXGI factory: {e}")))?;
        let mut outputs = Vec::new();
        let mut adapter_index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            let mut output_index = 0;
            while let Ok(output) = adapter.EnumOutputs(output_index) {
                if let Ok(desc) = output.GetDesc()
                    && desc.AttachedToDesktop.as_bool()
                {
                    let edges = desc.DesktopCoordinates;
                    let rect = PhysRect {
                        x: edges.left,
                        y: edges.top,
                        width: edges.right.abs_diff(edges.left),
                        height: edges.bottom.abs_diff(edges.top),
                    };
                    outputs.push((adapter_index, output_index, rect));
                }
                output_index += 1;
            }
            adapter_index += 1;
        }
        Ok(outputs)
    }
}

/// Builds the recording input for `area`. Loopback devices become WASAPI `Pcm` inputs. Every other
/// device is passed through to `dshow` as listed.
#[cfg(windows)]
fn open_input(area: PhysRect, audio: &[AudioDevice]) -> Result<RecordingInput, PortError> {
    let screen = screen_for_area(&enumerate_outputs()?, area)?;
    let mut inputs = Vec::with_capacity(audio.len());
    for device in audio {
        inputs.push(if device.loopback {
            AudioInput::Pcm(Box::new(WasapiLoopback::start()?))
        } else {
            AudioInput::Device(device.clone())
        });
    }
    Ok(RecordingInput { video: VideoSource::Target(OsScreenRef::new(screen)), audio: inputs })
}

/// The Windows screen stream. It keeps no state: each `open` looks the displays up again.
#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct WindowsStream;

#[cfg(windows)]
impl ScreenStream for WindowsStream {
    // `show_cursor` and `fps` are not part of the output reference. `laqta-media` reads them from
    // the record spec.
    fn open(&self, area: PhysRect, _show_cursor: bool, _fps: u32, audio: &[AudioDevice]) -> BoxFuture<'_, Result<RecordingInput, PortError>> {
        let audio = audio.to_vec();
        Box::pin(blocking::unblock(move || open_input(area, &audio)))
    }

    fn audio_devices(&self) -> BoxFuture<'_, Result<Vec<AudioDevice>, PortError>> {
        Box::pin(blocking::unblock(list_devices))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysRect {
        PhysRect { x, y, width, height }
    }

    /// Adapter 0 has a 1920x1080 display at the origin and a 1280x1024 display to its right, one
    /// 200 px higher. Adapter 1 has a 2560x1440 display to the left of the origin.
    fn desktop() -> Vec<(u32, u32, PhysRect)> {
        vec![(0, 0, rect(0, 0, 1920, 1080)), (0, 1, rect(1920, -200, 1280, 1024)), (1, 0, rect(-2560, 0, 2560, 1440))]
    }

    #[test]
    fn area_on_the_first_display() {
        assert_eq!(
            screen_for_area(&desktop(), rect(100, 200, 800, 600)).ok(),
            Some(OsScreen::Windows { adapter_index: 0, output_index: 0, offset_x: 100, offset_y: 200, width: 800, height: 600, desktop_x: 100, desktop_y: 200 })
        );
    }

    #[test]
    fn second_output_of_the_same_adapter_gets_its_own_offsets() {
        assert_eq!(
            screen_for_area(&desktop(), rect(2000, -100, 640, 480)).ok(),
            Some(OsScreen::Windows { adapter_index: 0, output_index: 1, offset_x: 80, offset_y: 100, width: 640, height: 480, desktop_x: 2000, desktop_y: -100 })
        );
    }

    #[test]
    fn display_on_another_adapter_with_negative_coordinates() {
        assert_eq!(
            screen_for_area(&desktop(), rect(-2000, 10, 1000, 500)).ok(),
            Some(OsScreen::Windows { adapter_index: 1, output_index: 0, offset_x: 560, offset_y: 10, width: 1000, height: 500, desktop_x: -2000, desktop_y: 10 })
        );
    }

    #[test]
    fn area_crossing_an_edge_is_clipped_to_its_display() {
        assert_eq!(
            screen_for_area(&desktop(), rect(1800, 100, 400, 300)).ok(),
            Some(OsScreen::Windows { adapter_index: 0, output_index: 0, offset_x: 1800, offset_y: 100, width: 120, height: 300, desktop_x: 1800, desktop_y: 100 })
        );
    }

    #[test]
    fn sides_round_down_to_even() {
        assert_eq!(
            screen_for_area(&desktop(), rect(0, 0, 801, 601)).ok(),
            Some(OsScreen::Windows { adapter_index: 0, output_index: 0, offset_x: 0, offset_y: 0, width: 800, height: 600, desktop_x: 0, desktop_y: 0 })
        );
    }

    #[test]
    fn right_edge_belongs_to_the_next_display() {
        // x = 1920 is outside the first display (its right edge is exclusive) and inside the second.
        assert_eq!(
            screen_for_area(&desktop(), rect(1920, 0, 100, 100)).ok(),
            Some(OsScreen::Windows { adapter_index: 0, output_index: 1, offset_x: 0, offset_y: 200, width: 100, height: 100, desktop_x: 1920, desktop_y: 0 })
        );
    }

    #[test]
    fn point_outside_every_display_is_refused() {
        assert!(screen_for_area(&desktop(), rect(5000, 5000, 100, 100)).is_err());
    }

    #[test]
    fn area_smaller_than_two_pixels_is_refused() {
        assert!(screen_for_area(&desktop(), rect(10, 10, 1, 100)).is_err());
        assert!(screen_for_area(&desktop(), rect(10, 10, 0, 0)).is_err());
    }
}
