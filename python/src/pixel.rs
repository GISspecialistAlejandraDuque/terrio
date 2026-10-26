//! Pixel-wise raster operations: `unscale`, `nodata_to_alpha`, `reclassify`, `as_features`,
//! `blend`, `rgb_to_palette`, `pct2rgb` and `attach_pct`, with keyword arguments named after the
//! command-line options.

use crate::PyResultExt;
use crate::invalid;
use crate::ops::{finish_raster, input_raster, vector_write_options, write_mode};
use crate::raster::PyRaster;
use crate::vector::PyLayer;
use pyo3::prelude::*;
use terrio::ops::as_features::{AsFeaturesOptions, PixelGeometry};
use terrio::ops::parse_data_type;
use terrio::ops::pixel::{NodataToAlphaOptions, ReclassifyOptions, UnscaleOptions, mapping_from_file_text};

/// Registers the functions in the module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(unscale, m)?)?;
    m.add_function(wrap_pyfunction!(nodata_to_alpha, m)?)?;
    m.add_function(wrap_pyfunction!(reclassify, m)?)?;
    m.add_function(wrap_pyfunction!(as_features, m)?)?;
    m.add_function(wrap_pyfunction!(blend, m)?)?;
    m.add_function(wrap_pyfunction!(rgb_to_palette, m)?)?;
    m.add_function(wrap_pyfunction!(pct2rgb, m)?)?;
    m.add_function(wrap_pyfunction!(attach_pct, m)?)?;
    Ok(())
}

/// Applies band scales and offsets, like `terrio unscale` (GDAL's `gdal raster unscale`).
///
/// Args:
///     src: The raster: a path or a `Raster`.
///     dst: Also write the result to this path.
///     ot: The output data type; by default `"Float32"`, or `"Float64"` for Float64 sources.
///     format: The output format's short name, for `dst`.
///     compress: GeoTIFF/COG compression, for `dst`.
///     tile_size: GeoTIFF/COG tile size, for `dst`.
///     bigtiff: Write a BigTIFF, for `dst`.
///
/// Returns:
///     Raster: `value * scale + offset` per band; nodata pixels keep the nodata value.
#[pyfunction]
#[pyo3(signature = (src, dst=None, *, ot=None, format=None, compress=None, tile_size=None, bigtiff=false))]
#[allow(clippy::too_many_arguments)]
fn unscale(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    dst: Option<&str>,
    ot: Option<&str>,
    format: Option<&str>,
    compress: Option<String>,
    tile_size: Option<u32>,
    bigtiff: bool,
) -> PyResult<PyRaster> {
    let o = UnscaleOptions { output_type: ot.map(parse_data_type).transpose().py()? };
    let ds = input_raster(py, src)?;
    let out = py.detach(|| terrio::ops::pixel::unscale(&ds, &o)).py()?;
    finish_raster(py, out, dst, format, compress, tile_size, bigtiff)
}

/// Replaces nodata values with an alpha band, like `terrio nodata-to-alpha` (GDAL's
/// `gdal raster nodata-to-alpha`).
///
/// Args:
///     src: The raster: a path or a `Raster`.
///     dst: Also write the result to this path.
///     nodata: A nodata value overriding the source's, or a sequence of one per band (then a
///         pixel is transparent where every band has its value).
///     format: The output format's short name, for `dst`.
///     compress: GeoTIFF/COG compression, for `dst`.
///     tile_size: GeoTIFF/COG tile size, for `dst`.
///     bigtiff: Write a BigTIFF, for `dst`.
///
/// Returns:
///     Raster: The bands without nodata values, then an alpha band of their type (0 or 255).
#[pyfunction]
#[pyo3(signature = (src, dst=None, *, nodata=None, format=None, compress=None, tile_size=None, bigtiff=false))]
#[allow(clippy::too_many_arguments)]
fn nodata_to_alpha(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    dst: Option<&str>,
    nodata: Option<&Bound<'_, PyAny>>,
    format: Option<&str>,
    compress: Option<String>,
    tile_size: Option<u32>,
    bigtiff: bool,
) -> PyResult<PyRaster> {
    let nodata: Vec<f64> = match nodata {
        None => Vec::new(),
        Some(v) => match v.extract::<f64>() {
            Ok(x) => vec![x],
            Err(_) => v.extract().map_err(|_| invalid("nodata is a number or a sequence of numbers"))?,
        },
    };
    let o = NodataToAlphaOptions { nodata };
    let ds = input_raster(py, src)?;
    let out = py.detach(|| terrio::ops::pixel::nodata_to_alpha(&ds, &o)).py()?;
    finish_raster(py, out, dst, format, compress, tile_size, bigtiff)
}

/// Maps values or ranges of values to new values, like `terrio reclassify` (GDAL's
/// `gdal raster reclassify`).
///
/// Args:
///     src: The raster: a path or a `Raster`.
///     mapping: GDAL's mapping: `from=to` entries separated by `;`, `from` a value, an interval
///         (`"[a,b)"`, `"(a,b]"`, `"(-inf,b]"`...), `NO_DATA`, `NAN` or `DEFAULT`, `to` a value,
///         `NO_DATA` or `PASS_THROUGH`. `"@path"` reads the entries from a file, one per line.
///     dst: Also write the result to this path.
///     ot: The output data type; by default that of the source.
///     format: The output format's short name, for `dst`.
///     compress: GeoTIFF/COG compression, for `dst`.
///     tile_size: GeoTIFF/COG tile size, for `dst`.
///     bigtiff: Write a BigTIFF, for `dst`.
///
/// Returns:
///     Raster: The reclassified bands, with the source's nodata values.
#[pyfunction]
#[pyo3(signature = (src, mapping, dst=None, *, ot=None, format=None, compress=None, tile_size=None, bigtiff=false))]
#[allow(clippy::too_many_arguments)]
fn reclassify(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    mapping: &str,
    dst: Option<&str>,
    ot: Option<&str>,
    format: Option<&str>,
    compress: Option<String>,
    tile_size: Option<u32>,
    bigtiff: bool,
) -> PyResult<PyRaster> {
    let mapping = match mapping.strip_prefix('@') {
        Some(path) => {
            mapping_from_file_text(&std::fs::read_to_string(path).map_err(|e| invalid(format!("cannot open {path}: {e}")))?).py()?
        }
        None => mapping.to_string(),
    };
    let o = ReclassifyOptions { mapping, output_type: ot.map(parse_data_type).transpose().py()? };
    let ds = input_raster(py, src)?;
    let out = py.detach(|| terrio::ops::pixel::reclassify(&ds, &o)).py()?;
    finish_raster(py, out, dst, format, compress, tile_size, bigtiff)
}

/// One vector feature per pixel, like `terrio as-features` (GDAL's `gdal raster as-features`).
///
/// Args:
///     src: The raster: a path or a `Raster`.
///     dst: Also write the features to this path.
///     bands: The bands to report, numbered from 1; all by default.
///     geometry_type: `"none"` (an attribute table), `"point"` (pixel centres) or `"polygon"`
///         (pixel squares).
///     skip_nodata: Leave out the pixels whose first reported band has its nodata value.
///     include_xy: Add `CENTER_X` and `CENTER_Y` fields.
///     include_row_col: Add `ROW` and `COL` fields.
///     layer_name: The layer name.
///     format: The output format's short name, for `dst`.
///     append: Add the features to the layer of the same name in an existing output.
///     overwrite: Replace the layer of the same name in an existing output.
///
/// Returns:
///     Layer: The features, with a `BAND_<n>` field per reported band.
#[pyfunction]
#[pyo3(signature = (
    src, dst=None, *, bands=None, geometry_type="none", skip_nodata=false, include_xy=false, include_row_col=false,
    layer_name="pixels", format=None, append=false, overwrite=false
))]
#[allow(clippy::too_many_arguments)]
fn as_features(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    dst: Option<&str>,
    bands: Option<Vec<usize>>,
    geometry_type: &str,
    skip_nodata: bool,
    include_xy: bool,
    include_row_col: bool,
    layer_name: &str,
    format: Option<&str>,
    append: bool,
    overwrite: bool,
) -> PyResult<PyLayer> {
    let o = AsFeaturesOptions {
        bands: bands.unwrap_or_default(),
        geometry: PixelGeometry::parse(geometry_type).py()?,
        skip_nodata,
        include_xy,
        include_row_col,
        layer_name: layer_name.to_string(),
    };
    let ds = input_raster(py, src)?;
    let write = dst.map(|_| vector_write_options(format)).transpose()?;
    let mode = write_mode(append, overwrite)?;
    let layer = py
        .detach(|| -> terrio::Result<terrio::Layer> {
            let layer = terrio::ops::as_features::as_features(&ds, &o)?;
            if let (Some(path), Some(w)) = (dst, &write) {
                terrio::ops::vector_filter::write_vector_with_mode(std::slice::from_ref(&layer), path, w, mode)?;
            }
            Ok(layer)
        })
        .py()?;
    Ok(PyLayer { inner: layer })
}

/// A color table given to a function: a path (a raster's palette or a color file) or a sequence
/// of `(r, g, b)` or `(r, g, b, a)` entries from index 0.
fn color_table(v: &Bound<'_, PyAny>, fill: bool) -> PyResult<Vec<[u8; 4]>> {
    if let Ok(path) = v.extract::<String>() {
        return terrio::ops::palette::load_color_table(&path, fill).py();
    }
    let entries: Vec<Vec<u8>> = v.extract().map_err(|_| invalid("a color table is a path or a sequence of (r, g, b[, a]) entries"))?;
    entries
        .into_iter()
        .map(|e| match e.as_slice() {
            [r, g, b] => Ok([*r, *g, *b, 255]),
            [r, g, b, a] => Ok([*r, *g, *b, *a]),
            _ => Err(invalid("color table entries are (r, g, b) or (r, g, b, a)")),
        })
        .collect()
}

/// Blends an overlay onto a raster, like `terrio blend` (GDAL's `gdal raster blend`).
///
/// Args:
///     src: The color input (a path or a `Raster`): 1 to 4 Byte bands or a paletted band.
///     overlay: The overlay, of the same size.
///     dst: Also write the result to this path.
///     operator: `"src-over"`, `"hsv-value"`, `"multiply"`, `"screen"`, `"overlay"`,
///         `"hard-light"`, `"darken"`, `"lighten"`, `"color-burn"` or `"color-dodge"`.
///     opacity: The overlay's opacity in percent, 0 to 100.
///     format: The output format's short name, for `dst`.
///     compress: GeoTIFF/COG compression, for `dst`.
///     tile_size: GeoTIFF/COG tile size, for `dst`.
///     bigtiff: Write a BigTIFF, for `dst`.
///
/// Returns:
///     Raster: The blended Byte bands.
#[pyfunction]
#[pyo3(signature = (src, overlay, dst=None, *, operator="src-over", opacity=100, format=None, compress=None, tile_size=None, bigtiff=false))]
#[allow(clippy::too_many_arguments)]
fn blend(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    overlay: &Bound<'_, PyAny>,
    dst: Option<&str>,
    operator: &str,
    opacity: u32,
    format: Option<&str>,
    compress: Option<String>,
    tile_size: Option<u32>,
    bigtiff: bool,
) -> PyResult<PyRaster> {
    use terrio::ops::blend::{BlendOperator, BlendOptions};
    let o = BlendOptions { operator: BlendOperator::parse(operator).py()?, opacity };
    let (color, over) = (input_raster(py, src)?, input_raster(py, overlay)?);
    let out = py.detach(|| terrio::ops::blend::blend(&color, &over, &o)).py()?;
    finish_raster(py, out, dst, format, compress, tile_size, bigtiff)
}

/// Converts an RGB raster to a paletted one, like `terrio rgb-to-palette` (GDAL's
/// `gdal raster rgb-to-palette`, `rgb2pct.py`).
///
/// Args:
///     src: The raster (a path or a `Raster`), with at least three Byte bands.
///     dst: Also write the result to this path.
///     color_count: The number of colors to compute (2 to 256).
///     color_map: A color table to use instead: a path or a sequence of entries.
///     output_nodata: The index of nodata pixels; by default 0 when the input has a shared nodata
///         value or an alpha band and no color map is given.
///     dither: Floyd-Steinberg dithering; `False` takes the nearest colors.
///     bit_depth: The color component bit depth of the computation, 5 or 8.
///     format: The output format's short name, for `dst`.
///     compress: GeoTIFF/COG compression, for `dst`.
///     tile_size: GeoTIFF/COG tile size, for `dst`.
///     bigtiff: Write a BigTIFF, for `dst`.
///
/// Returns:
///     Raster: One paletted Byte band (its table in `colormap`).
#[pyfunction]
#[pyo3(signature = (
    src, dst=None, *, color_count=256, color_map=None, output_nodata=None, dither=true, bit_depth=5, format=None,
    compress=None, tile_size=None, bigtiff=false
))]
#[allow(clippy::too_many_arguments)]
fn rgb_to_palette(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    dst: Option<&str>,
    color_count: usize,
    color_map: Option<&Bound<'_, PyAny>>,
    output_nodata: Option<u8>,
    dither: bool,
    bit_depth: u32,
    format: Option<&str>,
    compress: Option<String>,
    tile_size: Option<u32>,
    bigtiff: bool,
) -> PyResult<PyRaster> {
    use terrio::ops::palette::RgbToPaletteOptions;
    let o = RgbToPaletteOptions {
        color_count,
        color_table: color_map.map(|c| color_table(c, false)).transpose()?,
        output_nodata,
        dither,
        bit_depth,
    };
    let ds = input_raster(py, src)?;
    let out = py.detach(|| terrio::ops::palette::rgb_to_palette(&ds, &o)).py()?;
    finish_raster(py, out, dst, format, compress, tile_size, bigtiff)
}

/// Expands a paletted band to RGB or RGBA, like `terrio pct2rgb` (GDAL's `pct2rgb.py`).
///
/// Args:
///     src: The raster: a path or a `Raster`.
///     dst: Also write the result to this path.
///     band: The paletted band, from 1.
///     rgba: Also write an alpha band from the entries' alpha.
///     pct: A color table to use instead of the raster's: a path or a sequence of entries.
///     format: The output format's short name, for `dst`.
///     compress: GeoTIFF/COG compression, for `dst`.
///     tile_size: GeoTIFF/COG tile size, for `dst`.
///     bigtiff: Write a BigTIFF, for `dst`.
///
/// Returns:
///     Raster: Red, green and blue (and alpha) Byte bands.
#[pyfunction]
#[pyo3(signature = (src, dst=None, *, band=1, rgba=false, pct=None, format=None, compress=None, tile_size=None, bigtiff=false))]
#[allow(clippy::too_many_arguments)]
fn pct2rgb(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    dst: Option<&str>,
    band: usize,
    rgba: bool,
    pct: Option<&Bound<'_, PyAny>>,
    format: Option<&str>,
    compress: Option<String>,
    tile_size: Option<u32>,
    bigtiff: bool,
) -> PyResult<PyRaster> {
    use terrio::ops::palette::Pct2RgbOptions;
    let o = Pct2RgbOptions { band, rgba, color_table: pct.map(|c| color_table(c, true)).transpose()? };
    let ds = input_raster(py, src)?;
    let out = py.detach(|| terrio::ops::palette::pct2rgb(&ds, &o)).py()?;
    finish_raster(py, out, dst, format, compress, tile_size, bigtiff)
}

/// Attaches a color table to a raster's single band, like `terrio attach-pct` (GDAL's
/// `gdalattachpct.py`).
///
/// Args:
///     pct: The color table: a path (a raster's palette, or a color file read as
///         `gdalattachpct.py` reads it) or a sequence of entries.
///     src: The raster (a path or a `Raster`): a single Byte or UInt16 band.
///     dst: Also write the result to this path.
///     format: The output format's short name, for `dst`.
///     compress: GeoTIFF/COG compression, for `dst`.
///     tile_size: GeoTIFF/COG tile size, for `dst`.
///     bigtiff: Write a BigTIFF, for `dst`.
///
/// Returns:
///     Raster: The raster with the color table.
#[pyfunction]
#[pyo3(signature = (pct, src, dst=None, *, format=None, compress=None, tile_size=None, bigtiff=false))]
#[allow(clippy::too_many_arguments)]
fn attach_pct(
    py: Python<'_>,
    pct: &Bound<'_, PyAny>,
    src: &Bound<'_, PyAny>,
    dst: Option<&str>,
    format: Option<&str>,
    compress: Option<String>,
    tile_size: Option<u32>,
    bigtiff: bool,
) -> PyResult<PyRaster> {
    let table = color_table(pct, true)?;
    let ds = input_raster(py, src)?;
    let out = py.detach(|| terrio::ops::palette::attach_pct(&ds, &table)).py()?;
    finish_raster(py, out, dst, format, compress, tile_size, bigtiff)
}
