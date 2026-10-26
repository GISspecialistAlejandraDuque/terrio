//! `TransformOptions::promote_to_3d`, against GDAL 3.13.3 / PROJ 9.9 after
//! `OGRSpatialReference::PromoteTo3D` on the source (traditional GIS order, no grids).

use terrio_crs::{SpatialRef, TransformOptions, Transformer};

fn transform(src: &str, dst: &str, p: (f64, f64, f64), promote_to_3d: bool) -> (f64, f64, f64) {
    terrio_crs::set_grid_search_paths(Some(Vec::new()));
    terrio_crs::set_network_enabled(Some(false));
    let opts = TransformOptions { promote_to_3d, ..Default::default() };
    let t = Transformer::with_options(&SpatialRef::parse(src).unwrap(), &SpatialRef::parse(dst).unwrap(), &opts).unwrap();
    t.transform_3d(p.0, p.1, p.2).unwrap()
}

#[test]
fn heights_of_promoted_2d_crss_go_through_the_datum_transformation() {
    // (source, point, GDAL's longitude, latitude and height in EPSG:4979). Within 1e-8 degrees
    // and 1 mm: the oblique stereographic case differs from GDAL by 0.3 mm in 2D too.
    let cases = [
        ("EPSG:28992", (85000.0, 447000.0, 0.0), (4.367683424792349, 52.006949098822965, 43.57639261614531)),
        (
            "+proj=sterea +lat_0=79.999999 +ellps=airy +towgs84=12,-34,56",
            (1000.0, 1000.0, 0.0),
            (0.0498592388546416, 80.00917321263181, -440.5388948461041),
        ),
        (
            "+proj=tmerc +a=6371000 +b=6371000 +towgs84=0,0,0",
            (100000.0, 5000000.0, 0.0),
            (1.2709242482153242, 45.15134808463906, 3566.9643223807216),
        ),
    ];
    for (src, p, (lon, lat, h)) in cases {
        let (x, y, z) = transform(src, "EPSG:4979", p, true);
        assert!((x - lon).abs() < 1e-8 && (y - lat).abs() < 1e-8 && (z - h).abs() < 1e-3, "{src}: {x} {y} {z}");
        let promoted = (x, y, z);
        // GDAL's default: the height is kept.
        let (x, y, z) = transform(src, "EPSG:4979", p, false);
        assert!((x - lon).abs() < 1e-8 && (y - lat).abs() < 1e-8 && z == 0.0, "{src}: {x} {y} {z}");
        // And back, within 1 mm: GDAL's own round trip of RD New is 0.2 mm off, as terrio's.
        let (bx, by, bz) = transform("EPSG:4979", src, promoted, true);
        assert!((bx - p.0).abs() < 1e-3 && (by - p.1).abs() < 1e-3 && bz.abs() < 1e-3, "{src}: {bx} {by} {bz}");
    }
}

#[test]
fn crss_with_heights_are_unchanged() {
    // Compound and geographic 3D CRSs keep their own heights.
    let p = (153.0, -27.0, 10.0);
    assert_eq!(transform("EPSG:4326+5773", "EPSG:4979", p, true), transform("EPSG:4326+5773", "EPSG:4979", p, false));
    assert_eq!(transform("EPSG:4979", "EPSG:4978", p, true), transform("EPSG:4979", "EPSG:4978", p, false));
    // Between 2D CRSs of one datum, heights stay ellipsoidal heights of that datum.
    assert_eq!(transform("EPSG:4326", "EPSG:3857", p, true).2, 10.0);
}

#[test]
fn the_option_is_read_from_key_value_options() {
    // `--ct-opt PROMOTE_TO_3D=YES` in the CLI, `ct_opt` and `to` in Python.
    assert!(TransformOptions::from_strict_options(&[("PROMOTE_TO_3D", "YES")]).unwrap().promote_to_3d);
    assert!(!TransformOptions::from_options(&[("promote_to_3d", "no")]).unwrap().promote_to_3d);
    assert!(TransformOptions::from_strict_options(&[("PROMOTE_TO_3D", "perhaps")]).is_err());
}
