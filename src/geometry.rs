use anyhow::{Result, ensure};
use image::{Rgb, RgbImage};
use serde::{Deserialize, Serialize};

pub type Point = [f64; 2];

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Homography(pub [[f64; 3]; 3]);

impl Homography {
    pub fn from_quad(source: [Point; 4], destination: [Point; 4]) -> Result<Self> {
        ensure!(
            source
                .iter()
                .chain(destination.iter())
                .flatten()
                .all(|v| v.is_finite()),
            "homography points must be finite"
        );
        let mut equations = [[0.0; 9]; 8];
        for (index, ([x, y], [u, v])) in source.into_iter().zip(destination).enumerate() {
            equations[index * 2] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            equations[index * 2 + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        for column in 0..8 {
            let pivot = (column..8)
                .max_by(|&a, &b| {
                    equations[a][column]
                        .abs()
                        .total_cmp(&equations[b][column].abs())
                })
                .unwrap();
            ensure!(
                equations[pivot][column].abs() > 1e-12,
                "degenerate homography"
            );
            equations.swap(column, pivot);
            let divisor = equations[column][column];
            for entry in &mut equations[column][column..] {
                *entry /= divisor;
            }
            let pivot_row = equations[column];
            for (index, row) in equations.iter_mut().enumerate() {
                if index == column {
                    continue;
                }
                let factor = row[column];
                for (entry, &pivot) in row[column..].iter_mut().zip(&pivot_row[column..]) {
                    *entry -= factor * pivot;
                }
            }
        }
        let values: [f64; 8] = std::array::from_fn(|i| equations[i][8]);
        ensure!(values.iter().all(|v| v.is_finite()), "nonfinite homography");
        Ok(Self([
            [values[0], values[1], values[2]],
            [values[3], values[4], values[5]],
            [values[6], values[7], 1.0],
        ]))
    }

    pub fn transform(&self, [x, y]: Point) -> Option<Point> {
        let denominator = self.0[2][0] * x + self.0[2][1] * y + self.0[2][2];
        if !denominator.is_finite() || denominator.abs() < 1e-12 {
            return None;
        }
        let mapped = [
            (self.0[0][0] * x + self.0[0][1] * y + self.0[0][2]) / denominator,
            (self.0[1][0] * x + self.0[1][1] * y + self.0[1][2]) / denominator,
        ];
        mapped.iter().all(|v| v.is_finite()).then_some(mapped)
    }

    pub fn inverse(&self) -> Result<Self> {
        let [[a, b, c], [d, e, f], [g, h, i]] = self.0;
        let cofactors = [
            [e * i - f * h, c * h - b * i, b * f - c * e],
            [f * g - d * i, a * i - c * g, c * d - a * f],
            [d * h - e * g, b * g - a * h, a * e - b * d],
        ];
        let determinant = a * cofactors[0][0] + b * cofactors[1][0] + c * cofactors[2][0];
        ensure!(
            determinant.is_finite() && determinant.abs() > 1e-12,
            "singular homography"
        );
        Ok(Self(
            cofactors.map(|row| row.map(|value| value / determinant)),
        ))
    }
}

pub fn bilinear(image: &RgbImage, x: f64, y: f64) -> Rgb<u8> {
    let x = x.clamp(0.0, (image.width() - 1) as f64);
    let y = y.clamp(0.0, (image.height() - 1) as f64);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let dx = x - x0 as f64;
    let dy = y - y0 as f64;
    Rgb(std::array::from_fn(|channel| {
        let top = image.get_pixel(x0, y0)[channel] as f64 * (1.0 - dx)
            + image.get_pixel(x1, y0)[channel] as f64 * dx;
        let bottom = image.get_pixel(x0, y1)[channel] as f64 * (1.0 - dx)
            + image.get_pixel(x1, y1)[channel] as f64 * dx;
        (top * (1.0 - dy) + bottom * dy).round().clamp(0.0, 255.0) as u8
    }))
}
