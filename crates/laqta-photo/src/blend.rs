//! The 16 W3C compositing blend modes (Compositing and Blending, Level 1) in sRGB, with the general
//! source-over formula for partially transparent backdrops. Colours are straight (not premultiplied)
//! and in 0..1. Nothing is linearised: browsers and ShareX blend encoded values.

/// A layer's blend mode. `Normal` is plain source-over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

/// Hard light: multiply with `2 cs` when `cs <= 0.5`, otherwise screen with `2 cs - 1`.
fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb * 2.0 * cs
    } else {
        let s = 2.0 * cs - 1.0;
        cb + s - cb * s
    }
}

/// Soft light. The spec's `D(Cb)` switches to the polynomial at `Cb <= 0.25`, not at `0.5`.
fn soft_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
    } else {
        let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() };
        cb + (2.0 * cs - 1.0) * (d - cb)
    }
}

/// The separable modes, one channel. The non-separable modes return `cs` here and are handled by
/// [`non_separable`].
fn separable(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    use BlendMode::*;
    match mode {
        Normal | Hue | Saturation | Color | Luminosity => cs,
        Multiply => cb * cs,
        Screen => cb + cs - cb * cs,
        // Overlay is hard light with the arguments swapped.
        Overlay => hard_light(cs, cb),
        Darken => cb.min(cs),
        Lighten => cb.max(cs),
        // Edge cases are checked in the spec's order: the backdrop first, then the source.
        ColorDodge => {
            if cb <= 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        ColorBurn => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        HardLight => hard_light(cb, cs),
        SoftLight => soft_light(cb, cs),
        Difference => (cb - cs).abs(),
        Exclusion => cb + cs - 2.0 * cb * cs,
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    // n and x are taken once from the input, as the spec's ClipColor does.
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 && l - n > f32::EPSILON {
        for v in &mut out {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 && x - l > f32::EPSILON {
        for v in &mut out {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    out
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

/// SetSat: the largest channel becomes `s`, the smallest becomes 0, the middle one is scaled.
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mut out = [0.0_f32; 3];
    let imax = (0..3).fold(0, |best, i| if c[i] > c[best] { i } else { best });
    let imin = (0..3).fold(0, |best, i| if c[i] < c[best] { i } else { best });
    if c[imax] > c[imin] {
        let imid = 3 - imax - imin;
        out[imid] = (c[imid] - c[imin]) * s / (c[imax] - c[imin]);
        out[imax] = s;
    }
    out
}

/// The non-separable modes, which mix whole colours through luminosity and saturation.
fn non_separable(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        BlendMode::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        BlendMode::Color => set_lum(cs, lum(cb)),
        BlendMode::Luminosity => set_lum(cb, lum(cs)),
        _ => cs,
    }
}

/// B(Cb, Cs) for all three channels.
pub(crate) fn blend_rgb(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Hue | BlendMode::Saturation | BlendMode::Color | BlendMode::Luminosity => non_separable(mode, cb, cs),
        _ => [separable(mode, cb[0], cs[0]), separable(mode, cb[1], cs[1]), separable(mode, cb[2], cs[2])],
    }
}

/// Composites a straight-alpha source over a straight-alpha backdrop (W3C section 5):
/// `ar = as + ab - as ab`, `Cr = (1 - as/ar) Cb + (as/ar) ((1 - ab) Cs + ab B(Cb, Cs))`.
/// Returns straight RGBA. A transparent source leaves the backdrop unchanged.
pub(crate) fn composite(mode: BlendMode, cb: [f32; 4], cs: [f32; 4]) -> [f32; 4] {
    let (ab, asrc) = (cb[3], cs[3]);
    if asrc <= 0.0 {
        return cb;
    }
    let ar = asrc + ab - asrc * ab;
    let b = blend_rgb(mode, [cb[0], cb[1], cb[2]], [cs[0], cs[1], cs[2]]);
    let k = asrc / ar;
    let mut out = [0.0_f32; 4];
    for (i, o) in out.iter_mut().take(3).enumerate() {
        *o = (1.0 - k) * cb[i] + k * ((1.0 - ab) * cs[i] + ab * b[i]);
    }
    out[3] = ar;
    out
}
