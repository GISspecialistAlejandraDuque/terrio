#!/usr/bin/env python3
"""Compare terrio's pixel-wise raster commands with GDAL's on real files.

Runs ``terrio unscale``, ``nodata-to-alpha``, ``reclassify``, ``as-features``, ``blend``,
``rgb-to-palette``, ``pct2rgb`` and ``attach-pct`` and the equivalent ``gdal raster``
commands (GDAL 3.12 or later; the reference is GDAL 3.13.3), ``rgb2pct.py``, ``pct2rgb.py`` and ``gdalattachpct.py`` on the
files fetched by ``scripts/fetch_test_data.sh``, then reports for each case whether the outputs
have the same size, type, nodata value, CRS, band color interpretations, scales and color
tables, and how many pixels agree. Feature outputs are compared feature by feature: count,
field names, field values and geometry coordinates.

GDAL 3.13's ``rgb-to-palette`` options (``--output-nodata``, ``--no-dither``, ``--bit-depth``,
nodata and alpha masks) and ``blend`` operators beyond ``src-over`` and ``hsv-value`` are
compared with GDAL 3.13 only (skipped with GDAL 3.12). ``rgb-to-palette`` is also checked
against the band checksums GDAL 3.13.3's autotest expects, on its own test files (downloaded
from GDAL's repository at v3.13.3).

Usage:
    python scripts/compare_raster_pixel_with_gdal.py ../terrio-data [--terrio target/release/terrio]
"""

import argparse
import json
import os
import shutil
import sys
import tempfile

import numpy as np
from osgeo import gdal, ogr, osr

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from compare_dem_calc_with_gdal import compare, format_row, run  # noqa: E402

gdal.UseExceptions()
ogr.UseExceptions()

#: GDAL 3.13 adds rgb-to-palette's nodata handling and options and the separable blend operators.
GDAL_313 = int(gdal.VersionInfo()) >= 3130000

S5P = (
    "S5P_OFFL_L2__CH4____20230101T000319_20230101T014449_27036_03_020400_20230103T110255.nc"
)


def prepare(d, tmp):
    """Writes the inputs derived from the real files.

    Args:
        d: The test data directory.
        tmp: The work directory.

    Returns:
        dict: Input paths by name.
    """
    p = {}
    usgs = f"{d}/usgs_dem.tif"
    # Real elevations packed as Int16 decimetres above 1000 m, with GDAL's scale and offset.
    p["packed"] = os.path.join(tmp, "packed.tif")
    ds = gdal.Translate(p["packed"], usgs, srcWin=[1000, 1000, 600, 500], outputType=gdal.GDT_Int16,
                        scaleParams=[[1000, 4276.7, 0, 32767]], noData=-32768)
    a = ds.GetRasterBand(1).ReadAsArray()
    a[:20, :30] = -32768
    ds.GetRasterBand(1).WriteArray(a)
    ds.GetRasterBand(1).SetScale(0.1)
    ds.GetRasterBand(1).SetOffset(1000)
    ds = None
    # Landsat bands with a scale and offset of their own (reflectance-like), nodata 0.
    p["landsat_scaled"] = os.path.join(tmp, "landsat_scaled.tif")
    ds = gdal.Translate(p["landsat_scaled"], f"{d}/landsat.tif", srcWin=[600, 500, 400, 300])
    for i in range(1, 5):
        ds.GetRasterBand(i).SetScale(2.75e-5 * i)
        ds.GetRasterBand(i).SetOffset(-0.2 + 0.05 * i)
    ds = None
    # Land surface temperature (Float32, NaN nodata) in hundredths of a kelvin.
    p["lst_scaled"] = os.path.join(tmp, "lst_scaled.tif")
    ds = gdal.Translate(p["lst_scaled"], f"{d}/ECOv002_L2T_LSTE_26860_001_10SGD_20230401T203733_0710_01_LST.tif",
                        srcWin=[300, 300, 400, 400])
    ds.GetRasterBand(1).SetScale(0.01)
    ds.GetRasterBand(1).SetOffset(-273.15)
    ds = None
    p["landsat_window"] = os.path.join(tmp, "landsat_window.tif")
    gdal.Translate(p["landsat_window"], f"{d}/landsat.tif", srcWin=[0, 0, 500, 400])
    p["landsat7_window"] = os.path.join(tmp, "landsat7_window.tif")
    gdal.Translate(p["landsat7_window"], f"{d}/landsat7.tif", srcWin=[0, 0, 500, 400])
    p["dem_window"] = os.path.join(tmp, "dem_window.tif")
    gdal.Translate(p["dem_window"], f"{d}/dem.tif", srcWin=[1300, 500, 400, 300])
    p["usgs_window"] = os.path.join(tmp, "usgs_window.tif")
    ds = gdal.Translate(p["usgs_window"], usgs, srcWin=[1500, 1500, 300, 300])
    a = ds.GetRasterBand(1).ReadAsArray()
    a[100:140, 50:90] = -999999
    ds.GetRasterBand(1).WriteArray(a)
    ds = None
    # UInt16 with nodata, from the Landsat scene.
    p["landsat_u16"] = os.path.join(tmp, "landsat_u16.tif")
    gdal.Translate(p["landsat_u16"], p["landsat_window"], outputType=gdal.GDT_UInt16, scaleParams=[[0, 255, 0, 65535]],
                   noData=0)
    p["nlcd"] = f"{d}/nlcd_knoxville.tif"
    p["nlcd_nodata"] = os.path.join(tmp, "nlcd_nodata.tif")
    ds = gdal.Translate(p["nlcd_nodata"], p["nlcd"], noData=11)
    ds = None
    p["usgs_small"] = os.path.join(tmp, "usgs_small.tif")
    ds = gdal.Translate(p["usgs_small"], usgs, srcWin=[2000, 2000, 60, 45])
    a = ds.GetRasterBand(1).ReadAsArray()
    a[:5, :7] = -999999
    ds.GetRasterBand(1).WriteArray(a)
    ds = None
    p["landsat_small"] = os.path.join(tmp, "landsat_small.tif")
    gdal.Translate(p["landsat_small"], f"{d}/landsat.tif", srcWin=[0, 0, 50, 40])
    p["s5p_qa"] = f'NETCDF:"{d}/{S5P}":/PRODUCT/qa_value'
    mapping = os.path.join(tmp, "nlcd_mapping.txt")
    with open(mapping, "w") as f:
        f.write("# NLCD classes to groups\n11=1  # water\n[21,25)=2\n31=3\n[41,44)=4 # forest\n"
                "52=5\n71=5\n[81,83)=6\n90=7\n95=7\nDEFAULT=0\n")
    p["nlcd_mapping"] = mapping
    return p


def cases(d, tmp):
    """The comparison cases.

    Args:
        d: The test data directory.
        tmp: The work directory.

    Returns:
        list: Dicts with the case name, the GDAL and terrio command lines (``{out}`` standing for
        the output path), the tolerance and the output extension.
    """
    p = prepare(d, tmp)
    out = []

    def add(name, gdal_cmd, terrio_cmd, tol=0.0, ext="tif", kind="raster"):
        out.append({"name": name, "gdal": gdal_cmd, "terrio": terrio_cmd, "tol": tol, "ext": ext, "kind": kind})

    def unscale(name, src, ot=None):
        extra = ["--ot", ot] if ot else []
        add(f"unscale {name}", ["gdal", "raster", "unscale", "-q", src, "{out}", *extra], ["unscale", src, "{out}", *extra])

    unscale("Int16 elevation", p["packed"])
    unscale("Int16 elevation Float64", p["packed"], "Float64")
    unscale("Int16 elevation UInt16", p["packed"], "UInt16")
    unscale("Landsat 4 bands", p["landsat_scaled"])
    unscale("Landsat Byte", p["landsat_scaled"], "Byte")
    unscale("LST Float32 NaN", p["lst_scaled"])
    unscale("LST Int32", p["lst_scaled"], "Int32")
    if gdal.GetDriverByName("netCDF") is not None:
        unscale("S5P qa_value netCDF", p["s5p_qa"])
    unscale("unscaled DEM Float64", p["dem_window"])

    def alpha(name, src, nodata=()):
        g = [x for v in nodata for x in ("--nodata", str(v))]
        add(f"nodata-to-alpha {name}", ["gdal", "raster", "nodata-to-alpha", "-q", src, "{out}", *g],
            ["nodata-to-alpha", src, "{out}", *g])

    alpha("Landsat 4 bands", p["landsat_window"])
    alpha("Landsat 7 RGB", p["landsat7_window"])
    alpha("Landsat UInt16", p["landsat_u16"])
    alpha("DEM Float64 NaN", p["dem_window"])
    alpha("USGS Float32", p["usgs_window"])
    alpha("one override", p["landsat7_window"], [12])
    alpha("per-band overrides", p["landsat7_window"], [0, 0, 0])
    alpha("no nodata", f"{d}/srtm90.tif")

    def reclass(name, src, mapping, ot=None):
        extra = ["--ot", ot] if ot else []
        add(f"reclassify {name}", ["gdal", "raster", "reclassify", "-q", src, "{out}", "-m", mapping, *extra],
            ["reclassify", src, "{out}", "-m", mapping, *extra])

    reclass("NLCD groups file", p["nlcd"], "@" + p["nlcd_mapping"])
    reclass("NLCD NO_DATA", p["nlcd_nodata"], "NO_DATA=NO_DATA;[20,30)=1;DEFAULT=PASS_THROUGH")
    reclass("elevation ranges Byte", p["usgs_window"],
            "NO_DATA=0;[0,1500)=1;[1500,2000)=2;[2000,2500)=3;[2500,inf)=4", "Byte")
    reclass("elevation pass-through", p["usgs_window"], "[2000,2500]=0;DEFAULT=PASS_THROUGH")
    reclass("LST NaN and ranges", p["lst_scaled"], "NAN=-1;(-inf,29000)=1;[29000,30000)=2;[30000,inf]=3", "Int16")
    reclass("Landsat bands", p["landsat_window"], "0=NO_DATA;[1,100)=50;[100,255]=PASS_THROUGH")

    def features(name, src, opts, ext="gpkg"):
        # GDAL 3.12 names the layer "" by default (3.13 names it "pixels"): name it for both.
        opts = [*opts, "--output-layer", "pixels"]
        add(f"as-features {name}", ["gdal", "raster", "as-features", src, "{out}", *opts],
            ["as-features", src, "{out}", *opts], ext=ext, kind="vector")

    features("polygons xy row-col", p["usgs_small"], ["--geometry-type", "polygon", "--include-xy", "--include-row-col"])
    features("points skip-nodata", p["usgs_small"], ["--geometry-type", "point", "--skip-nodata"])
    features("table Landsat bands", p["landsat_small"], ["--include-row-col"], ext="csv")
    features("Landsat polygons skip-nodata", p["landsat_small"], ["--geometry-type", "polygon", "--skip-nodata"])

    # Color tables. RGB without nodata (GDAL 3.12's median cut ignores nodata; 3.13's does not).
    rgb = os.path.join(tmp, "rgb.tif")
    gdal.Translate(rgb, f"{d}/landsat7.tif", srcWin=[0, 0, 400, 300], noData="none")
    opera = f"{d}/OPERA_L3_DSWx_WTR.tif"
    add("rgb-to-palette rgb2pct.py 256 colors", ["rgb2pct.py", rgb, "{out}"], ["rgb-to-palette", rgb, "{out}"])
    add("rgb-to-palette rgb2pct.py 16 colors", ["rgb2pct.py", "-n", "16", rgb, "{out}"],
        ["rgb-to-palette", rgb, "{out}", "--color-count", "16"])
    add("rgb-to-palette rgb2pct.py raster palette", ["rgb2pct.py", "-pct", opera, rgb, "{out}"],
        ["rgb-to-palette", rgb, "{out}", "--color-map", opera])
    add("rgb-to-palette gdal raster 64 colors", ["gdal", "raster", "rgb-to-palette", "-q", rgb, "{out}", "--color-count", "64"],
        ["rgb-to-palette", rgb, "{out}", "--color-count", "64"])
    palette = os.path.join(tmp, "palette.txt")
    with open(palette, "w") as f:
        f.write("0 0 0 0\n11 70 107 159\n21 222 197 197\n41 104 171 95\n81 220 217 57 200\n95 108 159 184\n")
    nlcd = p["nlcd"]
    add("pct2rgb NLCD RGB", ["pct2rgb.py", nlcd, "{out}"], ["pct2rgb", nlcd, "{out}"])
    add("pct2rgb OPERA RGBA", ["pct2rgb.py", "-rgba", opera, "{out}"], ["pct2rgb", opera, "{out}", "--rgba"])
    add("pct2rgb palette file", ["pct2rgb.py", "-rgba", "-pct", palette, nlcd, "{out}"],
        ["pct2rgb", nlcd, "{out}", "--rgba", "--pct", palette])
    plain = os.path.join(tmp, "nlcd_plain.tif")
    src = gdal.Open(nlcd)
    ds = gdal.GetDriverByName("GTiff").Create(plain, src.RasterXSize, src.RasterYSize, 1)
    ds.SetGeoTransform(src.GetGeoTransform())
    ds.SetProjection(src.GetProjection())
    ds.GetRasterBand(1).WriteArray(src.ReadAsArray())
    ds = src = None
    add("attach-pct palette file", ["gdalattachpct.py", palette, plain, "{out}"], ["attach-pct", palette, plain, "{out}"])
    add("attach-pct raster palette", ["gdalattachpct.py", opera, plain, "{out}"], ["attach-pct", opera, plain, "{out}"])

    # Blending: real Landsat 7 colors with a hillshade of USGS terrain, and GDAL's RGBA test image.
    hs = os.path.join(tmp, "hillshade.tif")
    gdal.DEMProcessing(os.path.join(tmp, "hs_full.tif"), f"{d}/usgs_dem.tif", "hillshade")
    gdal.Translate(hs, os.path.join(tmp, "hs_full.tif"), width=400, height=300, resampleAlg="bilinear")
    rgba = os.path.join(tmp, "rgba.tif")
    gdal.Translate(rgba, p["landsat7_window"], srcWin=[0, 0, 400, 300])
    ds = gdal.Translate(os.path.join(tmp, "rgba4.tif"), rgba, bandList=[1, 2, 3, 1])
    alpha = np.where(ds.GetRasterBand(1).ReadAsArray() > 0, 255, 0).astype(np.uint8)
    alpha[100:150, 100:200] = 128
    ds.GetRasterBand(4).WriteArray(alpha)
    ds.GetRasterBand(4).SetColorInterpretation(gdal.GCI_AlphaBand)
    for b in range(1, 5):
        ds.GetRasterBand(b).DeleteNoDataValue()
    ds = None
    rgba4 = os.path.join(tmp, "rgba4.tif")
    gray_alpha = os.path.join(tmp, "gray_alpha.tif")
    gdal.Translate(gray_alpha, rgba4, bandList=[2, 4])

    def blend(name, color, overlay, opts=()):
        add(f"blend {name}", ["gdal", "raster", "blend", "-q", color, overlay, "{out}", *opts],
            ["blend", color, overlay, "{out}", *opts])

    blend("hsv-value RGB hillshade", rgb, hs, ["--operator", "hsv-value"])
    blend("hsv-value 50% opacity", rgb, hs, ["--operator", "hsv-value", "--opacity", "50"])
    blend("hsv-value RGBA", rgba4, hs, ["--operator", "hsv-value"])
    # GDAL's file output copies the input's first alpha row to every row (an opaque band here;
    # its per-band reads copy the whole band); Terrio copies the input's alpha band. The color
    # bands are compared.
    out[-1]["bands"] = [1, 2, 3]
    blend("src-over RGB gray 60%", rgb, hs, ["--opacity", "60"])
    blend("src-over RGBA RGBA 75%", rgba4, rgba4, ["--opacity", "75"])
    blend("src-over RGB RGBA", rgb, rgba4)
    blend("src-over gray alpha over RGB", gray_alpha, rgb, ["--opacity", "80"])
    blend("src-over gray RGBA", hs, rgba4, ["--opacity", "90"])
    if GDAL_313:
        rgb_nodata = os.path.join(tmp, "rgb_nodata.tif")
        gdal.Translate(rgb_nodata, f"{d}/landsat7.tif", srcWin=[0, 0, 400, 300])
        for name, opts in [
            ("nodata mask", []),
            ("no-dither", ["--no-dither"]),
            ("bit-depth 8", ["--bit-depth", "8", "--color-count", "32"]),
            ("output-nodata 255", ["--output-nodata", "255", "--color-count", "100"]),
        ]:
            add(f"rgb-to-palette gdal raster {name}", ["gdal", "raster", "rgb-to-palette", "-q", rgb_nodata, "{out}", *opts],
                ["rgb-to-palette", rgb_nodata, "{out}", *opts])
        add("rgb-to-palette gdal raster RGBA alpha", ["gdal", "raster", "rgb-to-palette", "-q", rgba4, "{out}"],
            ["rgb-to-palette", rgba4, "{out}"])
        for op in ("multiply", "screen", "overlay", "hard-light", "darken", "lighten", "color-burn", "color-dodge"):
            blend(f"{op} RGBA RGBA", rgba4, rgba, ["--operator", op, "--opacity", "80"])
        for op in ("multiply", "screen", "overlay", "hard-light", "color-burn", "color-dodge"):
            blend(f"{op} RGB gray", rgb, hs, ["--operator", op])
        blend("multiply gray over RGBA (swapped)", hs, rgba4, ["--operator", "multiply", "--opacity", "70"])
        blend("darken gray alpha", gray_alpha, gray_alpha, ["--operator", "darken"])
    return out


#: GDAL 3.13.3's autotest/utilities/test_gdalalg_raster_rgb_to_palette.py: input, options and
#: the output band checksum it expects.
GDAL_313_PALETTE = [
    ("nominal", "small_world.tif", [], 14890),
    ("no-dither", "small_world.tif", ["--no-dither"], 4419),
    ("bit-depth 8", "small_world.tif", ["--bit-depth", "8"], 7593),
    ("RGBA alpha mask", "stefan_full_rgba.tif", [], 19683),
    ("nodata 0 output-nodata 255", "stefan_rgb_nodata.tif", ["--output-nodata", "255"], 33355),
    ("nodata 0 output-nodata 0", "stefan_rgb_nodata.tif", ["--output-nodata", "0"], 27863),
]

GDAL_DATA_URL = "https://raw.githubusercontent.com/OSGeo/gdal/v3.13.3/autotest/"


def gdal_313_palette_cases(terrio, tmp):
    """Checks rgb-to-palette against GDAL 3.13.3's autotest checksums.

    Args:
        terrio: The terrio executable.
        tmp: The work directory.

    Returns:
        list: Results in the raster comparison's terms.
    """
    import urllib.request

    files = {}
    for name, sub in [("small_world.tif", "gdrivers/data"), ("stefan_full_rgba.tif", "gcore/data")]:
        files[name] = os.path.join(tmp, name)
        urllib.request.urlretrieve(f"{GDAL_DATA_URL}{sub}/{name}", files[name])
    files["stefan_rgb_nodata.tif"] = os.path.join(tmp, "stefan_rgb_nodata.tif")
    gdal.Translate(files["stefan_rgb_nodata.tif"], files["stefan_full_rgba.tif"], bandList=[1, 2, 3], noData=0)
    out = []
    for label, src, opts, expected in GDAL_313_PALETTE:
        path = os.path.join(tmp, f"autotest_{len(out)}.tif")
        r = {"name": f"rgb-to-palette GDAL 3.13 autotest {label}"}
        try:
            run([terrio, "rgb-to-palette", files[src], path, *opts])
            ds = gdal.Open(path)
            got = ds.GetRasterBand(1).Checksum()
            ok = got == expected
            r.update({"size": f"checksum {expected}", "same_size": True, "same_type": True, "same_nodata": True,
                      "same_crs": True, "same_metadata": True, "gt_diff_px": 0.0, "differing": 0 if ok else 1,
                      "exact": 1.0 if ok else 0.0, "within_tol": 1.0 if ok else 0.0, "max_diff": abs(got - expected),
                      "inf_diff": 0})
            ds = None
        except Exception as e:  # noqa: BLE001 - report and continue
            r["error"] = str(e).strip().splitlines()[-1][:300]
        out.append(r)
    return out


def raster_extra(gout, tout):
    """Compares what ``compare`` leaves out: CRS, color interpretations, scales and offsets.

    Args:
        gout: GDAL's output.
        tout: terrio's output.

    Returns:
        dict: ``same_crs`` and ``same_metadata``.
    """
    g, t = gdal.Open(gout), gdal.Open(tout)
    gs, ts = osr.SpatialReference(), osr.SpatialReference()
    same_crs = (not g.GetProjection() and not t.GetProjection()) or (
        bool(g.GetProjection()) and bool(t.GetProjection()) and gs.ImportFromWkt(g.GetProjection()) == 0
        and ts.ImportFromWkt(t.GetProjection()) == 0 and bool(gs.IsSame(ts))
    )

    def props(ds):
        out = []
        for i in range(1, ds.RasterCount + 1):
            b = ds.GetRasterBand(i)
            out.append((gdal.GetColorInterpretationName(b.GetColorInterpretation()), b.GetScale() or 1.0,
                        b.GetOffset() or 0.0))
        return out

    def table(ds):
        ct = ds.GetRasterBand(1).GetColorTable()
        return None if ct is None else [ct.GetColorEntry(i) for i in range(ct.GetCount())]

    gp, tp = props(g), props(t)
    if table(g) != table(t):
        gp.append(("color table", table(g) is not None))
        tp.append(("color table differs", table(t) is not None))
    res = {"same_crs": same_crs, "same_metadata": gp == tp}
    if gp != tp:
        res["metadata"] = f"GDAL {gp}, terrio {tp}"
    return res


def read_layer(path):
    """Reads a vector output's single layer.

    Args:
        path: The dataset.

    Returns:
        tuple: Field names, then per feature its field values and geometry coordinates.
    """
    ds = gdal.OpenEx(path, gdal.OF_VECTOR)
    layer = ds.GetLayer(0)
    defn = layer.GetLayerDefn()
    names = [defn.GetFieldDefn(i).GetName() for i in range(defn.GetFieldCount())]
    feats = []
    for f in layer:
        values = [f.GetField(i) for i in range(len(names))]
        g = f.GetGeometryRef()
        coords = []
        if g is not None:
            g = g.Clone()
            g.FlattenTo2D()
            if g.GetGeometryType() == ogr.wkbPolygon:
                coords = list(g.GetGeometryRef(0).GetPoints())
            else:
                coords = [(g.GetX(), g.GetY())]
        feats.append((values, coords))
    srs = layer.GetSpatialRef()
    return names, feats, layer.GetName(), srs


def compare_vector(gout, tout):
    """Compares two vector outputs feature by feature.

    Args:
        gout: GDAL's output.
        tout: terrio's output.

    Returns:
        dict: Comparison results in the raster comparison's terms (``differing`` features).
    """
    gn, gf, gl, gs = read_layer(gout)
    tn, tf, tl, ts = read_layer(tout)
    res = {"size": f"{len(gf)} features", "same_size": len(gf) == len(tf), "type": ",".join(gn),
           "same_type": gn == tn, "same_nodata": True, "gt_diff_px": 0.0,
           "same_crs": (gs is None and ts is None) or (gs is not None and ts is not None and bool(gs.IsSame(ts))),
           "same_metadata": gl == tl or gout.endswith(".csv")}
    if not res["same_size"] or not res["same_type"]:
        return res
    differing, max_diff = 0, 0.0
    for (gv, gc), (tv, tc) in zip(gf, tf):
        diff = 0.0
        same = len(gc) == len(tc)
        for a, b in zip(gv, tv):
            a, b = float(a), float(b)
            if not (a == b or (np.isnan(a) and np.isnan(b))):
                same = False
                diff = max(diff, abs(a - b))
        for a, b in zip(gc, tc):
            if a != b:
                same = False
                diff = max(diff, abs(a[0] - b[0]), abs(a[1] - b[1]))
        differing += not same
        max_diff = max(max_diff, diff)
    n = max(len(gf), 1)
    res.update({"differing": differing, "exact": 1 - differing / n, "within_tol": 1 - differing / n,
                "max_diff": max_diff, "inf_diff": 0})
    return res


def main():
    """Runs the cases and reports them."""
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("data")
    ap.add_argument("--terrio", default="target/release/terrio")
    ap.add_argument("--only", help="run the cases whose name contains this")
    ap.add_argument("--json", action="store_true", help="print the results as JSON")
    ap.add_argument("--keep", help="keep the outputs in this directory")
    a = ap.parse_args()
    tmp = a.keep or tempfile.mkdtemp(prefix="terrio_pixel_")
    os.makedirs(tmp, exist_ok=True)
    results = []
    try:
        for i, c in enumerate(cases(a.data, tmp)):
            if a.only and a.only not in c["name"]:
                continue
            gout = os.path.join(tmp, f"gdal_{i}.{c['ext']}")
            tout = os.path.join(tmp, f"terrio_{i}.{c['ext']}")
            for path in (gout, tout):
                if os.path.exists(path):
                    os.remove(path)
            try:
                run([x.replace("{out}", gout) for x in c["gdal"]])
                run([a.terrio] + [x.replace("{out}", tout) for x in c["terrio"]])
                if c["kind"] == "vector":
                    r = compare_vector(gout, tout)
                else:
                    if c.get("bands"):
                        for path in (gout, tout):
                            gdal.Translate(path + ".bands.tif", path, bandList=c["bands"])
                        gout, tout = gout + ".bands.tif", tout + ".bands.tif"
                    r = compare(gout, tout, c["tol"])
                    r.update(raster_extra(gout, tout))
            except Exception as e:  # noqa: BLE001 - report and continue
                r = {"error": str(e).strip().splitlines()[-1][:300]}
            r["name"] = c["name"]
            results.append(r)
            if not a.json:
                line = format_row(r)
                if "error" not in r:
                    line += f" crs={'ok' if r.get('same_crs') else 'DIFF'} meta={'ok' if r.get('same_metadata') else 'DIFF'}"
                    if r.get("metadata"):
                        line += f" ({r['metadata']})"
                print(line, flush=True)
        if not a.only or a.only in "rgb-to-palette GDAL 3.13 autotest":
            try:
                extra = gdal_313_palette_cases(a.terrio, tmp)
            except Exception as e:  # noqa: BLE001 - offline: report and continue
                extra = [{"name": "rgb-to-palette GDAL 3.13 autotest", "error": str(e)[:300]}]
            for r in extra:
                results.append(r)
                if not a.json:
                    print(format_row(r), flush=True)
    finally:
        if not a.keep:
            shutil.rmtree(tmp, ignore_errors=True)
    if a.json:
        print(json.dumps(results, indent=1))
    bad = [r for r in results if "error" in r or not r.get("same_size") or r.get("differing")]
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
