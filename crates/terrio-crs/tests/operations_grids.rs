//! The operation a transformer reports with the grids of `TERRIO_TEST_DATA/grids` (see
//! `scripts/fetch_test_data.sh`), against PROJ 9.9's first candidate with the same grids. Geoid
//! models are named without PROJ's EPSG index (`WGS 84 to EGM96 height (1)`).

use terrio_crs::{SpatialRef, Transformer};

fn grids() -> Option<()> {
    let dir = std::path::PathBuf::from(std::env::var_os("TERRIO_TEST_DATA")?).join("grids");
    if !dir.join("uk_os_OSTN15_NTv2_OSGBtoETRS.tif").exists() || !dir.join("us_nga_egm96_15.tif").exists() {
        eprintln!("the grids are missing: run scripts/fetch_test_data.sh");
        return None;
    }
    terrio_crs::set_grid_search_paths(Some(vec![dir]));
    terrio_crs::set_network_enabled(Some(false));
    Some(())
}

#[test]
fn reports_grid_transformations() {
    let Some(()) = grids() else { return };
    let t = Transformer::new(&SpatialRef::parse("EPSG:27700").unwrap(), &SpatialRef::parse("EPSG:4326").unwrap()).unwrap();
    let op = t.operation_at(530000.0, 180000.0, 0.0).unwrap().unwrap();
    assert_eq!((op.name.as_str(), op.accuracy, op.ballpark), ("OSGB36 to WGS 84 (9)", Some(1.0), false));
    assert_eq!(op.grids, ["uk_os_OSTN15_NTv2_OSGBtoETRS.tif"]);
    let t = Transformer::new(&SpatialRef::parse("EPSG:4326+5773").unwrap(), &SpatialRef::parse("EPSG:4979").unwrap()).unwrap();
    let op = t.operation_at(153.0, -27.0, 0.0).unwrap().unwrap();
    assert_eq!((op.name.as_str(), op.accuracy, op.ballpark), ("Inverse of WGS 84 to EGM96 height", Some(1.0), false));
    assert_eq!(op.grids, ["us_nga_egm96_15.tif"]);
}
