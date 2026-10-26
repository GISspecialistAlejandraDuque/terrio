//! Blending of two Byte rasters: an equivalent of `gdal raster blend` (GDAL 3.13).
//!
//! The operators and their integer arithmetic are GDAL's: `src-over` (alpha compositing),
//! `hsv-value` (the overlay's value with the color input's hue and saturation, as
//! `hsv_merge.py` does), and the separable modes `multiply`, `screen`, `overlay`, `hard-light`,
//! `darken`, `lighten`, `color-burn` and `color-dodge` on premultiplied colors (MapServer's
//! formulas). Inputs have 1 (gray), 2 (gray and alpha), 3 (RGB) or 4 (RGBA) Byte bands; a
//! single paletted band is expanded to RGBA first.

use crate::{Error, RasterDataset, Result};
use wbraster::DataType;

/// A composition operator of `gdal raster blend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlendOperator {
    /// The overlay over the input, by its alpha and the opacity (the default).
    #[default]
    SrcOver,
    /// The overlay's value (HSV) with the input's hue and saturation.
    HsvValue,
    /// The product of the colors.
    Multiply,
    /// The complement of the product of the complements.
    Screen,
    /// Multiply or screen, by the input's color.
    Overlay,
    /// Multiply or screen, by the overlay's color.
    HardLight,
    /// The darker of the colors.
    Darken,
    /// The lighter of the colors.
    Lighten,
    /// The input darkened by the overlay.
    ColorBurn,
    /// The input brightened by the overlay.
    ColorDodge,
}

impl BlendOperator {
    /// Parses GDAL's name of an operator: `src-over`, `hsv-value`, `multiply`, `screen`,
    /// `overlay`, `hard-light`, `darken`, `lighten`, `color-burn` or `color-dodge`.
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "src-over" => Self::SrcOver,
            "hsv-value" => Self::HsvValue,
            "multiply" => Self::Multiply,
            "screen" => Self::Screen,
            "overlay" => Self::Overlay,
            "hard-light" => Self::HardLight,
            "darken" => Self::Darken,
            "lighten" => Self::Lighten,
            "color-burn" => Self::ColorBurn,
            "color-dodge" => Self::ColorDodge,
            other => {
                return Err(Error::InvalidArgument(format!(
                    "unknown operator '{other}' (src-over, hsv-value, multiply, screen, overlay, hard-light, darken, lighten, color-burn, color-dodge)"
                )));
            }
        })
    }

    /// GDAL's name of the operator.
    pub fn name(self) -> &'static str {
        match self {
            Self::SrcOver => "src-over",
            Self::HsvValue => "hsv-value",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
            Self::Overlay => "overlay",
            Self::HardLight => "hard-light",
            Self::Darken => "darken",
            Self::Lighten => "lighten",
            Self::ColorBurn => "color-burn",
            Self::ColorDodge => "color-dodge",
        }
    }
}

/// Options for [`blend`], named after `gdal raster blend`'s.
#[derive(Debug, Clone, Copy)]
pub struct BlendOptions {
    /// The operator (`--operator`).
    pub operator: BlendOperator,
    /// The overlay's opacity in percent, 0 to 100 (`--opacity`).
    pub opacity: u32,
}

impl Default for BlendOptions {
    fn default() -> Self {
        Self { operator: BlendOperator::SrcOver, opacity: 100 }
    }
}

/// A product of two 0-255 ratios, rounded up (GDAL's `MulScale255`).
#[inline]
fn mul(a: u8, b: u8) -> u8 {
    (a as u32 * b as u32).div_ceil(256) as u8
}

/// A quotient of two 0-255 ratios, rounded down (GDAL's `DivScale255`), wrapping like its cast.
#[inline]
fn div(a: u8, b: u8) -> u8 {
    if a == 0 {
        0
    } else if b == 0 {
        255
    } else {
        ((a as u32 * 255) / b as u32) as u8
    }
}

/// An `int` passed where GDAL takes a byte: wrapped modulo 256, as C converts it.
#[inline]
fn byte(v: i32) -> u8 {
    v as u8
}

/// The bands of one input at one pixel: gray or red, then green and blue, then alpha.
#[derive(Clone, Copy)]
struct Px {
    r: u8,
    g: Option<u8>,
    b: Option<u8>,
    a: Option<u8>,
}

impl Px {
    fn bands(self) -> usize {
        1 + if self.g.is_some() { 2 } else { 0 } + usize::from(self.a.is_some())
    }

    /// The premultiplied color components, a missing green or blue being the red's.
    fn premultiplied(self, a: u8) -> [u8; 3] {
        if a == 255 {
            let r = self.r;
            [r, self.g.unwrap_or(r), self.b.unwrap_or(r)]
        } else {
            let r = mul(self.r, a);
            [r, self.g.map(|v| mul(v, a)).unwrap_or(r), self.b.map(|v| mul(v, a)).unwrap_or(r)]
        }
    }
}

/// The separable operators on premultiplied colors (`Blend*_Generic`): the output bands, as
/// many as the larger input has (gray, gray and alpha, RGB or RGBA).
fn separable(op: BlendOperator, s: Px, o: Px, opacity: u8, swapped: bool) -> [u8; 4] {
    if op == BlendOperator::HardLight {
        // Hard light is overlay with the two inputs' roles swapped.
        return separable(BlendOperator::Overlay, o, s, opacity, !swapped);
    }
    let (a, oa) = if swapped {
        (s.a.map(|v| mul(v, opacity)).unwrap_or(opacity), o.a.unwrap_or(255))
    } else {
        (s.a.unwrap_or(255), o.a.map(|v| mul(v, opacity)).unwrap_or(opacity))
    };
    let fin = byte(oa as i32 + a as i32 - mul(oa, a) as i32);
    let c = s.premultiplied(a);
    let oc = o.premultiplied(oa);
    let alpha_mul = mul(oa, a) as i32;
    let rest = |c: u8, oc: u8| mul(c, 255 - oa) as i32 + mul(oc, 255 - a) as i32;
    let component = |c: u8, oc: u8| -> u8 {
        match op {
            BlendOperator::Multiply => div(byte(mul(c, oc) as i32 + rest(c, oc)), fin),
            BlendOperator::Screen => div(byte(c as i32 + oc as i32 - mul(c, oc) as i32), fin),
            BlendOperator::Overlay => {
                if 2 * (c as i32) < a as i32 {
                    div(byte(2 * mul(c, oc) as i32 + rest(c, oc)), fin)
                } else {
                    let v = alpha_mul - 2 * mul(byte(a as i32 - c as i32), byte(oa as i32 - oc as i32)) as i32 + rest(c, oc);
                    div(byte(v), fin)
                }
            }
            BlendOperator::Darken => div(byte((mul(oc, a).min(mul(c, oa))) as i32 + rest(c, oc)), fin),
            BlendOperator::Lighten => div(byte((mul(oc, a).max(mul(c, oa))) as i32 + rest(c, oc)), fin),
            BlendOperator::ColorDodge => {
                if mul(oc, a) as i32 + mul(c, oa) as i32 >= alpha_mul {
                    div(byte(alpha_mul + rest(c, oc)), fin)
                } else {
                    let v = div(mul(c, oa), byte(255 - div(oc, oa) as i32)) as i32 + rest(c, oc);
                    div(byte(v), fin)
                }
            }
            BlendOperator::ColorBurn => {
                if mul(oc, a) as i32 + mul(c, oa) as i32 <= alpha_mul {
                    div(byte(rest(c, oc)), fin)
                } else {
                    let (cu, ocu) = (div(c, a), div(oc, oa));
                    let v = mul(alpha_mul as u8, byte(cu as i32 + ocu as i32 - 255)) as i32 + rest(c, oc);
                    div(byte(v), fin)
                }
            }
            BlendOperator::SrcOver | BlendOperator::HsvValue | BlendOperator::HardLight => unreachable!(),
        }
    };
    let mut out = [0u8; 4];
    out[0] = component(c[0], oc[0]);
    match s.bands().max(o.bands()) {
        2 => out[1] = fin,
        n => {
            if n >= 3 {
                out[1] = component(c[1], oc[1]);
                out[2] = component(c[2], oc[2]);
            }
            if n == 4 {
                out[3] = fin;
            }
        }
    }
    out
}

/// Hue and saturation (0 to 1) of an RGB color, as GDAL's `rgb_to_hs` computes them in single
/// precision; `reciprocal` multiplies by `1 / (6 * (max - min))` as GDAL's SSE2 code does,
/// instead of dividing.
fn rgb_to_hs(r: i32, g: i32, b: i32, reciprocal: bool) -> (f32, f32) {
    let minc = r.min(g).min(b);
    let maxc = r.max(g).max(b);
    let d = maxc - minc;
    let s = d as f32 / (maxc.max(1) as f32);
    let six = if d == 0 { 1.0f32 } else { 6.0f32 * d as f32 };
    let q = |x: i32| if reciprocal { x as f32 * (1.0f32 / six) } else { x as f32 / six };
    let h = if maxc == b {
        4.0f32 / 6.0 + q(r - g)
    } else if maxc == g {
        2.0f32 / 6.0 + q(b - r)
    } else {
        let t = q(g - b);
        if t < 0.0 { t + 1.0 } else { t }
    };
    (h, s)
}

/// An RGB color from hue and saturation (0 to 1) and a value (0 to 255), as GDAL's `hsv_to_rgb`.
fn hsv_to_rgb(h: f32, s: f32, v: u8) -> [u8; 3] {
    let i = (6.0f32 * h) as i32;
    let f = 6.0f32 * h - i as f32;
    let vf = v as f32;
    let p = (vf * (1.0 - s) + 0.5) as u8;
    let q = (vf * (1.0 - s * f) + 0.5) as u8;
    let t = (vf * (1.0 - s * (1.0 - f)) + 0.5) as u8;
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// The first `n` bands of a Byte raster, as slices.
fn byte_bands(ds: &RasterDataset) -> Vec<&[u8]> {
    let n = ds.raster.cols * ds.raster.rows;
    let wbraster::raster::RasterData::U8(v) = &ds.raster.data else { return Vec::new() };
    (0..ds.raster.bands).map(|b| &v[b * n..(b + 1) * n]).collect()
}

/// A single paletted band expanded to RGBA (`gdal_translate -expand rgba`): indices outside the
/// table become 0.
fn expand_palette(ds: &RasterDataset) -> Result<RasterDataset> {
    let n = ds.raster.cols * ds.raster.rows;
    let view = super::BandView::of(&ds.raster, 0);
    let mut data = vec![0u8; n * 4];
    for i in 0..n {
        let v = view.get(i);
        if let Some(e) = (v >= 0.0).then(|| ds.color_table.get(v as usize)).flatten() {
            for c in 0..4 {
                data[c * n + i] = e[c];
            }
        }
    }
    let mut out = super::derived(ds, wbraster::raster::RasterData::U8(data), 4)?;
    out.band_metadata = ["Red", "Green", "Blue", "Alpha"]
        .iter()
        .map(|c| crate::BandMetadata { color_interp: Some((*c).into()), ..Default::default() })
        .collect();
    Ok(out)
}

/// The number of color bands of a 1- to 4-band input (alpha left out).
fn color_bands(n: usize) -> usize {
    if n == 2 || n == 4 { n - 1 } else { n }
}

/// Blends an overlay onto a color input, as `gdal raster blend` does: both are Byte rasters of
/// 1 to 4 bands and the same size (single paletted bands are expanded to RGBA first), and the
/// output has the color input's bands, georeferencing and CRS (the overlay's when GDAL swaps
/// them: for `multiply`, `screen`, `overlay` and `hard-light` with an overlay of more bands).
pub fn blend(color: &RasterDataset, overlay: &RasterDataset, o: &BlendOptions) -> Result<RasterDataset> {
    let expanded = |ds: &RasterDataset| -> Result<Option<RasterDataset>> {
        (ds.raster.bands == 1 && !ds.color_table.is_empty()).then(|| expand_palette(ds)).transpose()
    };
    let (ce, oe) = (expanded(color)?, expanded(overlay)?);
    let (mut color, mut overlay) = (ce.as_ref().unwrap_or(color), oe.as_ref().unwrap_or(overlay));
    for (ds, what) in [(color, "input"), (overlay, "overlay")] {
        if ds.raster.bands == 0 || ds.raster.bands > 4 || ds.raster.data_type != DataType::U8 || ds.complex {
            return Err(Error::Unsupported(format!("only 1-band, 2-band, 3-band or 4-band Byte datasets are supported as {what}")));
        }
    }
    if (color.raster.cols, color.raster.rows) != (overlay.raster.cols, overlay.raster.rows) {
        return Err(Error::InvalidArgument("the input and overlay datasets must have the same dimensions".into()));
    }
    let op = o.operator;
    if op == BlendOperator::HsvValue && color.raster.bands < 3 {
        return Err(Error::InvalidArgument(format!(
            "the input has {} band(s), but operator hsv-value requires between 3 and 4 bands",
            color.raster.bands
        )));
    }
    if matches!(op, BlendOperator::Darken | BlendOperator::Lighten) && color_bands(color.raster.bands) != color_bands(overlay.raster.bands)
    {
        return Err(Error::InvalidArgument(format!(
            "for the lighten and darken operators, the input and overlay must have the same number of bands (without considering alpha); they have {} and {}",
            color_bands(color.raster.bands),
            color_bands(overlay.raster.bands)
        )));
    }
    if o.opacity > 100 {
        return Err(Error::InvalidArgument("the opacity is a percentage, from 0 to 100".into()));
    }
    let opacity = ((o.opacity * 255 + 50) / 100) as u8;
    let mut swapped = false;
    if matches!(op, BlendOperator::Multiply | BlendOperator::Screen | BlendOperator::HardLight | BlendOperator::Overlay)
        && color.raster.bands < overlay.raster.bands
    {
        swapped = true;
        std::mem::swap(&mut color, &mut overlay);
    }
    let (cb, ob) = (byte_bands(color), byte_bands(overlay));
    let (nc, no) = (cb.len(), ob.len());
    let n = color.raster.cols * color.raster.rows;
    let px = |bands: &[&[u8]], i: usize| Px {
        r: bands[0][i],
        g: (bands.len() >= 3).then(|| bands[1][i]),
        b: (bands.len() >= 3).then(|| bands[2][i]),
        a: match bands.len() {
            2 => Some(bands[1][i]),
            4 => Some(bands[3][i]),
            _ => None,
        },
    };
    let op255 = opacity as i32;
    let mut out = vec![0u8; n * nc];
    let mut set = |i: usize, values: &[u8]| {
        for (k, v) in values.iter().take(nc).enumerate() {
            out[k * n + i] = *v;
        }
    };
    match op {
        BlendOperator::HsvValue => {
            for i in 0..n {
                let (r, g, b) = (cb[0][i] as i32, cb[1][i] as i32, cb[2][i] as i32);
                let value = if no == 1 && opacity == 255 {
                    ob[0][i]
                } else {
                    let ov = if no >= 3 { ob[0][i].max(ob[1][i]).max(ob[2][i]) } else { ob[0][i] } as i32;
                    let oa = match no {
                        2 | 4 => (ob[no - 1][i] as i32 * op255 + 255) / 256,
                        _ => op255,
                    };
                    ((ov * oa + r.max(g).max(b) * (255 - oa) + 255) / 256) as u8
                };
                let (h, s) = rgb_to_hs(r, g, b, false);
                let rgb = hsv_to_rgb(h, s, value);
                let alpha = if nc == 4 { cb[3][i] } else { 255 };
                set(i, &[rgb[0], rgb[1], rgb[2], alpha]);
            }
        }
        BlendOperator::SrcOver if nc == 4 && no == 4 => {
            // GDAL's dataset-level RGBA path, dividing by a table of reciprocals.
            for i in 0..n {
                let oa = (ob[3][i] as u32 * opacity as u32).div_ceil(256) as u8;
                let sa = (cb[3][i] as u32 * (255 - oa as u32)).div_ceil(256) as u8;
                let da = oa.wrapping_add(sa);
                let inv = if da == 0 { 0 } else { ((255u32 << 8) + da as u32 / 2) / da as u32 };
                let comp = |k: usize| {
                    let v = (ob[k][i] as u32 * oa as u32 + cb[k][i] as u32 * sa as u32).div_ceil(256) as u8;
                    ((v as u32 * inv + 255) >> 8) as u8
                };
                set(i, &[comp(0), comp(1), comp(2), da]);
            }
        }
        BlendOperator::SrcOver if nc == 3 && no == 3 => {
            for i in 0..n {
                let comp = |k: usize| ((ob[k][i] as i32 * op255 + cb[k][i] as i32 * (255 - op255) + 255) / 256) as u8;
                set(i, &[comp(0), comp(1), comp(2)]);
            }
        }
        BlendOperator::SrcOver => {
            let gray = |r: i32, g: i32, b: i32| (r * 306 + g * 601 + b * 117) / 1024;
            for i in 0..n {
                let oa = match no {
                    2 | 4 => (ob[no - 1][i] as i32 * op255 + 255) / 256,
                    _ => op255,
                };
                let sa = match nc {
                    2 | 4 => cb[nc - 1][i] as i32,
                    _ => 255,
                };
                let s_mul = (sa * (255 - oa) + 255) / 256;
                let da = oa + s_mul;
                let mut values = [0u8; 4];
                for (band, v) in values.iter_mut().enumerate().take(nc) {
                    let band = band + 1;
                    // As GDAL, the alpha of a gray and alpha input is blended as a color band.
                    if band == 4 {
                        *v = da as u8;
                        continue;
                    }
                    let src = if band <= nc { cb[band - 1][i] as i32 } else { 255 };
                    let over = if no >= 3 && nc < 3 && band <= 3 {
                        gray(ob[0][i] as i32, ob[1][i] as i32, ob[2][i] as i32)
                    } else if band <= no {
                        ob[band - 1][i] as i32
                    } else if band <= 3 {
                        ob[0][i] as i32
                    } else {
                        255
                    };
                    let mut d = (over * oa + src * s_mul + 255) / 256;
                    if da != 0 && da != 255 {
                        d = (d * 255 + da / 2) / da;
                    }
                    *v = d.min(255) as u8;
                }
                set(i, &values);
            }
        }
        _ => {
            for i in 0..n {
                let v = separable(op, px(&cb, i), px(&ob, i), opacity, swapped);
                set(i, &v);
            }
        }
    }
    let mut ds = super::derived(color, wbraster::raster::RasterData::U8(out), nc)?;
    let interp = |band: usize| match (nc, band) {
        (1 | 2, 1) => "Gray",
        (2, _) | (_, 4) => "Alpha",
        (_, 2) => "Green",
        (_, 3) => "Blue",
        _ => "Red",
    };
    ds.band_metadata = (1..=nc).map(|b| crate::BandMetadata { color_interp: Some(interp(b).into()), ..Default::default() }).collect();
    Ok(ds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{Grid, build_raster, convert_value};

    fn ds(values: &[u8]) -> RasterDataset {
        let grid = Grid { width: 1, height: 1, geo_transform: [0.0, 1.0, 0.0, 1.0, 0.0, -1.0] };
        let bands = values.iter().map(|v| vec![*v as f64]).collect();
        build_raster(grid, bands, DataType::U8, None, &Default::default(), convert_value).unwrap()
    }

    fn run(op: &str, color: &[u8], overlay: &[u8], opacity: u32) -> Vec<u8> {
        let o = BlendOptions { operator: BlendOperator::parse(op).unwrap(), opacity };
        let out = blend(&ds(color), &ds(overlay), &o).unwrap();
        (0..out.raster.bands).map(|b| out.raster.data.get_f64(b) as u8).collect()
    }

    /// Expected values from GDAL 3.13.3's autotest/utilities/test_gdalalg_raster_blend.py:
    /// `(operator, symmetric, input, overlay, expected)` at 100 % opacity.
    type Case = (&'static str, bool, &'static [u8], &'static [u8], &'static [u8]);

    #[rustfmt::skip]
    const GDAL_CASES: &[Case] = &[
        ("multiply", true, &[128, 128, 128, 128], &[128, 128, 128, 128], &[106, 106, 106, 192]),
        ("multiply", true, &[100, 100, 100, 100], &[100, 100, 100, 100], &[90, 90, 90, 160]),
        ("multiply", true, &[255, 0, 0, 255], &[0, 255, 0, 255], &[0, 0, 0, 255]),
        ("multiply", true, &[120, 150, 180, 255], &[255, 255, 255, 255], &[120, 150, 180, 255]),
        ("multiply", true, &[100, 150, 200, 255], &[255, 255, 255, 128], &[100, 150, 200, 255]),
        ("multiply", true, &[200, 200, 200, 255], &[0, 0, 255, 128], &[100, 100, 200, 255]),
        ("multiply", true, &[128, 128, 128, 255], &[64, 64, 64, 255], &[32, 32, 32, 255]),
        ("multiply", true, &[255, 0, 0, 128], &[0, 0, 255, 128], &[85, 0, 85, 192]),
        ("multiply", true, &[0, 0, 0, 0], &[150, 75, 200, 255], &[150, 75, 200, 255]),
        ("multiply", true, &[255, 255, 255, 255], &[0, 0, 0, 25], &[230, 230, 230, 255]),
        ("multiply", true, &[128, 128, 128, 255], &[128, 128, 128, 255], &[64, 64, 64, 255]),
        ("multiply", true, &[0, 0, 0, 255], &[255, 255], &[0, 0, 0, 255]),
        ("multiply", true, &[255, 255, 255, 255], &[255, 255], &[255, 255, 255, 255]),
        ("multiply", true, &[128, 128, 128, 255], &[255, 255], &[128, 128, 128, 255]),
        ("multiply", true, &[10, 20, 30, 255], &[128, 255], &[5, 10, 15, 255]),
        ("multiply", true, &[0, 0], &[0, 0], &[0, 0]),
        ("multiply", true, &[255, 255], &[255, 255], &[255, 255]),
        ("multiply", true, &[128, 255], &[10, 255], &[5, 255]),
        ("multiply", true, &[0, 0], &[0], &[0, 255]),
        ("multiply", true, &[255, 255], &[255], &[255, 255]),
        ("multiply", true, &[128, 255], &[10], &[5, 255]),
        ("multiply", true, &[0], &[0], &[0]),
        ("multiply", true, &[255], &[255], &[255]),
        ("multiply", true, &[128], &[10], &[5]),
        ("screen", true, &[100, 100, 100, 100], &[100, 100, 100, 100], &[116, 116, 116, 160]),
        ("screen", true, &[128, 128, 128, 128], &[128, 128, 128, 128], &[148, 148, 148, 192]),
        ("screen", true, &[128, 128, 128, 255], &[128, 128, 128, 255], &[192, 192, 192, 255]),
        ("screen", true, &[150, 150, 150, 255], &[0, 0, 0, 255], &[150, 150, 150, 255]),
        ("screen", true, &[150, 150, 150, 255], &[255, 255, 255, 255], &[255, 255, 255, 255]),
        ("overlay", false, &[100, 100, 100, 100], &[100, 100, 100, 100], &[102, 102, 102, 160]),
        ("overlay", false, &[128, 128, 128, 128], &[128, 128, 128, 128], &[127, 127, 127, 192]),
        ("overlay", false, &[200, 200, 200, 255], &[50, 50, 50, 255], &[165, 165, 165, 255]),
        ("overlay", false, &[50, 50, 50, 255], &[200, 200, 200, 255], &[80, 80, 80, 255]),
        ("overlay", false, &[0, 0, 0, 0], &[150, 100, 50, 255], &[150, 100, 50, 255]),
        ("overlay", false, &[150, 100, 50, 255], &[0, 0, 0, 0], &[150, 100, 50, 255]),
        ("overlay", false, &[100, 100, 100, 128], &[200, 200, 200, 128], &[152, 152, 152, 192]),
        ("hard-light", false, &[10, 20, 30, 255], &[128, 128, 128, 255], &[11, 21, 31, 255]),
        ("hard-light", false, &[128, 128, 128, 255], &[10, 20, 30, 255], &[10, 20, 30, 255]),
        ("lighten", true, &[100, 100, 100, 128], &[200, 200, 200, 128], &[166, 166, 166, 192]),
        ("lighten", true, &[128, 128, 128, 128], &[128, 128, 128, 128], &[127, 127, 127, 192]),
        ("lighten", true, &[50, 50, 50, 255], &[200, 200, 200, 255], &[200, 200, 200, 255]),
        ("lighten", true, &[200, 200, 200, 255], &[50, 50, 50, 255], &[200, 200, 200, 255]),
        ("lighten", true, &[0, 0, 255, 128], &[255, 0, 0, 128], &[170, 0, 170, 192]),
        ("lighten", true, &[0, 0, 0, 0], &[100, 150, 200, 255], &[100, 150, 200, 255]),
        ("lighten", true, &[100, 150, 200, 255], &[0, 0, 0, 0], &[100, 150, 200, 255]),
        ("lighten", true, &[100, 255], &[200, 255], &[200, 255]),
        ("lighten", true, &[100, 255], &[200, 128], &[150, 255]),
        ("lighten", true, &[100, 128], &[200, 128], &[166, 192]),
        ("darken", false, &[100, 100, 100, 100], &[100, 100, 100, 100], &[105, 105, 105, 160]),
        ("darken", false, &[128, 128, 128, 128], &[128, 128, 128, 128], &[127, 127, 127, 192]),
        ("darken", false, &[200, 200, 200, 255], &[50, 50, 50, 255], &[50, 50, 50, 255]),
        ("darken", false, &[50, 50, 50, 255], &[200, 200, 200, 255], &[50, 50, 50, 255]),
        ("darken", false, &[255, 0, 0, 128], &[0, 0, 255, 128], &[85, 0, 85, 192]),
        ("darken", false, &[0, 0, 0, 0], &[100, 150, 200, 255], &[100, 150, 200, 255]),
        ("darken", false, &[100, 150, 200, 255], &[0, 0, 0, 0], &[100, 150, 200, 255]),
        ("darken", false, &[100, 100, 100, 50], &[150, 150, 150, 200], &[140, 140, 140, 210]),
        ("darken", true, &[100, 255], &[200, 255], &[100, 255]),
        ("darken", true, &[100, 255], &[200, 128], &[100, 255]),
        ("darken", true, &[100, 128], &[200, 128], &[132, 192]),
        ("color-dodge", false, &[100, 100, 100, 100], &[100, 100, 100, 100], &[121, 121, 121, 160]),
        ("color-dodge", false, &[128, 128, 128, 128], &[128, 128, 128, 128], &[170, 170, 170, 192]),
        ("color-dodge", false, &[150, 150, 150, 255], &[0, 0, 0, 255], &[150, 150, 150, 255]),
        ("color-dodge", false, &[150, 150, 150, 255], &[255, 255, 255, 255], &[255, 255, 255, 255]),
        ("color-dodge", false, &[100, 100, 100, 255], &[0, 0, 255, 128], &[100, 100, 178, 255]),
        ("color-dodge", false, &[0, 0, 0, 0], &[100, 200, 50, 255], &[100, 200, 50, 255]),
        ("color-dodge", false, &[100, 150, 200, 255], &[0, 0, 0, 0], &[100, 150, 200, 255]),
        ("color-burn", false, &[100, 100, 100, 100], &[100, 100, 100, 100], &[79, 79, 79, 160]),
        ("color-burn", false, &[128, 128, 128, 128], &[128, 128, 128, 128], &[85, 85, 85, 192]),
        ("color-burn", false, &[150, 150, 150, 255], &[255, 255, 255, 255], &[150, 150, 150, 255]),
        ("color-burn", false, &[150, 150, 150, 255], &[0, 0, 0, 255], &[0, 0, 0, 255]),
        ("color-burn", false, &[100, 100, 100, 128], &[200, 200, 200, 128], &[114, 114, 114, 192]),
        ("color-burn", false, &[0, 0, 0, 0], &[100, 150, 200, 255], &[100, 150, 200, 255]),
        ("color-burn", false, &[100, 150, 200, 255], &[0, 0, 0, 0], &[100, 150, 200, 255]),
        ("color-burn", false, &[102, 153, 204, 255], &[0, 0, 0, 255], &[0, 0, 0, 255]),
    ];

    #[test]
    fn matches_gdal_autotest_values() {
        for (op, symmetric, color, overlay, expected) in GDAL_CASES {
            assert_eq!(run(op, color, overlay, 100), *expected, "{op} {color:?} over {overlay:?}");
            if *symmetric {
                assert_eq!(run(op, overlay, color, 100), *expected, "{op} {overlay:?} over {color:?}");
            }
        }
    }

    #[test]
    fn src_over_matches_gdal_autotest_values() {
        // GDAL 3.13.3's test_gdalalg_raster_blend_src_over.
        let (rgba, over) = ([100, 150, 200, 250], [205, 220, 240, 250]);
        assert_eq!(run("src-over", &rgba[..3], &over[..3], 100), over[..3]);
        assert_eq!(run("src-over", &rgba[..3], &over[..3], 0), rgba[..3]);
        let expected: Vec<u8> = (0..3).map(|k| ((rgba[k] as u32 * 25 + over[k] as u32 * 75) / 100) as u8).collect();
        assert_eq!(run("src-over", &rgba[..3], &over[..3], 75), expected);
        assert_eq!(run("src-over", &rgba[..3], &over, 75), [177, 201, 229]);
        assert_eq!(run("src-over", &rgba, &over, 75), [177, 201, 229, 254]);
        assert_eq!(run("src-over", &rgba, &over[..3], 100), [205, 220, 240, 255]);
        let gray = (205 * 299 + 220 * 587 + 240 * 114) / 1000;
        assert_eq!(run("src-over", &[125], &over[..3], 100), [gray as u8]);
        assert_eq!(run("src-over", &[125], &over[..3], 75), [((gray * 75 + 125 * 25) / 100) as u8]);
        assert_eq!(run("src-over", &[125, 250], &[213, 250], 75), [190, 250]);
        assert_eq!(run("src-over", &[125, 250], &[213], 75), [191, 222]);
        assert_eq!(run("src-over", &[125], &[213, 250], 75), [189]);
        assert_eq!(
            run("src-over", &rgba[..3], &[213], 75),
            [(100 * 25 + 213 * 75) / 100, (150 * 25 + 213 * 75) / 100, (200 * 25 + 213 * 75) / 100].map(|v: u32| v as u8)
        );
    }

    #[test]
    fn hsv_value_keeps_hue_and_saturation() {
        // Pure red at value 128 stays red; gray inputs take the overlay's value.
        assert_eq!(run("hsv-value", &[255, 0, 0], &[128], 100), [128, 0, 0]);
        assert_eq!(run("hsv-value", &[50, 50, 50, 200], &[90], 100), [90, 90, 90, 200]);
        assert!(blend(&ds(&[1, 2]), &ds(&[3]), &BlendOptions { operator: BlendOperator::HsvValue, opacity: 100 }).is_err());
        assert!(blend(&ds(&[1, 2, 3]), &ds(&[3]), &BlendOptions { operator: BlendOperator::Darken, opacity: 100 }).is_err());
        assert!(BlendOperator::parse("xor").is_err());
    }

    #[test]
    fn palettes_expand_to_rgba() {
        // GDAL 3.13.3's test_gdalalg_raster_blend_multiply_1band_paletted_over_1band.
        let mut paletted = ds(&[2]);
        paletted.color_table = vec![[0, 0, 0, 255], [255, 255, 255, 255], [10, 20, 30, 255]];
        let o = BlendOptions { operator: BlendOperator::Multiply, opacity: 100 };
        for out in [blend(&paletted, &ds(&[128]), &o).unwrap(), blend(&ds(&[128]), &paletted, &o).unwrap()] {
            assert_eq!((0..4).map(|b| out.raster.data.get_f64(b) as u8).collect::<Vec<_>>(), [5, 10, 15, 255]);
        }
    }
}
