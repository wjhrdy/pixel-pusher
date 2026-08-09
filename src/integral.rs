use image::RgbImage;

#[derive(Clone, Copy, Debug, Default)]
pub struct Moments {
    pub sum: [f64; 3],
    pub sum_sq: [f64; 3],
    pub area: f64,
}

impl Moments {
    fn add_scaled(&mut self, other: Self, scale: f64) {
        for channel in 0..3 {
            self.sum[channel] += other.sum[channel] * scale;
            self.sum_sq[channel] += other.sum_sq[channel] * scale;
        }
        self.area += other.area * scale;
    }

    pub fn mean(self) -> [f64; 3] {
        if self.area <= 0.0 {
            return [0.0; 3];
        }
        self.sum.map(|value| value / self.area)
    }

    pub fn sse(self) -> f64 {
        if self.area <= 0.0 {
            return 0.0;
        }
        (0..3)
            .map(|channel| {
                (self.sum_sq[channel] - self.sum[channel] * self.sum[channel] / self.area).max(0.0)
            })
            .sum()
    }
}

/// Summed-area RGB and RGB² tables. Pixels are treated as unit squares with a
/// constant value, which makes fractional rectangle queries exact.
pub struct IntegralImage {
    width: usize,
    height: usize,
    table: Vec<Moments>,
}

impl IntegralImage {
    pub fn new(image: &RgbImage) -> Self {
        let width = image.width() as usize;
        let height = image.height() as usize;
        let stride = width + 1;
        let mut table = vec![Moments::default(); (width + 1) * (height + 1)];

        for y in 0..height {
            let mut row_sum = [0.0; 3];
            let mut row_sum_sq = [0.0; 3];
            for x in 0..width {
                let pixel = image.get_pixel(x as u32, y as u32).0;
                for channel in 0..3 {
                    let value = pixel[channel] as f64 / 255.0;
                    row_sum[channel] += value;
                    row_sum_sq[channel] += value * value;
                }
                let above = table[y * stride + x + 1];
                let entry = &mut table[(y + 1) * stride + x + 1];
                for channel in 0..3 {
                    entry.sum[channel] = above.sum[channel] + row_sum[channel];
                    entry.sum_sq[channel] = above.sum_sq[channel] + row_sum_sq[channel];
                }
                entry.area = ((y + 1) * (x + 1)) as f64;
            }
        }

        Self {
            width,
            height,
            table,
        }
    }

    fn at(&self, x: usize, y: usize) -> Moments {
        self.table[y * (self.width + 1) + x]
    }

    /// Integral over [0, x) × [0, y), including fractional edge-pixel coverage.
    fn prefix(&self, x: f64, y: f64) -> Moments {
        let x = x.clamp(0.0, self.width as f64);
        let y = y.clamp(0.0, self.height as f64);
        let ix = x.floor() as usize;
        let iy = y.floor() as usize;
        let fx = x - ix as f64;
        let fy = y - iy as f64;

        let mut result = self.at(ix, iy);
        if fx > 0.0 && ix < self.width {
            let mut column = self.at(ix + 1, iy);
            column.add_scaled(self.at(ix, iy), -1.0);
            result.add_scaled(column, fx);
        }
        if fy > 0.0 && iy < self.height {
            let mut row = self.at(ix, iy + 1);
            row.add_scaled(self.at(ix, iy), -1.0);
            result.add_scaled(row, fy);
        }
        if fx > 0.0 && fy > 0.0 && ix < self.width && iy < self.height {
            let mut pixel = self.at(ix + 1, iy + 1);
            pixel.add_scaled(self.at(ix, iy + 1), -1.0);
            pixel.add_scaled(self.at(ix + 1, iy), -1.0);
            pixel.add_scaled(self.at(ix, iy), 1.0);
            result.add_scaled(pixel, fx * fy);
        }
        result.area = x * y;
        result
    }

    pub fn rect(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Moments {
        let x0 = x0.clamp(0.0, self.width as f64);
        let y0 = y0.clamp(0.0, self.height as f64);
        let x1 = x1.clamp(x0, self.width as f64);
        let y1 = y1.clamp(y0, self.height as f64);
        let mut result = self.prefix(x1, y1);
        result.add_scaled(self.prefix(x0, y1), -1.0);
        result.add_scaled(self.prefix(x1, y0), -1.0);
        result.add_scaled(self.prefix(x0, y0), 1.0);
        result.area = (x1 - x0) * (y1 - y0);
        result
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn fractional_query_uses_exact_pixel_area() {
        let image = RgbImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        });
        let integral = IntegralImage::new(&image);
        let moments = integral.rect(0.5, 0.0, 1.5, 1.0);
        let mean = moments.mean();
        assert!((mean[0] - 0.5).abs() < 1e-9);
        assert!((mean[2] - 0.5).abs() < 1e-9);
    }
}
