//! Color tables: [`rgb_to_palette`] (`gdal raster rgb-to-palette`, `rgb2pct.py`), [`pct2rgb`]
//! (`pct2rgb.py`) and [`attach_pct`] (`gdalattachpct.py`), with GDAL's median cut
//! ([`median_cut`], `GDALComputeMedianCutPCT`) and Floyd-Steinberg dithering ([`dither`],
//! `GDALDitherRGB2PCT`), and color table files ([`parse_color_table`]).
//!
//! A color table is a list of `(red, green, blue, alpha)` entries, from index 0, as
//! [`RasterDataset::color_table`] holds them.

use crate::{Error, RasterDataset, Result};
use wbraster::DataType;

/// A color table: `(red, green, blue, alpha)` entries from index 0.
pub type ColorTable = Vec<[u8; 4]>;

/// C's `atoi()`: the leading integer, 0 when there is none.
fn atoi(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    let v = digits[..end].parse::<i64>().unwrap_or(0);
    if neg { -v } else { v }
}

/// Reads a color table from the text of a color file, as GDAL reads GMT and GRASS color files:
/// lines of `index red green blue [alpha]` (or `index colorname [alpha]`), separated by spaces,
/// tabs, commas, colons or slashes; `#` and `/` lines are comments; `nv` (nodata) entries are
/// skipped. Indices must be integers from 0 to 65535.
///
/// `fill` follows GDAL's Python utilities (`gdalattachpct.py`, `pct2rgb.py`): the table has the
/// 256 entries 0 to 255, each index without a line taking the color of the line before (the
/// first line's color below it). Otherwise entries are set at their indices, as
/// `gdal raster rgb-to-palette` loads them, those in between being transparent black.
pub fn parse_color_table(text: &str, fill: bool) -> Result<ColorTable> {
    let mut entries: Vec<(i64, [u8; 4])> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') || line.starts_with('/') {
            continue;
        }
        let fields: Vec<&str> = line.split([' ', ',', '\t', ':', '/']).filter(|f| !f.is_empty()).collect();
        if fields.len() < 2 {
            continue;
        }
        if fields[0].eq_ignore_ascii_case("nv") {
            continue;
        }
        if fields[0].len() > 1 && fields[0].ends_with('%') {
            return Err(Error::Unsupported("percentages in a color table file".into()));
        }
        let value = super::dem::atof(fields[0]);
        if !(0.0..=65536.0).contains(&value) || value.fract() != 0.0 {
            return Err(Error::InvalidArgument(format!(
                "unsupported value '{}' for a color entry: only integer values in [0, 65535] are supported",
                fields[0]
            )));
        }
        let color = if fields.len() >= 4 {
            let alpha = fields.get(4).map(|a| atoi(a)).unwrap_or(255);
            [atoi(fields[1]), atoi(fields[2]), atoi(fields[3]), alpha]
        } else {
            let (r, g, b) =
                super::dem::named_color(fields[1]).ok_or_else(|| Error::InvalidArgument(format!("unknown color: {}", fields[1])))?;
            let alpha = fields.get(2).map(|a| atoi(a)).unwrap_or(255);
            [r as i64, g as i64, b as i64, alpha]
        };
        entries.push((value as i64, color.map(|c| c as u8)));
    }
    if entries.is_empty() {
        return Err(Error::InvalidArgument("no color entry found".into()));
    }
    let mut table = ColorTable::new();
    if fill {
        let mut sorted = entries.clone();
        sorted.sort_by_key(|e| e.0);
        let mut color = sorted[0].1;
        for key in 0..=255i64 {
            // The last line of an index wins, as in a dictionary.
            if let Some(e) = entries.iter().rev().find(|e| e.0 == key) {
                color = e.1;
            }
            table.push(color);
        }
    } else {
        for (index, color) in entries {
            let i = index as usize;
            if table.len() <= i {
                table.resize(i + 1, [0, 0, 0, 0]);
            }
            table[i] = color;
        }
    }
    Ok(table)
}

/// The color table of a file: a raster's (the first band's palette), or a color file's (see
/// [`parse_color_table`], with `fill` as there). Files with a text extension (`.txt`, `.gmt`,
/// `.cpt`, `.clr`, `.pal`) and files no raster driver reads are read as color files.
pub fn load_color_table(path: &str, fill: bool) -> Result<ColorTable> {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    if !matches!(ext.as_str(), "txt" | "gmt" | "cpt" | "clr" | "pal")
        && let Ok(ds) = crate::open_raster(path)
    {
        if ds.color_table.is_empty() {
            return Err(Error::InvalidArgument(format!("'{path}' does not contain a color table")));
        }
        return Ok(ds.color_table);
    }
    let local = crate::vfs::resolve(path)?;
    let bytes = crate::memfs::read(local.path()).map_err(|e| Error::InvalidArgument(format!("cannot read '{path}': {e}")))?;
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(Error::InvalidArgument(format!("color files must be UTF-8 encoded, not UTF-16: {path}")));
    }
    parse_color_table(&String::from_utf8_lossy(&bytes), fill)
}

/// A box of the color cube, as GDAL's median cut keeps them.
#[derive(Debug, Clone, Copy)]
struct ColorBox {
    rmin: i32,
    rmax: i32,
    gmin: i32,
    gmax: i32,
    bmin: i32,
    bmax: i32,
    total: u64,
}

/// The pixel histogram of the color cube, `levels` values per axis.
struct Histogram {
    counts: Vec<u32>,
    levels: usize,
}

impl Histogram {
    #[inline]
    fn get(&self, r: i32, g: i32, b: i32) -> u32 {
        self.counts[(r as usize * self.levels + g as usize) * self.levels + b as usize]
    }
}

/// Shrinks a box to the colors it holds, scanning as GDAL's `shrinkbox` does.
fn shrink(b: &mut ColorBox, h: &Histogram) {
    if b.rmax > b.rmin {
        'rmin: for ir in b.rmin..=b.rmax {
            for ig in b.gmin..=b.gmax {
                for ib in b.bmin..=b.bmax {
                    if h.get(ir, ig, ib) != 0 {
                        b.rmin = ir;
                        break 'rmin;
                    }
                }
            }
        }
    }
    if b.rmax > b.rmin {
        'rmax: for ir in (b.rmin..=b.rmax).rev() {
            for ig in b.gmin..=b.gmax {
                for ib in b.bmin..=b.bmax {
                    if h.get(ir, ig, ib) != 0 {
                        b.rmax = ir;
                        break 'rmax;
                    }
                }
            }
        }
    }
    if b.gmax > b.gmin {
        'gmin: for ig in b.gmin..=b.gmax {
            for ir in b.rmin..=b.rmax {
                for ib in b.bmin..=b.bmax {
                    if h.get(ir, ig, ib) != 0 {
                        b.gmin = ig;
                        break 'gmin;
                    }
                }
            }
        }
    }
    if b.gmax > b.gmin {
        'gmax: for ig in (b.gmin..=b.gmax).rev() {
            for ir in b.rmin..=b.rmax {
                for ib in b.bmin..=b.bmax {
                    if h.get(ir, ig, ib) != 0 {
                        b.gmax = ig;
                        break 'gmax;
                    }
                }
            }
        }
    }
    if b.bmax > b.bmin {
        'bmin: for ib in b.bmin..=b.bmax {
            for ir in b.rmin..=b.rmax {
                for ig in b.gmin..=b.gmax {
                    if h.get(ir, ig, ib) != 0 {
                        b.bmin = ib;
                        break 'bmin;
                    }
                }
            }
        }
    }
    if b.bmax > b.bmin {
        'bmax: for ib in (b.bmin..=b.bmax).rev() {
            for ir in b.rmin..=b.rmax {
                for ig in b.gmin..=b.gmax {
                    if h.get(ir, ig, ib) != 0 {
                        b.bmax = ib;
                        break 'bmax;
                    }
                }
            }
        }
    }
}

/// Splits a box at the median of its longest axis, as GDAL's `splitbox` does: the box keeps the
/// upper part, the new box (returned) the lower part.
fn split(b: &mut ColorBox, h: &Histogram) -> ColorBox {
    #[derive(PartialEq)]
    enum Axis {
        R,
        G,
        B,
    }
    let dr = b.rmax - b.rmin;
    let axis = if dr >= b.gmax - b.gmin && dr >= b.bmax - b.bmin {
        Axis::R
    } else if b.gmax - b.gmin >= b.bmax - b.bmin {
        Axis::G
    } else {
        Axis::B
    };
    let mut hist2 = [0u64; 256];
    let (first, last) = match axis {
        Axis::R => {
            for ir in b.rmin..=b.rmax {
                let mut s = 0u64;
                for ig in b.gmin..=b.gmax {
                    for ib in b.bmin..=b.bmax {
                        s += h.get(ir, ig, ib) as u64;
                    }
                }
                hist2[ir as usize] = s;
            }
            (b.rmin, b.rmax)
        }
        Axis::G => {
            for ig in b.gmin..=b.gmax {
                let mut s = 0u64;
                for ir in b.rmin..=b.rmax {
                    for ib in b.bmin..=b.bmax {
                        s += h.get(ir, ig, ib) as u64;
                    }
                }
                hist2[ig as usize] = s;
            }
            (b.gmin, b.gmax)
        }
        Axis::B => {
            for ib in b.bmin..=b.bmax {
                let mut s = 0u64;
                for ir in b.rmin..=b.rmax {
                    for ig in b.gmin..=b.gmax {
                        s += h.get(ir, ig, ib) as u64;
                    }
                }
                hist2[ib as usize] = s;
            }
            (b.bmin, b.bmax)
        }
    };
    // The median: the first value where the running sum reaches half the box's total.
    let half = b.total / 2;
    let mut i = first;
    let mut sum = 0u64;
    while i <= last {
        sum += hist2[i as usize];
        if sum >= half {
            break;
        }
        i += 1;
    }
    if i == first {
        i += 1;
    }
    let lower: u64 = (first..i).map(|j| hist2[j as usize]).sum();
    let upper: u64 = (i..=last).map(|j| hist2[j as usize]).sum();
    let mut new = ColorBox { total: lower, ..*b };
    b.total = upper;
    match axis {
        Axis::R => {
            new.rmax = i - 1;
            b.rmin = i;
        }
        Axis::G => {
            new.gmax = i - 1;
            b.gmin = i;
        }
        Axis::B => {
            new.bmax = i - 1;
            b.bmin = i;
        }
    }
    shrink(&mut new, h);
    shrink(b, h);
    new
}

/// Computes a color table of at most `colors` entries for RGB pixels with GDAL's median cut
/// (`GDALComputeMedianCutPCT`), on a histogram of `bits`-bit components (5 by default in GDAL,
/// or 8). Pixels where `valid` is false are left out of the histogram (but not of the boxes'
/// pixel counts, as in GDAL). With 8 bits and no more distinct colors than `colors`, the table
/// is those colors, in the order they appear.
pub fn median_cut(red: &[u8], green: &[u8], blue: &[u8], valid: Option<&[bool]>, colors: usize, bits: u32) -> Result<ColorTable> {
    if !(1..=256).contains(&colors) {
        return Err(Error::InvalidArgument("the color count must be from 1 to 256".into()));
    }
    if !(1..=8).contains(&bits) {
        return Err(Error::InvalidArgument("the component bit depth must be from 1 to 8".into()));
    }
    let n = red.len();
    if green.len() != n || blue.len() != n || valid.is_some_and(|v| v.len() != n) {
        return Err(Error::InvalidArgument("the red, green and blue bands differ in size".into()));
    }
    let levels = 1usize << bits;
    let shift = 8 - bits;
    let mut h = Histogram { counts: vec![0u32; levels * levels * levels], levels };
    let mut first = ColorBox { rmin: 999, rmax: -1, gmin: 999, gmax: -1, bmin: 999, bmax: -1, total: n as u64 };
    let mut seen: Vec<[u8; 4]> = Vec::new();
    let mut distinct = 0usize;
    for i in 0..n {
        if valid.is_some_and(|v| !v[i]) {
            continue;
        }
        let (r, g, b) = ((red[i] >> shift) as i32, (green[i] >> shift) as i32, (blue[i] >> shift) as i32);
        first.rmin = first.rmin.min(r);
        first.gmin = first.gmin.min(g);
        first.bmin = first.bmin.min(b);
        first.rmax = first.rmax.max(r);
        first.gmax = first.gmax.max(g);
        first.bmax = first.bmax.max(b);
        let c = &mut h.counts[(r as usize * levels + g as usize) * levels + b as usize];
        if *c == 0 {
            if shift == 0 && distinct < colors {
                seen.push([r as u8, g as u8, b as u8, 255]);
            }
            distinct += 1;
        }
        *c = c.saturating_add(1);
    }
    if shift == 0 && distinct <= colors {
        return Ok(seen);
    }
    // The boxes in GDAL's list order: new boxes go first.
    let mut boxes = vec![first];
    while boxes.len() < colors {
        let mut largest: Option<usize> = None;
        for (k, b) in boxes.iter().enumerate() {
            if (b.rmax > b.rmin || b.gmax > b.gmin || b.bmax > b.bmin) && largest.is_none_or(|l| b.total > boxes[l].total) {
                largest = Some(k);
            }
        }
        let Some(k) = largest else { break };
        let new = split(&mut boxes[k], &h);
        boxes.insert(0, new);
    }
    if distinct == 0 {
        return Ok(Vec::new());
    }
    Ok(boxes
        .iter()
        .map(|b| {
            let c = |lo: i32, hi: i32| (((lo + hi) << shift) / 2) as u8;
            [c(b.rmin, b.rmax), c(b.gmin, b.gmax), c(b.bmin, b.bmax), 255]
        })
        .collect())
}

/// Options of [`dither`].
#[derive(Debug, Clone)]
pub struct DitherOptions {
    /// The component bit depth of the color lookup cube (5 in GDAL by default, or 8).
    pub bits: u32,
    /// Diffuse the color errors (Floyd-Steinberg); otherwise take the nearest color.
    pub dither: bool,
    /// The output index for invalid pixels, which no valid pixel takes.
    pub nodata: Option<u8>,
}

impl Default for DitherOptions {
    fn default() -> Self {
        Self { bits: 5, dither: true, nodata: None }
    }
}

/// The index of the color nearest (by summed absolute differences) to a color, the first of
/// equally near ones, never `skip`.
fn nearest(table: &[[i32; 3]], r: i32, g: i32, b: i32, skip: Option<usize>) -> usize {
    let mut best = 768;
    let mut index = 0;
    for (i, c) in table.iter().enumerate() {
        if Some(i) == skip {
            continue;
        }
        let d = (r - c[0]).abs() + (g - c[1]).abs() + (b - c[2]).abs();
        if d < best {
            best = d;
            index = i;
        }
    }
    index
}

/// Converts RGB pixels (rows of `width`) to indices of a color table with GDAL's
/// Floyd-Steinberg dithering (`GDALDitherRGB2PCT`): colors are looked up in a cube of
/// `bits`-bit components, and errors spread to the right (2/6), below-left, below and
/// below-right. With a `nodata` index, pixels where `valid` is false (before the errors are added
/// to them, and after) take it and pass no error on.
pub fn dither(
    red: &[u8],
    green: &[u8],
    blue: &[u8],
    width: usize,
    table: &[[u8; 4]],
    valid: Option<&dyn Fn(u8, u8, u8, usize) -> bool>,
    o: &DitherOptions,
) -> Result<Vec<u8>> {
    if table.is_empty() {
        return Err(Error::InvalidArgument("the color table must not be empty".into()));
    }
    if table.len() > 256 {
        return Err(Error::InvalidArgument("the color table cannot have more than 256 entries".into()));
    }
    if !(1..=8).contains(&o.bits) {
        return Err(Error::InvalidArgument("the component bit depth must be from 1 to 8".into()));
    }
    let n = red.len();
    if width == 0 || !n.is_multiple_of(width) || green.len() != n || blue.len() != n {
        return Err(Error::InvalidArgument("the red, green and blue bands differ in size".into()));
    }
    let pct: Vec<[i32; 3]> = table.iter().map(|c| [c[0] as i32, c[1] as i32, c[2] as i32]).collect();
    let skip = o.nodata.map(|v| v as usize);
    let levels = 1usize << o.bits;
    // The cube GDAL fills before dithering, filled here as its cells are needed.
    let mut cube = vec![-1i16; levels * levels * levels];
    let level_value = |i: usize| ((i * 255) / (levels - 1)) as i32;
    let mut out = vec![0u8; n];
    let mut errors = vec![0i32; (width + 2) * 3];
    let (mut r_line, mut g_line, mut b_line) = (vec![0u8; width], vec![0u8; width], vec![0u8; width]);
    let invalid = |r: u8, g: u8, b: u8, i: usize| o.nodata.is_some() && valid.is_some_and(|f| !f(r, g, b, i));
    for (row, line) in out.chunks_mut(width).enumerate() {
        let start = row * width;
        r_line.copy_from_slice(&red[start..start + width]);
        g_line.copy_from_slice(&green[start..start + width]);
        b_line.copy_from_slice(&blue[start..start + width]);
        if o.dither {
            for i in 0..width {
                if invalid(r_line[i], g_line[i], b_line[i], start + i) {
                    continue;
                }
                r_line[i] = (r_line[i] as i32 + errors[i * 3 + 3]).clamp(0, 255) as u8;
                g_line[i] = (g_line[i] as i32 + errors[i * 3 + 4]).clamp(0, 255) as u8;
                b_line[i] = (b_line[i] as i32 + errors[i * 3 + 5]).clamp(0, 255) as u8;
            }
            errors.iter_mut().for_each(|e| *e = 0);
        }
        let (mut last_r, mut last_g, mut last_b) = (0i32, 0i32, 0i32);
        for i in 0..width {
            if invalid(r_line[i], g_line[i], b_line[i], start + i) {
                (last_r, last_g, last_b) = (0, 0, 0);
                line[i] = o.nodata.unwrap_or(0);
                continue;
            }
            let rv = (r_line[i] as i32 + last_r).clamp(0, 255);
            let gv = (g_line[i] as i32 + last_g).clamp(0, 255);
            let bv = (b_line[i] as i32 + last_b).clamp(0, 255);
            let (ir, ig, ib) = (rv as usize * levels / 256, gv as usize * levels / 256, bv as usize * levels / 256);
            let cell = &mut cube[ir + ig * levels + ib * levels * levels];
            if *cell < 0 {
                *cell = nearest(&pct, level_value(ir), level_value(ig), level_value(ib), skip) as i16;
            }
            let index = *cell as usize;
            line[i] = index as u8;
            if !o.dither {
                continue;
            }
            let c = pct[index];
            for (k, (v, last)) in [(rv, &mut last_r), (gv, &mut last_g), (bv, &mut last_b)].into_iter().enumerate() {
                let e = v - c[k];
                let sixth = e / 6;
                errors[i * 3 + k] += sixth;
                errors[i * 3 + 6 + k] = sixth;
                errors[i * 3 + 3 + k] += e - 5 * sixth;
                *last = 2 * sixth;
            }
        }
    }
    Ok(out)
}

/// Options of [`rgb_to_palette`], named after `gdal raster rgb-to-palette`'s.
#[derive(Debug, Clone)]
pub struct RgbToPaletteOptions {
    /// The number of colors of the computed table (`--color-count`, 2 to 256).
    pub color_count: usize,
    /// A color table to use instead of computing one (`--color-map`).
    pub color_table: Option<ColorTable>,
    /// The output index of nodata pixels (`--output-nodata`). By default 0 when the red, green
    /// and blue bands share a Byte nodata value or the raster has an alpha band and no color
    /// table is given (GDAL 3.13), none otherwise.
    pub output_nodata: Option<u8>,
    /// Diffuse the color errors (Floyd-Steinberg dithering); `--no-dither` turns it off.
    pub dither: bool,
    /// The component bit depth of the histogram and color lookups (`--bit-depth`, 5 or 8).
    pub bit_depth: u32,
}

impl Default for RgbToPaletteOptions {
    fn default() -> Self {
        Self { color_count: 256, color_table: None, output_nodata: None, dither: true, bit_depth: 5 }
    }
}

/// The red, green and blue bands (from 0) of a raster, as GDAL picks them: by color
/// interpretation, or the first three.
fn rgb_bands(ds: &RasterDataset) -> Result<[usize; 3]> {
    let n = ds.raster.bands;
    let mut found = [None, None, None];
    for b in 1..=n {
        let ci = ds.band_color_interp(b);
        for (k, name) in ["Red", "Green", "Blue"].iter().enumerate() {
            if ci.eq_ignore_ascii_case(name) {
                if found[k].is_some() {
                    return Err(Error::Unsupported(format!("several {name} bands found")));
                }
                found[k] = Some(b - 1);
            }
        }
    }
    Ok(match found {
        [Some(r), Some(g), Some(b)] => [r, g, b],
        _ => [0, 1, 2],
    })
}

/// Converts an RGB raster to a paletted one, as `gdal raster rgb-to-palette` does (and
/// `rgb2pct.py` with its defaults): a color table from [`median_cut`] (or given), then
/// [`dither`]. The input needs at least three Byte bands; the output is one Byte band with the
/// color table and the input's georeferencing and metadata.
pub fn rgb_to_palette(ds: &RasterDataset, o: &RgbToPaletteOptions) -> Result<RasterDataset> {
    let r = &ds.raster;
    if r.bands < 3 || ds.complex {
        return Err(Error::Unsupported("the input raster must have at least 3 bands".into()));
    }
    if r.data_type != DataType::U8 {
        return Err(Error::Unsupported("only Byte bands are supported".into()));
    }
    if !(2..=256).contains(&o.color_count) {
        return Err(Error::InvalidArgument("the color count must be from 2 to 256".into()));
    }
    if !matches!(o.bit_depth, 5 | 8) {
        return Err(Error::InvalidArgument("the bit depth must be 5 or 8".into()));
    }
    let [ri, gi, bi] = rgb_bands(ds)?;
    let n = r.cols * r.rows;
    let band = |b: usize| -> &[u8] {
        let wbraster::raster::RasterData::U8(v) = &r.data else { unreachable!() };
        &v[b * n..(b + 1) * n]
    };
    let (red, green, blue) = (band(ri), band(gi), band(bi));
    // The source's nodata value (shared by the three bands) or its alpha band, as GDAL's mask.
    let shared = [ri, gi, bi].map(|b| ds.band_nodata(b + 1));
    let src_nodata = match shared {
        [Some(a), Some(b), Some(c)] if a == b && a == c && (0.0..=255.0).contains(&a) && a.fract() == 0.0 => Some(a as u8),
        _ => None,
    };
    let alpha = (src_nodata.is_none() && r.bands == 4 && ds.band_color_interp(4).eq_ignore_ascii_case("Alpha")).then(|| band(3));
    let has_mask = src_nodata.is_some() || alpha.is_some();
    let valid_at = |rv: u8, gv: u8, bv: u8, i: usize| match (src_nodata, alpha) {
        (Some(nd), _) => !(rv == nd && gv == nd && bv == nd),
        (None, Some(a)) => a[i] != 0,
        _ => true,
    };
    let dst_nodata = o.output_nodata.or_else(|| (o.color_table.is_none() && has_mask).then_some(0));
    let mut table = match &o.color_table {
        Some(t) => t.clone(),
        None => {
            let count = if dst_nodata.is_some() && o.color_count == 256 { 255 } else { o.color_count };
            let valid: Option<Vec<bool>> = has_mask.then(|| (0..n).map(|i| valid_at(red[i], green[i], blue[i], i)).collect());
            median_cut(red, green, blue, valid.as_deref(), count, o.bit_depth)?
        }
    };
    if let Some(nd) = dst_nodata {
        let nd = nd as usize;
        // Entries above the nodata index move up by one, as GDAL inserts the transparent entry.
        let top = table.len().min(255);
        for i in ((nd + 1)..=top).rev() {
            let e = table[i - 1];
            if i == table.len() {
                table.push(e);
            } else {
                table[i] = e;
            }
        }
        if table.len() <= nd {
            table.resize(nd + 1, [0, 0, 0, 0]);
        }
        table[nd] = [0, 0, 0, 0];
    }
    let valid_fn: &dyn Fn(u8, u8, u8, usize) -> bool = &valid_at;
    let indices = dither(
        red,
        green,
        blue,
        r.cols,
        &table,
        has_mask.then_some(valid_fn),
        &DitherOptions { bits: o.bit_depth, dither: o.dither, nodata: dst_nodata },
    )?;
    let mut out = super::derived(ds, wbraster::raster::RasterData::U8(indices), 1)?;
    out.band_metadata = vec![crate::BandMetadata { color_interp: Some("Palette".into()), ..Default::default() }];
    out.color_table = table;
    if let Some(nd) = dst_nodata {
        out.has_nodata = true;
        out.raster.nodata = nd as f64;
    }
    Ok(out)
}

/// Options of [`pct2rgb`], named after `pct2rgb.py`'s.
#[derive(Debug, Clone)]
pub struct Pct2RgbOptions {
    /// The paletted band, from 1 (`-b`).
    pub band: usize,
    /// Write a fourth, alpha band from the entries' alpha (`-rgba`).
    pub rgba: bool,
    /// A color table to use instead of the raster's (`-pct`).
    pub color_table: Option<ColorTable>,
}

impl Default for Pct2RgbOptions {
    fn default() -> Self {
        Self { band: 1, rgba: false, color_table: None }
    }
}

/// Expands a paletted band to red, green and blue (and alpha) Byte bands, as `pct2rgb.py` does
/// (`gdal_translate -expand rgb|rgba`). The output has the input's georeferencing and no nodata
/// value; an index outside the color table is an error.
pub fn pct2rgb(ds: &RasterDataset, o: &Pct2RgbOptions) -> Result<RasterDataset> {
    let r = &ds.raster;
    if o.band == 0 || o.band > ds.band_count() {
        return Err(Error::InvalidArgument(format!("the raster has no band {}", o.band)));
    }
    let table = match &o.color_table {
        Some(t) => t.as_slice(),
        None if o.band == 1 && !ds.color_table.is_empty() => ds.color_table.as_slice(),
        None => return Err(Error::InvalidArgument(format!("band {} has no color table", o.band))),
    };
    let view = super::BandView::of(r, o.band - 1);
    let n = r.cols * r.rows;
    let components = if o.rgba { 4 } else { 3 };
    let mut data = vec![0u8; n * components];
    for i in 0..n {
        let v = view.get(i);
        let entry = (v >= 0.0 && v.fract() == 0.0).then(|| table.get(v as usize)).flatten();
        let Some(e) = entry else {
            return Err(Error::InvalidArgument(format!("value {v} is outside the color table of {} entries", table.len())));
        };
        for c in 0..components {
            data[c * n + i] = e[c];
        }
    }
    let mut out = super::derived(ds, wbraster::raster::RasterData::U8(data), components)?;
    let names = ["Red", "Green", "Blue", "Alpha"];
    out.band_metadata =
        (0..components).map(|c| crate::BandMetadata { color_interp: Some(names[c].into()), ..Default::default() }).collect();
    Ok(out)
}

/// Attaches a color table to a raster's single Byte or UInt16 band, as `gdalattachpct.py`
/// does: the pixels, georeferencing, nodata and metadata are kept, and the band becomes
/// paletted.
pub fn attach_pct(ds: &RasterDataset, table: &[[u8; 4]]) -> Result<RasterDataset> {
    if table.is_empty() {
        return Err(Error::InvalidArgument("the color table is empty".into()));
    }
    if ds.raster.bands != 1 || ds.complex || !matches!(ds.raster.data_type, DataType::U8 | DataType::U16) {
        return Err(Error::Unsupported("a color table needs a single Byte or UInt16 band".into()));
    }
    let max = if ds.raster.data_type == DataType::U8 { 256 } else { 65536 };
    if table.len() > max {
        return Err(Error::InvalidArgument(format!("{} entries for a band of at most {max} values", table.len())));
    }
    let mut out = ds.clone();
    out.color_table = table.to_vec();
    let mut meta = out.band_metadata.first().cloned().unwrap_or_default();
    meta.color_interp = Some("Palette".into());
    out.band_metadata = vec![meta];
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_color_files() {
        let text = "# palette\n0 0 0 0\n2,255,0,0,128\n5 white\nnv 1 2 3\n";
        let t = parse_color_table(text, false).unwrap();
        // Four fields: an opaque color; gaps are transparent black.
        assert_eq!(t, vec![[0, 0, 0, 255], [0, 0, 0, 0], [255, 0, 0, 128], [0, 0, 0, 0], [0, 0, 0, 0], [255, 255, 255, 255]]);
        let filled = parse_color_table("3 10 20 30\n5 1 2 3\n", true).unwrap();
        assert_eq!(filled.len(), 256);
        assert_eq!((filled[0], filled[4], filled[5], filled[255]), ([10, 20, 30, 255], [10, 20, 30, 255], [1, 2, 3, 255], [1, 2, 3, 255]));
        assert!(parse_color_table("1.5 0 0 0\n", false).is_err());
        assert!(parse_color_table("10% 0 0 0\n", false).is_err());
        assert!(parse_color_table("# nothing\n", false).is_err());
    }

    #[test]
    fn median_cut_keeps_few_colors_and_splits_many() {
        let (r, g, b) = (vec![10, 200, 10, 30], vec![20, 100, 20, 40], vec![30, 0, 30, 50]);
        // 8-bit components, no more colors than asked: those colors, in order.
        let t = median_cut(&r, &g, &b, None, 4, 8).unwrap();
        assert_eq!(t, vec![[10, 20, 30, 255], [200, 100, 0, 255], [30, 40, 50, 255]]);
        // A gradient cut into 2 boxes of 5-bit components, as GDAL's ComputeMedianCutPCT cuts it.
        let v: Vec<u8> = (0..=255).collect();
        let t = median_cut(&v, &v, &v, None, 2, 5).unwrap();
        assert_eq!(t, vec![[56, 56, 56, 255], [184, 184, 184, 255]]);
        let valid = [true, false, true, false];
        assert_eq!(median_cut(&r, &g, &b, Some(&valid), 4, 8).unwrap(), vec![[10, 20, 30, 255]]);
    }

    #[test]
    fn dithers_like_gdal() {
        let table = [[0, 0, 0, 255], [255, 255, 255, 255]];
        let v = vec![128u8; 8];
        // Nearest color only: mid gray goes to white (distance 381 vs 384).
        let o = DitherOptions { dither: false, ..Default::default() };
        assert_eq!(dither(&v, &v, &v, 4, &table, None, &o).unwrap(), vec![1; 8]);
        // Error diffusion alternates.
        let out = dither(&v, &v, &v, 4, &table, None, &Default::default()).unwrap();
        assert!(out.contains(&0) && out.contains(&1));
        let nodata = DitherOptions { nodata: Some(1), ..Default::default() };
        let all: &dyn Fn(u8, u8, u8, usize) -> bool = &|_, _, _, i| i != 0;
        let out = dither(&v, &v, &v, 4, &table, Some(all), &nodata).unwrap();
        assert_eq!(out[0], 1);
        assert!(out[1..].iter().all(|i| *i == 0));
    }

    #[test]
    fn expands_and_attaches_tables() {
        use crate::ops::{Grid, build_raster, convert_value};
        let grid = Grid { width: 3, height: 1, geo_transform: [0.0, 1.0, 0.0, 1.0, 0.0, -1.0] };
        let ds = build_raster(grid, vec![vec![0.0, 1.0, 2.0]], DataType::U8, None, &Default::default(), convert_value).unwrap();
        let table = vec![[1, 2, 3, 255], [4, 5, 6, 0], [7, 8, 9, 128]];
        let paletted = attach_pct(&ds, &table).unwrap();
        assert_eq!(paletted.band_color_interp(1), "Palette");
        let rgb = pct2rgb(&paletted, &Default::default()).unwrap();
        assert_eq!(rgb.raster.bands, 3);
        assert_eq!((0..9).map(|i| rgb.raster.data.get_f64(i) as u8).collect::<Vec<_>>(), [1, 4, 7, 2, 5, 8, 3, 6, 9]);
        let rgba = pct2rgb(&paletted, &Pct2RgbOptions { rgba: true, ..Default::default() }).unwrap();
        assert_eq!((9..12).map(|i| rgba.raster.data.get_f64(i) as u8).collect::<Vec<_>>(), [255, 0, 128]);
        assert!(pct2rgb(&ds, &Default::default()).is_err());
        // GeoTIFFs keep the table (GDAL's reading: 256 entries, opaque), read whole or by window.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.tif");
        crate::write_raster_dataset(&paletted, path.to_str().unwrap(), &Default::default()).unwrap();
        let back = crate::open_raster(path.to_str().unwrap()).unwrap();
        assert_eq!(back.color_table.len(), 256);
        assert_eq!(back.color_table[..3], [[1, 2, 3, 255], [4, 5, 6, 255], [7, 8, 9, 255]]);
        let mut lazy = crate::open_raster_lazy(path.to_str().unwrap()).unwrap();
        assert_eq!(lazy.header().color_table.len(), 256);
        let w = lazy.read_window(crate::Window::new(1, 0, 2, 1), &Default::default()).unwrap();
        assert_eq!(w.color_table, back.color_table);
        let bytes = crate::memory::write_raster_bytes(&paletted, "p.tif", &Default::default()).unwrap();
        assert_eq!(crate::memory::open_raster_bytes(bytes, "p.tif").unwrap().color_table.len(), 256);
        let short = Pct2RgbOptions { color_table: Some(vec![[0, 0, 0, 255]]), ..Default::default() };
        assert!(pct2rgb(&ds, &short).unwrap_err().to_string().contains("outside"));
    }
}
