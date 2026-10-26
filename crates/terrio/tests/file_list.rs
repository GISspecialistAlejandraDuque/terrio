//! `LazyRaster::file_list`: the files a raster is read from, as GDAL 3.13's `GetFileList` lists
//! them for a GeoTIFF with a `.msk` mask, an `.aux.xml` sidecar and an `.ovr`.

use terrio::raster::RasterData;
use terrio::{DataType, LazyRaster, RasterConfig, RasterDataset, RasterWriteOptions};

fn write(path: &std::path::Path) {
    write_sized(path, 4, 3);
}

fn write_sized(path: &std::path::Path, cols: usize, rows: usize) {
    let cfg = RasterConfig { cols, rows, bands: 1, data_type: DataType::U8, ..Default::default() };
    let r = terrio::Raster::from_data_native(cfg, RasterData::U8(vec![255; cols * rows])).unwrap();
    let path = path.to_str().unwrap();
    terrio::write_raster_dataset(&RasterDataset::new(r, false), path, &RasterWriteOptions::default()).unwrap();
}

#[test]
fn lists_the_tiff_and_the_sidecars_it_reads() {
    let dir = tempfile::tempdir().unwrap();
    let tif = dir.path().join("img.tif");
    write(&tif);
    let path = tif.to_str().unwrap().to_string();
    assert_eq!(terrio::open_raster_lazy(&path).unwrap().file_list().unwrap(), [path.as_str()]);

    // GDAL: ['img.tif', 'img.tif.msk', 'img.tif.aux.xml'].
    write(&dir.path().join("mask.tif"));
    std::fs::rename(dir.path().join("mask.tif"), dir.path().join("img.tif.msk")).unwrap();
    let pam = r#"<PAMDataset><PAMRasterBand band="1"><NoDataValue>7</NoDataValue></PAMRasterBand></PAMDataset>"#;
    std::fs::write(dir.path().join("img.tif.aux.xml"), pam).unwrap();
    let r = terrio::open_raster_lazy(&path).unwrap();
    assert!(r.has_mask());
    assert_eq!(r.nodata(), Some(7.0));
    assert_eq!(r.file_list().unwrap(), [path.clone(), format!("{path}.msk"), format!("{path}.aux.xml")]);

    // With an .ovr too, GDAL: ['img.tif', 'img.tif.ovr', 'img.tif.msk', 'img.tif.aux.xml'].
    write_sized(&dir.path().join("ovr.tif"), 2, 2);
    std::fs::rename(dir.path().join("ovr.tif"), dir.path().join("img.tif.ovr")).unwrap();
    let r = terrio::open_raster_lazy(&path).unwrap();
    assert_eq!(r.overviews(), [(2, 2)]);
    assert_eq!(r.file_list().unwrap(), [path.clone(), format!("{path}.ovr"), format!("{path}.msk"), format!("{path}.aux.xml")]);

    // In memory: no files.
    assert!(LazyRaster::from_tiff_bytes(std::fs::read(&tif).unwrap()).unwrap().unwrap().file_list().unwrap().is_empty());
}
