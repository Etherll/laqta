# ADR 016: Recording consumes a ScreenStream

Status: accepted (2026-11-07), revised after the EGM review and ADR 024; re-confirmed 2026-11-07 by the trade-offs review

**Context.**
- Recording needs frames over time, not stills, and the first plan had `laqta-media` call the platform's still-capture code.
- Audio capture also lives in the platform crates.
- Stock FFmpeg builds cannot reliably read a PipeWire node from the ScreenCast portal.

**Decision.** The platform module provides a `ScreenStream` (in `laqta-ports`), described in Laqta's own terms, with no FFmpeg vocabulary (ADR 024).

- **Video is one of two kinds:**
  - `Frames`: a frame source that Laqta pulls from. This is the WGC frame pool on Windows and PipeWire frames on Wayland.
  - `Target`: a screen target (monitor, window or region in physical pixels) that the recorder may capture itself. Used on Windows and X11. It carries a sealed `OsScreenRef`, either a Windows output (adapter LUID and output index) or an X11 display and screen. Only the platform adapter creates it; the application passes it through unread; `laqta-media` reads it to build `ddagrab` or `x11grab` input. This is an accepted, contained coupling between two adapters. It names OS concepts, not FFmpeg ones.
- **Audio inputs are a list of:**
  - `Device`: an audio device ID and the OS's name for it, as listed by the platform module: the WASAPI friendly name on Windows, which `dshow` accepts, and the PulseAudio source name on Linux.
  - `Pcm`: a PCM source the platform module captures, used for Windows system audio through WASAPI (the `wasapi` crate).

`laqta-media` alone maps these to FFmpeg inputs:
- a `Target` becomes `ddagrab` on Windows or `x11grab` on X11;
- a `Device` becomes `dshow` on Windows, or `pulse` on Linux, which also works on PipeWire systems through pipewire-pulse;
- `Frames` and `Pcm` are written to pipes.

FFmpeg has no `wasapi` or `pipewire` input device, and `dshow` cannot record loopback, which is why system audio is a `Pcm` input. `laqta-media` never calls platform code.

**Rejected.**
- **An `AudioSource` port:** it has no second real adapter until a native recorder exists (ADR 021). The Windows loopback source is part of `ScreenStream` instead.
- **Assuming FFmpeg reads PipeWire nodes:** that is not true for stock builds.

**Consequences.** On Wayland, frame pumping runs inside Laqta and counts toward the recording CPU budget. The native recorder in M7 may add in-process audio frames.

**Why `OsScreenRef` stays (trade-offs review, 2026-11-07).** Removing it was proposed and rejected. Without it, `laqta-media` would have to call the OS. It would have to work out the GPU adapter (set through `-init_hw_device d3d11va:<adapter>`), the output index and the capture offsets (`offset_x`, `offset_y`, `video_size`) that `ddagrab` needs. The platform adapter already knows all of these, because it creates one D3D device per adapter. Removing it would also break this ADR's rule that `laqta-media` never calls platform code.

**Open question for the M4 plan.** On hybrid-GPU laptops, `ddagrab` must run on the adapter that owns the output. Check which adapter and output index `OsScreenRef` must carry, and that the hardware encoder can take frames from that adapter.
