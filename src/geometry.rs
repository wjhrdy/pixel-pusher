use anyhow::{Result, bail};
use image::{Rgb, RgbImage};
use serde::Serialize;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Quad {
    /// Top-left, top-right, bottom-right, bottom-left.
    pub points: [Point; 4],
}

impl FromStr for Quad {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let parsed: std::result::Result<Vec<Point>, String> = value
            .split(';')
            .map(|pair| {
                let coordinates: Vec<&str> = pair.split(',').collect();
                if coordinates.len() != 2 {
                    return Err(format!("expected x,y, got {pair:?}"));
                }
                let x = coordinates[0]
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("invalid x coordinate in {pair:?}"))?;
                let y = coordinates[1]
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("invalid y coordinate in {pair:?}"))?;
                if !x.is_finite() || !y.is_finite() {
                    return Err("corner coordinates must be finite".to_string());
                }
                Ok(Point { x, y })
            })
            .collect();
        let points: [Point; 4] = parsed?
            .try_into()
            .map_err(|points: Vec<Point>| format!("expected four corners, got {}", points.len()))?;
        Ok(Self { points })
    }
}

#[derive(Clone, Copy, Debug)]
struct Homography {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
    g: f64,
    h: f64,
}

impl Homography {
    /// Map the unit square to an arbitrary quadrilateral.
    fn unit_square_to(quad: Quad) -> Result<Self> {
        let [p0, p1, p2, p3] = quad.points;
        let dx3 = p0.x - p1.x + p2.x - p3.x;
        let dy3 = p0.y - p1.y + p2.y - p3.y;
        if dx3.abs() < 1e-12 && dy3.abs() < 1e-12 {
            return Ok(Self {
                a: p1.x - p0.x,
                b: p3.x - p0.x,
                c: p0.x,
                d: p1.y - p0.y,
                e: p3.y - p0.y,
                f: p0.y,
                g: 0.0,
                h: 0.0,
            });
        }

        let dx1 = p1.x - p2.x;
        let dx2 = p3.x - p2.x;
        let dy1 = p1.y - p2.y;
        let dy2 = p3.y - p2.y;
        let denominator = dx1 * dy2 - dx2 * dy1;
        if denominator.abs() < 1e-12 {
            bail!("the supplied corners do not define a valid quadrilateral");
        }
        let g = (dx3 * dy2 - dx2 * dy3) / denominator;
        let h = (dx1 * dy3 - dx3 * dy1) / denominator;
        Ok(Self {
            a: p1.x - p0.x + g * p1.x,
            b: p3.x - p0.x + h * p3.x,
            c: p0.x,
            d: p1.y - p0.y + g * p1.y,
            e: p3.y - p0.y + h * p3.y,
            f: p0.y,
            g,
            h,
        })
    }

    fn map(self, u: f64, v: f64) -> Point {
        let denominator = self.g * u + self.h * v + 1.0;
        Point {
            x: (self.a * u + self.b * v + self.c) / denominator,
            y: (self.d * u + self.e * v + self.f) / denominator,
        }
    }
}

fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

pub fn estimated_size(quad: Quad) -> (u32, u32) {
    let [top_left, top_right, bottom_right, bottom_left] = quad.points;
    let width = ((distance(top_left, top_right) + distance(bottom_left, bottom_right)) * 0.5)
        .round()
        .max(1.0) as u32;
    let height = ((distance(top_left, bottom_left) + distance(top_right, bottom_right)) * 0.5)
        .round()
        .max(1.0) as u32;
    (width, height)
}

fn bilinear(image: &RgbImage, point: Point) -> Rgb<u8> {
    // Corner coordinates use image-edge space; pixel centers lie at n + 0.5.
    let x = (point.x - 0.5).clamp(0.0, image.width().saturating_sub(1) as f64);
    let y = (point.y - 0.5).clamp(0.0, image.height().saturating_sub(1) as f64);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let tx = x - x0 as f64;
    let ty = y - y0 as f64;
    let samples = [
        (image.get_pixel(x0, y0), (1.0 - tx) * (1.0 - ty)),
        (image.get_pixel(x1, y0), tx * (1.0 - ty)),
        (image.get_pixel(x0, y1), (1.0 - tx) * ty),
        (image.get_pixel(x1, y1), tx * ty),
    ];
    Rgb(std::array::from_fn(|channel| {
        samples
            .iter()
            .map(|(pixel, weight)| pixel[channel] as f64 * weight)
            .sum::<f64>()
            .round() as u8
    }))
}

/// Rectify a photographed quadrilateral into an axis-aligned image. Sampling
/// pixel centers in destination grid space mirrors the grid-sampler stage used
/// by QR readers, while retaining full RGB information.
pub fn rectify(source: &RgbImage, quad: Quad, width: u32, height: u32) -> Result<RgbImage> {
    if source.width() == 0 || source.height() == 0 || width == 0 || height == 0 {
        bail!("source and rectified dimensions must be non-zero");
    }
    let transform = Homography::unit_square_to(quad)?;
    Ok(RgbImage::from_fn(width, height, |x, y| {
        let u = (x as f64 + 0.5) / width as f64;
        let v = (y as f64 + 0.5) / height as f64;
        bilinear(source, transform.map(u, v))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_maps_all_four_corners() {
        let quad = Quad {
            points: [
                Point { x: 3.0, y: 5.0 },
                Point { x: 31.0, y: 2.0 },
                Point { x: 27.0, y: 25.0 },
                Point { x: 7.0, y: 29.0 },
            ],
        };
        let transform = Homography::unit_square_to(quad).unwrap();
        for ((u, v), expected) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .into_iter()
            .zip(quad.points)
        {
            let actual = transform.map(u, v);
            assert!((actual.x - expected.x).abs() < 1e-9);
            assert!((actual.y - expected.y).abs() < 1e-9);
        }
    }

    #[test]
    fn parses_corners_in_clockwise_order() {
        let quad: Quad = "1,2; 9,3; 8,7; 0,6".parse().unwrap();
        assert_eq!(quad.points[2].x, 8.0);
        assert_eq!(quad.points[3].y, 6.0);
    }
}
