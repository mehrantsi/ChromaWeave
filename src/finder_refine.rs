use crate::geometry::Homography;
use image::RgbImage;

pub(crate) fn refine_center(image: &RgbImage, quad: [[f64; 2]; 4]) -> Option<[f64; 2]> {
    if image.width() < 2 || image.height() < 2 {
        return None;
    }
    let transform = Homography::from_quad([[0., 0.], [1., 0.], [1., 1.], [0., 1.]], quad).ok()?;
    let initial = transform.transform([0.5, 0.5])?;
    let image_to_finder = transform.inverse().ok()?;
    let u = std::array::from_fn::<_, 2, _>(|d| {
        (quad[1][d] - quad[0][d] + quad[2][d] - quad[3][d]) / 14.0
    });
    let v = std::array::from_fn::<_, 2, _>(|d| {
        (quad[3][d] - quad[0][d] + quad[2][d] - quad[1][d]) / 14.0
    });
    let pitch = u[0].hypot(u[1]).min(v[0].hypot(v[1]));
    if pitch < 0.5 {
        return None;
    }
    let mut offsets = Vec::with_capacity(112);
    for y in -7..=7 {
        for x in -7..=7 {
            if y < 0 || (y == 0 && x <= 0) {
                continue;
            }
            offsets.push([
                (u[0] * x as f64 + v[0] * y as f64) * 0.5,
                (u[1] * x as f64 + v[1] * y as f64) * 0.5,
            ]);
        }
    }
    let mut best = None;
    let mut best_cost = f64::INFINITY;
    for oy in -1..=1 {
        for ox in -1..=1 {
            let candidate = [
                initial[0] + ox as f64 * pitch * 0.6,
                initial[1] + oy as f64 * pitch * 0.6,
            ];
            if let Some(center) = fit_center(image, &offsets, &image_to_finder, candidate, pitch) {
                let Some(cost) = symmetry_cost(image, &offsets, center) else {
                    continue;
                };
                if cost < best_cost {
                    best_cost = cost;
                    best = Some(center);
                }
            }
        }
    }
    best
}

fn symmetry_cost(image: &RgbImage, offsets: &[[f64; 2]], center: [f64; 2]) -> Option<f64> {
    let mut cost = 0.0;
    for offset in offsets {
        let a = sample(image, center[0] + offset[0], center[1] + offset[1])?;
        let b = sample(image, center[0] - offset[0], center[1] - offset[1])?;
        cost += (a[0] - b[0]).powi(2);
    }
    Some(cost)
}

fn fit_center(
    image: &RgbImage,
    offsets: &[[f64; 2]],
    image_to_finder: &Homography,
    mut center: [f64; 2],
    pitch: f64,
) -> Option<[f64; 2]> {
    for _ in 0..6 {
        let (mut xx, mut xy, mut yy, mut xr, mut yr) = (0., 0., 0., 0., 0.);
        for offset in offsets {
            let a = sample(image, center[0] + offset[0], center[1] + offset[1])?;
            let b = sample(image, center[0] - offset[0], center[1] - offset[1])?;
            let r = a[0] - b[0];
            let gx = a[1] - b[1];
            let gy = a[2] - b[2];
            xx += gx * gx;
            xy += gx * gy;
            yy += gy * gy;
            xr += gx * r;
            yr += gy * r;
        }
        let determinant = xx * yy - xy * xy;
        if determinant <= 1e-6 {
            return None;
        }
        let mut dx = (yy * xr - xy * yr) / determinant;
        let mut dy = (xx * yr - xy * xr) / determinant;
        let length = dx.hypot(dy);
        let bound = pitch * 0.3;
        if length > bound {
            dx *= bound / length;
            dy *= bound / length;
        }
        center[0] -= dx;
        center[1] -= dy;
        let canonical = image_to_finder.transform(center)?;
        if !canonical
            .iter()
            .all(|coordinate| (2.0 / 7.0..=5.0 / 7.0).contains(coordinate))
        {
            return None;
        }
        if length < 0.005 {
            break;
        }
    }
    Some(center)
}

fn sample(image: &RgbImage, x: f64, y: f64) -> Option<[f64; 3]> {
    if x < 0.0 || y < 0.0 || x >= f64::from(image.width() - 1) || y >= f64::from(image.height() - 1)
    {
        return None;
    }
    let ix = x as u32;
    let iy = y as u32;
    let fx = x - f64::from(ix);
    let fy = y - f64::from(iy);
    let lum = |x, y| {
        let p = image.get_pixel(x, y).0;
        (f64::from(p[0]) + 2.0 * f64::from(p[1]) + f64::from(p[2])) * 0.25
    };
    let a = lum(ix, iy);
    let b = lum(ix + 1, iy);
    let c = lum(ix, iy + 1);
    let d = lum(ix + 1, iy + 1);
    Some([
        (a + (b - a) * fx) * (1.0 - fy) + (c + (d - c) * fx) * fy,
        (b - a) * (1.0 - fy) + (d - c) * fy,
        (c - a) * (1.0 - fx) + (d - b) * fx,
    ])
}

#[cfg(test)]
#[path = "tests/finder_refine.rs"]
mod tests;
