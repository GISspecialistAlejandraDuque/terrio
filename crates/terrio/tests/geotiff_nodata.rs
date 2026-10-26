//! The nodata value of a GeoTIFF band as GDAL reads it. GDAL 3.13 rounds a Float32 band's
//! `GDAL_NODATA` tag to Float32 (-9999.1 is -9999.099609375); a `.aux.xml` value overrides the tag
//! unrounded.

use terrio::raster::RasterData;
use terrio::{DataType, LazyRaster, RasterConfig, RasterDataset, RasterWriteOptions};

/// A 2 x 1 GeoTIFF written by terrio, and its `.aux.xml` sidecar when terrio writes one.
fn written(dir: &std::path::Path, data_type: DataType, data: RasterData, nodata: f64) -> (String, std::path::PathBuf) {
    let cfg = RasterConfig { cols: 2, rows: 1, bands: 1, nodata, data_type, ..Default::default() };
    let r = terrio::Raster::from_data_native(cfg, data).unwrap();
    let path = dir.join("n.tif");
    terrio::write_raster_dataset(&RasterDataset::new(r, true), path.to_str().unwrap(), &RasterWriteOptions::default()).unwrap();
    (path.to_string_lossy().into_owned(), dir.join("n.tif.aux.xml"))
}

#[test]
fn float32_nodata_tags_are_rounded_to_float32() {
    let dir = tempfile::tempdir().unwrap();
    let (path, aux) = written(dir.path(), DataType::F32, RasterData::F32(vec![-9999.1, 1.0]), -9999.1);
    let rounded = -9999.1f32 as f64;
    assert_eq!(rounded, -9999.099609375);
    // The tag holds the double as written; terrio keeps it in PAM too, as gdal_translate does.
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.windows(8).any(|w| w == b"-9999.1\0"));
    assert!(aux.exists());
    assert_eq!(terrio::open_raster_lazy(&path).unwrap().nodata(), Some(-9999.1));
    // The tag alone: in memory, or without the sidecar.
    let lazy = LazyRaster::from_tiff_bytes(bytes).unwrap().unwrap();
    assert_eq!((lazy.nodata(), lazy.band_nodata(1)), (Some(rounded), Some(rounded)));
    std::fs::remove_file(&aux).unwrap();
    assert_eq!(terrio::open_raster_lazy(&path).unwrap().nodata(), Some(rounded));
    let ds = terrio::open_raster(&path).unwrap();
    assert_eq!(ds.nodata(), Some(rounded));
    // The sample written with the nodata value equals it.
    assert_eq!(ds.raster.data.to_f64_vec()[0], rounded);
}

#[test]
fn float32_nodata_beyond_the_range_is_infinite() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = written(dir.path(), DataType::F32, RasterData::F32(vec![0.0, 1.0]), 1e39);
    let lazy = LazyRaster::from_tiff_bytes(std::fs::read(path).unwrap()).unwrap().unwrap();
    assert_eq!(lazy.nodata(), Some(f64::INFINITY));
}

#[test]
fn other_types_keep_the_value_as_written() {
    let dir = tempfile::tempdir().unwrap();
    let (path, aux) = written(dir.path(), DataType::F64, RasterData::F64(vec![-9999.1, 1.0]), -9999.1);
    assert!(!aux.exists());
    assert_eq!(LazyRaster::from_tiff_bytes(std::fs::read(path).unwrap()).unwrap().unwrap().nodata(), Some(-9999.1));
}
