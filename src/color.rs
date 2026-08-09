/// Convert nonlinear sRGB (0–1) to Oklab for perceptual palette clustering.
pub fn srgb_to_oklab(rgb: [f64; 3]) -> [f64; 3] {
    let linear = rgb.map(|value| {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    });
    let l = 0.412_221_470_8 * linear[0] + 0.536_332_536_3 * linear[1] + 0.051_445_992_9 * linear[2];
    let m = 0.211_903_498_2 * linear[0] + 0.680_699_545_1 * linear[1] + 0.107_396_956_6 * linear[2];
    let s = 0.088_302_461_9 * linear[0] + 0.281_718_837_6 * linear[1] + 0.629_978_700_5 * linear[2];
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s,
    ]
}

pub fn distance_squared(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
}

pub fn rgb8(rgb: [f64; 3]) -> [u8; 3] {
    rgb.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
}
