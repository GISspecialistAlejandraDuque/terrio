//! `terrio blend`, `terrio rgb-to-palette`, `terrio pct2rgb` and `terrio attach-pct`.

use super::rasterout::{OutputArgs, open};
use clap::builder::TypedValueParser;
use terrio::ops::blend::{BlendOperator, BlendOptions};
use terrio::ops::palette::{Pct2RgbOptions, RgbToPaletteOptions, load_color_table};

/// `terrio blend`.
#[derive(clap::Args)]
pub struct BlendArgs {
    /// The color input: 1 to 4 Byte bands (gray, gray and alpha, RGB, RGBA) or a paletted band.
    input: String,
    /// The overlay: 1 to 4 Byte bands or a paletted band, of the input's size.
    overlay: String,
    /// The output path.
    output: String,
    /// The composition operator: src-over, hsv-value, multiply, screen, overlay, hard-light,
    /// darken, lighten, color-burn or color-dodge.
    #[arg(long, default_value = "src-over")]
    operator: String,
    /// The overlay's opacity in percent (0 transparent, 100 the overlay's own opacity).
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(0..=100))]
    opacity: u32,
    #[command(flatten)]
    out: OutputArgs,
}

/// Runs `terrio blend`.
pub fn blend(a: BlendArgs) -> terrio::Result<()> {
    let o = BlendOptions { operator: BlendOperator::parse(&a.operator)?, opacity: a.opacity };
    let (color, overlay) = (open(&a.input)?, open(&a.overlay)?);
    a.out.write(&terrio::ops::blend::blend(&color, &overlay, &o)?, &a.output)
}

/// `terrio rgb-to-palette`.
#[derive(clap::Args)]
pub struct RgbToPaletteArgs {
    /// The input: at least three Byte bands (red, green and blue by color interpretation, or the
    /// first three).
    input: String,
    /// The output path: one paletted Byte band.
    output: String,
    /// The number of colors of the computed color table (2 to 256).
    #[arg(short = 'n', long, default_value_t = 256, value_parser = clap::value_parser!(u16).range(2..=256))]
    color_count: u16,
    /// Use this color table instead of computing one: a raster's, or a color file (`index red
    /// green blue [alpha]` lines).
    #[arg(long, visible_alias = "pct", conflicts_with = "color_count")]
    color_map: Option<String>,
    /// The index of nodata pixels (0 to 255). By default 0 when the input has a shared Byte
    /// nodata value or an alpha band and no color map is given.
    #[arg(long, visible_alias = "dst-nodata")]
    output_nodata: Option<u8>,
    /// Take the nearest color instead of dithering (Floyd-Steinberg).
    #[arg(long)]
    no_dither: bool,
    /// The bit depth of the color components in the histogram and lookups: 5 or 8 (slower).
    #[arg(long, default_value_t = 5, value_parser = clap::builder::PossibleValuesParser::new(["5", "8"]).map(|s| s.parse::<u32>().unwrap()))]
    bit_depth: u32,
    #[command(flatten)]
    out: OutputArgs,
}

/// Runs `terrio rgb-to-palette`.
pub fn rgb_to_palette(a: RgbToPaletteArgs) -> terrio::Result<()> {
    let o = RgbToPaletteOptions {
        color_count: a.color_count as usize,
        color_table: a.color_map.as_deref().map(|p| load_color_table(p, false)).transpose()?,
        output_nodata: a.output_nodata,
        dither: !a.no_dither,
        bit_depth: a.bit_depth,
    };
    let ds = open(&a.input)?;
    a.out.write(&terrio::ops::palette::rgb_to_palette(&ds, &o)?, &a.output)
}

/// `terrio pct2rgb`.
#[derive(clap::Args)]
pub struct Pct2RgbArgs {
    /// The paletted input.
    input: String,
    /// The output path: red, green and blue (and alpha) Byte bands.
    output: String,
    /// The paletted band, from 1.
    #[arg(short = 'b', long = "band", default_value_t = 1)]
    band: usize,
    /// Also write an alpha band, from the color table's alpha.
    #[arg(long)]
    rgba: bool,
    /// Use this color table instead of the input's: a raster's, or a color file (indices 0 to
    /// 255, each missing one taking the color of the line before, as pct2rgb.py reads them).
    #[arg(long)]
    pct: Option<String>,
    #[command(flatten)]
    out: OutputArgs,
}

/// Runs `terrio pct2rgb`.
pub fn pct2rgb(a: Pct2RgbArgs) -> terrio::Result<()> {
    let o = Pct2RgbOptions { band: a.band, rgba: a.rgba, color_table: a.pct.as_deref().map(|p| load_color_table(p, true)).transpose()? };
    let ds = open(&a.input)?;
    a.out.write(&terrio::ops::palette::pct2rgb(&ds, &o)?, &a.output)
}

/// `terrio attach-pct`.
#[derive(clap::Args)]
pub struct AttachPctArgs {
    /// The color table: a raster's, or a color file (indices 0 to 255, each missing one taking
    /// the color of the line before, as gdalattachpct.py reads them).
    pct: String,
    /// The input: a single Byte or UInt16 band.
    input: String,
    /// The output path (GeoTIFF or COG keep the color table).
    output: String,
    #[command(flatten)]
    out: OutputArgs,
}

/// Runs `terrio attach-pct`.
pub fn attach_pct(a: AttachPctArgs) -> terrio::Result<()> {
    let table = load_color_table(&a.pct, true)?;
    let ds = open(&a.input)?;
    a.out.write(&terrio::ops::palette::attach_pct(&ds, &table)?, &a.output)
}
