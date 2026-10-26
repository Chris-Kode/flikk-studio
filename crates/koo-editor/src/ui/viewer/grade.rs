//! Primary grade (PRD K1) on decoded RGBA, for preview. Mirrors the export
//! filters (`koo_deliver` `grade_filters`) in the same order: exposure and
//! gain, then contrast, gamma and saturation (`eq`), then lift and
//! temperature/tint (`colorbalance`), then per-channel gamma (`lutrgb`).
//! LUTs are applied on export only.

use koo_core::model::Grade;

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Temperature/tint: -1..1 (values beyond that are read as percent).
fn unit(v: f32) -> f32 {
    if v.abs() > 1.0 { v / 100.0 } else { v }.clamp(-1.0, 1.0)
}

/// Whether `g` changes the picture in preview.
pub fn is_neutral(g: &Grade) -> bool {
    let k = 2f32.powf(g.exposure);
    g.gain.iter().all(|v| near(v * k, 1.0))
        && near(g.contrast, 1.0)
        && near(g.saturation, 1.0)
        && near(g.gamma, 1.0)
        && near(unit(g.temperature), 0.0)
        && near(unit(g.tint), 0.0)
        && g.lift.iter().all(|v| near(*v, 0.0))
        && g.gamma_rgb.iter().all(|v| near(*v, 1.0))
}

/// Grade RGBA pixels in place.
pub fn apply(rgba: &mut [u8], g: &Grade) {
    if is_neutral(g) {
        return;
    }
    let k = 2f32.powf(g.exposure);
    let gain: [[f32; 256]; 3] = std::array::from_fn(|c| {
        std::array::from_fn(|i| (i as f32 / 255.0 * g.gain[c] * k).clamp(0.0, 1.0))
    });
    // `eq` works on 8-bit BT.601 limited-range luma; saturation scales chroma.
    let contrast = g.contrast.clamp(-1000.0, 1000.0);
    let gamma = 1.0 / g.gamma.clamp(0.1, 10.0);
    let luma: [f32; 256] = std::array::from_fn(|i| {
        let v = contrast * (i as f32 / 255.0 - 0.5) + 0.5;
        if v <= 0.0 {
            0.0
        } else {
            (256.0 * v.powf(gamma)).min(255.0).floor()
        }
    });
    let saturation = g.saturation.clamp(0.0, 3.0);
    let eq = !near(contrast, 1.0) || !near(saturation, 1.0) || !near(gamma, 1.0);
    let post: [[u8; 256]; 3] = std::array::from_fn(|c| {
        let p = 1.0 / g.gamma_rgb[c].clamp(0.1, 10.0);
        std::array::from_fn(|i| ((i as f32 / 255.0).powf(p) * 255.0).round() as u8)
    });
    let (temp, tint) = (unit(g.temperature), unit(g.tint));
    let shadows = g.lift.map(|v| v.clamp(-1.0, 1.0));
    let midtones = [temp * 0.3, -tint * 0.3, -temp * 0.3];
    let balance = shadows.iter().chain(&midtones).any(|v| !near(*v, 0.0));

    for px in rgba.as_chunks_mut::<4>().0 {
        let mut c = [
            gain[0][px[0] as usize],
            gain[1][px[1] as usize],
            gain[2][px[2] as usize],
        ];
        if eq {
            let [r, g, b] = c;
            let y = 16.0 + 219.0 * (0.299 * r + 0.587 * g + 0.114 * b);
            let cb = (-0.168736 * r - 0.331264 * g + 0.5 * b) * saturation;
            let cr = (0.5 * r - 0.418688 * g - 0.081312 * b) * saturation;
            let y = (luma[y.round().clamp(0.0, 255.0) as usize] - 16.0) / 219.0;
            c = [
                y + 1.402 * cr,
                y - 0.344136 * cb - 0.714136 * cr,
                y + 1.772 * cb,
            ]
            .map(|v| v.clamp(0.0, 1.0));
        }
        if balance {
            // ffmpeg's colorbalance weights, keyed on max + min.
            let l = c[0].max(c[1]).max(c[2]) + c[0].min(c[1]).min(c[2]);
            let (a, b, scale) = (4.0, 0.333, 0.7);
            let ws = ((b - l) * a + 0.5).clamp(0.0, 1.0) * scale;
            let wm = ((l - b) * a + 0.5).clamp(0.0, 1.0)
                * ((1.0 - l - b) * a + 0.5).clamp(0.0, 1.0)
                * scale;
            for i in 0..3 {
                c[i] = (c[i] + shadows[i] * ws + midtones[i] * wm).clamp(0.0, 1.0);
            }
        }
        for i in 0..3 {
            px[i] = post[i][(c[i] * 255.0).round() as usize];
        }
    }
}
