//! Pixel-wise raster operations: `unscale`, `nodataToAlpha`, `reclassify`, `asFeatures`,
//! `blend`, `rgbToPalette`, `pct2rgb` and `attachPct`, with camelCase options after the
//! command-line options.

use crate::ops::input_raster;
use crate::{Dataset, js_err, parse_options};
use serde::Deserialize;
use terrio::formats::VectorDriver;
use terrio::ops::as_features::{AsFeaturesOptions, PixelGeometry};
use terrio::ops::parse_data_type;
use terrio::ops::pixel::{NodataToAlphaOptions, ReclassifyOptions, UnscaleOptions, mapping_from_file_text};
use wasm_bindgen::prelude::*;

fn invalid(msg: impl std::fmt::Display) -> JsError {
    JsError::new(&msg.to_string())
}

/// Options of [`unscale`].
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct UnscaleArgs {
    ot: Option<String>,
}

/// Applies band scales and offsets (`gdal raster unscale`). Options: `ot`, the output data type
/// (Float32, or Float64 for Float64 sources, by default). Nodata pixels keep the nodata value.
#[wasm_bindgen]
pub fn unscale(src: JsValue, options: JsValue) -> Result<Dataset, JsError> {
    let a: UnscaleArgs = parse_options(&options)?;
    let o = UnscaleOptions { output_type: a.ot.as_deref().map(parse_data_type).transpose().map_err(js_err)? };
    let ds = input_raster(&src)?;
    Ok(Dataset::from_raster(terrio::ops::pixel::unscale(&ds, &o).map_err(js_err)?))
}

/// A number, or an array of them.
#[derive(Deserialize)]
#[serde(untagged)]
enum Numbers {
    One(f64),
    Many(Vec<f64>),
}

/// Options of [`nodata_to_alpha`].
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct NodataToAlphaArgs {
    nodata: Option<Numbers>,
}

/// Replaces nodata values with an alpha band (`gdal raster nodata-to-alpha`). Options:
/// `nodata`, a value overriding the source's, or one per band (transparent where every band has
/// its value).
#[wasm_bindgen(js_name = nodataToAlpha)]
pub fn nodata_to_alpha(src: JsValue, options: JsValue) -> Result<Dataset, JsError> {
    let a: NodataToAlphaArgs = parse_options(&options)?;
    let nodata = match a.nodata {
        None => Vec::new(),
        Some(Numbers::One(v)) => vec![v],
        Some(Numbers::Many(v)) => v,
    };
    let ds = input_raster(&src)?;
    Ok(Dataset::from_raster(terrio::ops::pixel::nodata_to_alpha(&ds, &NodataToAlphaOptions { nodata }).map_err(js_err)?))
}

/// Options of [`reclassify`].
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct ReclassifyArgs {
    ot: Option<String>,
    mapping_file: Option<String>,
}

/// Maps values or ranges to new values (`gdal raster reclassify`). `mapping` is GDAL's mapping
/// (`"[0,10)=1;[10,inf)=2;NO_DATA=NO_DATA;DEFAULT=PASS_THROUGH"`). Options: `ot`, the output data
/// type; `mappingFile`, the text of a mapping file (one entry per line, `#` comments) used
/// instead of `mapping`.
#[wasm_bindgen]
pub fn reclassify(src: JsValue, mapping: &str, options: JsValue) -> Result<Dataset, JsError> {
    let a: ReclassifyArgs = parse_options(&options)?;
    let mapping = match a.mapping_file {
        Some(text) => mapping_from_file_text(&text).map_err(js_err)?,
        None if mapping.starts_with('@') => return Err(invalid("files cannot be read here: give their text as `mappingFile`")),
        None => mapping.to_string(),
    };
    let o = ReclassifyOptions { mapping, output_type: a.ot.as_deref().map(parse_data_type).transpose().map_err(js_err)? };
    let ds = input_raster(&src)?;
    Ok(Dataset::from_raster(terrio::ops::pixel::reclassify(&ds, &o).map_err(js_err)?))
}

/// Options of [`as_features`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct AsFeaturesArgs {
    bands: Vec<usize>,
    geometry_type: String,
    skip_nodata: bool,
    include_xy: bool,
    include_row_col: bool,
    layer_name: String,
}

impl Default for AsFeaturesArgs {
    fn default() -> Self {
        Self {
            bands: Vec::new(),
            geometry_type: "none".into(),
            skip_nodata: false,
            include_xy: false,
            include_row_col: false,
            layer_name: "pixels".into(),
        }
    }
}

/// One vector feature per pixel (`gdal raster as-features`). Options: `bands` (from 1, all by
/// default), `geometryType` (`none`, `point` or `polygon`), `skipNodata`, `includeXy`,
/// `includeRowCol`, `layerName` (`pixels`).
#[wasm_bindgen(js_name = asFeatures)]
pub fn as_features(src: JsValue, options: JsValue) -> Result<Dataset, JsError> {
    let a: AsFeaturesArgs = parse_options(&options)?;
    let o = AsFeaturesOptions {
        bands: a.bands,
        geometry: PixelGeometry::parse(&a.geometry_type).map_err(js_err)?,
        skip_nodata: a.skip_nodata,
        include_xy: a.include_xy,
        include_row_col: a.include_row_col,
        layer_name: a.layer_name,
    };
    let ds = input_raster(&src)?;
    let layer = terrio::ops::as_features::as_features(&ds, &o).map_err(js_err)?;
    Ok(Dataset::from_layers(vec![layer], VectorDriver::GeoJSON))
}

/// A color table given to an operation: the text of a color file, an array of `[r, g, b]` or
/// `[r, g, b, a]` entries, or a raster (a `Dataset` or bytes) whose palette to take.
fn color_table(v: &JsValue, fill: bool) -> Result<Vec<[u8; 4]>, JsError> {
    if let Some(text) = v.as_string() {
        return terrio::ops::palette::parse_color_table(&text, fill).map_err(js_err);
    }
    if js_sys::Array::is_array(v) {
        let text: String = js_sys::JSON::stringify(v).map_err(|_| invalid("invalid color table"))?.into();
        let entries: Vec<Vec<u8>> = serde_json::from_str(&text).map_err(|e| invalid(format!("color table: {e}")))?;
        return entries
            .into_iter()
            .map(|e| match e.as_slice() {
                [r, g, b] => Ok([*r, *g, *b, 255]),
                [r, g, b, a] => Ok([*r, *g, *b, *a]),
                _ => Err(invalid("color table entries are [r, g, b] or [r, g, b, a]")),
            })
            .collect();
    }
    let ds = input_raster(v)?;
    if ds.color_table.is_empty() {
        return Err(invalid("the raster has no color table"));
    }
    Ok(ds.color_table)
}

/// Splits `options` into the plain options and the value of `key`.
fn take(options: &JsValue, key: &str) -> Result<(JsValue, Option<JsValue>), JsError> {
    if options.is_undefined() || options.is_null() {
        return Ok((JsValue::UNDEFINED, None));
    }
    let k = JsValue::from_str(key);
    let value = js_sys::Reflect::get(options, &k).ok().filter(|v| !v.is_undefined());
    let rest = js_sys::Object::assign(&js_sys::Object::new(), &options.clone().into());
    js_sys::Reflect::delete_property(&rest, &k).map_err(|_| invalid("invalid options"))?;
    Ok((rest.into(), value))
}

/// Options of [`blend`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct BlendArgs {
    operator: String,
    opacity: u32,
}

impl Default for BlendArgs {
    fn default() -> Self {
        Self { operator: "src-over".into(), opacity: 100 }
    }
}

/// Blends an overlay onto a raster (`gdal raster blend`). Options: `operator` (`src-over`,
/// `hsv-value`, `multiply`, `screen`, `overlay`, `hard-light`, `darken`, `lighten`,
/// `color-burn`, `color-dodge`) and `opacity` (percent).
#[wasm_bindgen]
pub fn blend(src: JsValue, overlay: JsValue, options: JsValue) -> Result<Dataset, JsError> {
    use terrio::ops::blend::{BlendOperator, BlendOptions};
    let a: BlendArgs = parse_options(&options)?;
    let o = BlendOptions { operator: BlendOperator::parse(&a.operator).map_err(js_err)?, opacity: a.opacity };
    let (color, over) = (input_raster(&src)?, input_raster(&overlay)?);
    Ok(Dataset::from_raster(terrio::ops::blend::blend(&color, &over, &o).map_err(js_err)?))
}

/// Options of [`rgb_to_palette`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct RgbToPaletteArgs {
    color_count: usize,
    output_nodata: Option<u8>,
    dither: bool,
    bit_depth: u32,
}

impl Default for RgbToPaletteArgs {
    fn default() -> Self {
        Self { color_count: 256, output_nodata: None, dither: true, bit_depth: 5 }
    }
}

/// Converts RGB to a paletted raster (`gdal raster rgb-to-palette`, `rgb2pct.py`). Options:
/// `colorCount`, `colorMap` (a color file's text, an array of entries, or a raster),
/// `outputNodata`, `dither` (`true`), `bitDepth` (5 or 8).
#[wasm_bindgen(js_name = rgbToPalette)]
pub fn rgb_to_palette(src: JsValue, options: JsValue) -> Result<Dataset, JsError> {
    use terrio::ops::palette::RgbToPaletteOptions;
    let (rest, map) = take(&options, "colorMap")?;
    let a: RgbToPaletteArgs = parse_options(&rest)?;
    let o = RgbToPaletteOptions {
        color_count: a.color_count,
        color_table: map.as_ref().map(|m| color_table(m, false)).transpose()?,
        output_nodata: a.output_nodata,
        dither: a.dither,
        bit_depth: a.bit_depth,
    };
    let ds = input_raster(&src)?;
    Ok(Dataset::from_raster(terrio::ops::palette::rgb_to_palette(&ds, &o).map_err(js_err)?))
}

/// Options of [`pct2rgb`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct Pct2RgbArgs {
    band: usize,
    rgba: bool,
}

impl Default for Pct2RgbArgs {
    fn default() -> Self {
        Self { band: 1, rgba: false }
    }
}

/// Expands a paletted band to RGB or RGBA (`pct2rgb.py`). Options: `band` (1), `rgba`, `pct` (a
/// color table instead of the raster's: a color file's text, an array of entries, or a raster).
#[wasm_bindgen]
pub fn pct2rgb(src: JsValue, options: JsValue) -> Result<Dataset, JsError> {
    use terrio::ops::palette::Pct2RgbOptions;
    let (rest, pct) = take(&options, "pct")?;
    let a: Pct2RgbArgs = parse_options(&rest)?;
    let o = Pct2RgbOptions { band: a.band, rgba: a.rgba, color_table: pct.as_ref().map(|p| color_table(p, true)).transpose()? };
    let ds = input_raster(&src)?;
    Ok(Dataset::from_raster(terrio::ops::palette::pct2rgb(&ds, &o).map_err(js_err)?))
}

/// Attaches a color table to a raster's single Byte or UInt16 band (`gdalattachpct.py`). `pct`
/// is a color file's text, an array of entries, or a raster whose palette to take.
#[wasm_bindgen(js_name = attachPct)]
pub fn attach_pct(pct: JsValue, src: JsValue) -> Result<Dataset, JsError> {
    let table = color_table(&pct, true)?;
    let ds = input_raster(&src)?;
    Ok(Dataset::from_raster(terrio::ops::palette::attach_pct(&ds, &table).map_err(js_err)?))
}
