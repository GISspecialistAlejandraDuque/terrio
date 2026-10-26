"""Color tables and blending: blend, rgb_to_palette, pct2rgb and attach_pct."""

import numpy as np
import pytest

import terrio

GT = (100.0, 10.0, 0.0, 50.0, 0.0, -10.0)


def test_attach_pct_and_pct2rgb_round_trip(tmp_path):
    src = terrio.Raster.from_array(np.array([[0, 1, 2]], dtype="uint8"), GT, crs="EPSG:32611")
    path = str(tmp_path / "paletted.tif")
    table = [(10, 20, 30), (40, 50, 60, 0), (70, 80, 90)]
    paletted = terrio.attach_pct(table, src, path)
    assert paletted.colormap[:3] == [(10, 20, 30, 255), (40, 50, 60, 0), (70, 80, 90, 255)]
    reread = terrio.open(path)
    assert reread.colormap[:3] == [(10, 20, 30, 255), (40, 50, 60, 255), (70, 80, 90, 255)]
    rgb = terrio.pct2rgb(paletted)
    assert rgb.count == 3
    np.testing.assert_array_equal(rgb.read(2), [[20, 50, 80]])
    rgba = terrio.pct2rgb(paletted, rgba=True)
    np.testing.assert_array_equal(rgba.read(4), [[255, 0, 255]])
    palette = tmp_path / "palette.txt"
    palette.write_text("0 255 0 0\n2 0 0 255\n")
    np.testing.assert_array_equal(terrio.pct2rgb(src, pct=str(palette)).read(1), [[255, 255, 0]])
    with pytest.raises(ValueError, match="color table"):
        terrio.pct2rgb(src)


def test_rgb_to_palette_exact_colors():
    rgb = np.array([[[10, 200, 10]], [[20, 100, 20]], [[30, 0, 30]]], dtype="uint8")
    src = terrio.Raster.from_array(rgb, GT)
    out = terrio.rgb_to_palette(src, color_count=2, bit_depth=8, dither=False)
    assert out.count == 1 and out.dtype == "uint8"
    table = out.colormap
    np.testing.assert_array_equal(
        [table[i][:3] for i in out.read(1)[0]], [(10, 20, 30), (200, 100, 0), (10, 20, 30)]
    )
    fixed = terrio.rgb_to_palette(src, color_map=[(0, 0, 0), (200, 100, 0)], dither=False)
    np.testing.assert_array_equal(fixed.read(1), [[0, 1, 0]])
    with pytest.raises(ValueError, match="color count"):
        terrio.rgb_to_palette(src, color_count=1)


def test_blend_operators():
    color = terrio.Raster.from_array(np.full((4, 1, 1), 128, dtype="uint8"), GT)
    overlay = terrio.Raster.from_array(np.full((4, 1, 1), 128, dtype="uint8"), GT)
    # GDAL 3.13.3's autotest: multiply of two half-transparent mid grays.
    out = terrio.blend(color, overlay, operator="multiply")
    assert [int(out.read(b)[0, 0]) for b in range(1, 5)] == [106, 106, 106, 192]
    gray = terrio.Raster.from_array(np.full((1, 1, 1), 90, dtype="uint8"), GT)
    rgb = terrio.Raster.from_array(np.full((3, 1, 1), 50, dtype="uint8"), GT)
    assert int(terrio.blend(rgb, gray, operator="hsv-value").read(1)[0, 0]) == 90
    with pytest.raises(ValueError, match="operator"):
        terrio.blend(rgb, gray, operator="xor")
