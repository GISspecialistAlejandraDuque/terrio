//! `LazyRaster::rpc`: the RPCs of a GeoTIFF from its tag (by path, in memory or sparse) or from a
//! `_RPC.TXT` sidecar, as `ops::rpc::read_rpc` reads them by path.

use terrio::raster::RasterData;
use terrio::sparse::{SparseFile, read_with};
use terrio::{DataType, LazyRaster, RasterConfig, RasterDataset, RasterWriteOptions};

/// The 92 values of an `RPCCoefficientTag`: offsets, scales, then line = lat, sample = lon.
fn coefficients() -> Vec<f64> {
    let mut v = vec![0.0; 92];
    v[..12].copy_from_slice(&[0.5, 0.25, 50.0, 60.0, 45.0, 10.0, 100.0, 50.0, 60.0, 0.1, 0.2, 500.0]);
    v[12 + 2] = 1.0; // line numerator: latitude
    v[32] = 1.0; // line denominator: 1
    v[52 + 1] = 1.0; // sample numerator: longitude
    v[72] = 1.0; // sample denominator: 1
    v
}

fn write(dir: &std::path::Path, rpc: Vec<f64>) -> String {
    let cfg = RasterConfig { cols: 4, rows: 3, bands: 1, data_type: DataType::U8, ..Default::default() };
    let r = terrio::Raster::from_data_native(cfg, RasterData::U8(vec![0; 12])).unwrap();
    let path = dir.join("img.tif").to_string_lossy().into_owned();
    let options = RasterWriteOptions { rpc, ..Default::default() };
    terrio::write_raster_dataset(&RasterDataset::new(r, false), &path, &options).unwrap();
    path
}

#[test]
fn rpc_tags_are_read_from_every_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), coefficients());
    let want = terrio::ops::rpc::read_rpc(&path).unwrap().unwrap();
    assert_eq!((want.line_off, want.long_scale, want.line_num_coeff[2]), (50.0, 0.2, 1.0));
    assert_eq!(terrio::open_raster_lazy(&path).unwrap().rpc().unwrap().as_ref(), Some(&want));
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(LazyRaster::from_tiff_bytes(bytes.clone()).unwrap().unwrap().rpc().unwrap(), Some(want.clone()));
    let file = SparseFile::new(bytes.len() as u64);
    let fetch = |o: u64, n: u64| Ok(bytes[o as usize..(o + n) as usize].to_vec());
    let r = read_with(&file, 8, fetch, || Ok(LazyRaster::from_sparse(file.clone())?.unwrap())).unwrap();
    assert_eq!(r.rpc().unwrap(), Some(want));
}

#[test]
fn rpc_sidecars_are_read_for_local_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), Vec::new());
    assert_eq!(terrio::open_raster_lazy(&path).unwrap().rpc().unwrap(), None);
    let mut txt = String::from("LINE_OFF: 50\nSAMP_OFF: 60\nLAT_OFF: 45\nLONG_OFF: 10\nHEIGHT_OFF: 100\n");
    txt.push_str("LINE_SCALE: 50\nSAMP_SCALE: 60\nLAT_SCALE: 0.1\nLONG_SCALE: 0.2\nHEIGHT_SCALE: 500\n");
    for (name, one) in [("LINE_NUM_COEFF", 3), ("LINE_DEN_COEFF", 1), ("SAMP_NUM_COEFF", 2), ("SAMP_DEN_COEFF", 1)] {
        for i in 1..=20 {
            txt.push_str(&format!("{name}_{i}: {}\n", u8::from(i == one)));
        }
    }
    std::fs::write(dir.path().join("img_rpc.txt"), txt).unwrap();
    let want = terrio::ops::rpc::read_rpc(&path).unwrap().unwrap();
    assert_eq!(terrio::open_raster_lazy(&path).unwrap().rpc().unwrap(), Some(want));
    // A file in memory has no sidecars.
    assert_eq!(LazyRaster::from_tiff_bytes(std::fs::read(&path).unwrap()).unwrap().unwrap().rpc().unwrap(), None);
}
