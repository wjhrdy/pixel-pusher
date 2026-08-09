use crate::{grid::Candidate, integral::IntegralImage};
use rayon::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct WarpOptions {
    pub patch_size: u32,
    pub radius: f64,
    pub step: f64,
    pub smoothness: f64,
}

/// A smooth displacement field applied to the otherwise regular logical grid.
/// Values describe how far grid lines move in source-image pixels.
pub struct WarpField {
    width: u32,
    height: u32,
    spacing: f64,
    columns: usize,
    rows: usize,
    offsets: Vec<[f64; 2]>,
}

impl WarpField {
    pub fn displacement(&self, x: f64, y: f64) -> [f64; 2] {
        let grid_x = (x / self.spacing).clamp(0.0, (self.columns - 1) as f64);
        let grid_y = (y / self.spacing).clamp(0.0, (self.rows - 1) as f64);
        let x0 = grid_x.floor() as usize;
        let y0 = grid_y.floor() as usize;
        let x1 = (x0 + 1).min(self.columns - 1);
        let y1 = (y0 + 1).min(self.rows - 1);
        let tx = grid_x - x0 as f64;
        let ty = grid_y - y0 as f64;
        let samples = [
            (
                self.offsets[y0 * self.columns + x0],
                (1.0 - tx) * (1.0 - ty),
            ),
            (self.offsets[y0 * self.columns + x1], tx * (1.0 - ty)),
            (self.offsets[y1 * self.columns + x0], (1.0 - tx) * ty),
            (self.offsets[y1 * self.columns + x1], tx * ty),
        ];
        std::array::from_fn(|axis| {
            samples
                .iter()
                .map(|(offset, weight)| offset[axis] * weight)
                .sum()
        })
    }

    pub fn max_displacement(&self) -> f64 {
        self.offsets
            .iter()
            .map(|offset| offset[0].hypot(offset[1]))
            .fold(0.0, f64::max)
    }

    pub fn rms_displacement(&self) -> f64 {
        (self
            .offsets
            .iter()
            .map(|offset| offset[0] * offset[0] + offset[1] * offset[1])
            .sum::<f64>()
            / self.offsets.len() as f64)
            .sqrt()
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

fn local_score(
    integral: &IntegralImage,
    grid: Candidate,
    center: [f64; 2],
    patch_size: f64,
    inset: f64,
    offset: [f64; 2],
) -> f64 {
    let [center_x, center_y] = center;
    let block_width = grid.cell_width;
    let block_height = grid.cell_height;
    let phase_x = grid.phase_x + offset[0];
    let phase_y = grid.phase_y + offset[1];
    let half = patch_size * 0.5;
    let patch_x0 = (center_x - half).max(0.0);
    let patch_y0 = (center_y - half).max(0.0);
    let patch_x1 = (center_x + half).min(integral.width() as f64);
    let patch_y1 = (center_y + half).min(integral.height() as f64);
    let cell_x0 = ((patch_x0 - phase_x) / block_width - 0.5).ceil() as i32;
    let cell_x1 = ((patch_x1 - phase_x) / block_width - 0.5).floor() as i32;
    let cell_y0 = ((patch_y0 - phase_y) / block_height - 0.5).ceil() as i32;
    let cell_y1 = ((patch_y1 - phase_y) / block_height - 0.5).floor() as i32;
    let margin_x = block_width * inset;
    let margin_y = block_height * inset;
    let mut loss_sum = 0.0;
    let mut cell_count = 0.0;

    for cell_y in cell_y0..=cell_y1 {
        let outer_y0 = phase_y + cell_y as f64 * block_height;
        let outer_y1 = outer_y0 + block_height;
        if outer_y0 < 0.0 || outer_y1 > integral.height() as f64 {
            continue;
        }
        for cell_x in cell_x0..=cell_x1 {
            let outer_x0 = phase_x + cell_x as f64 * block_width;
            let outer_x1 = outer_x0 + block_width;
            if outer_x0 < 0.0 || outer_x1 > integral.width() as f64 {
                continue;
            }
            let moments = integral.rect(
                outer_x0 + margin_x,
                outer_y0 + margin_y,
                outer_x1 - margin_x,
                outer_y1 - margin_y,
            );
            if moments.area > 0.0 {
                let cell_loss = moments.sse() / (moments.area * 3.0);
                loss_sum += cell_loss;
                cell_count += 1.0;
            }
        }
    }
    if cell_count > 0.0 {
        loss_sum / cell_count
    } else {
        f64::INFINITY
    }
}

pub fn fit_local_warp(
    integral: &IntegralImage,
    grid: Candidate,
    inset: f64,
    options: WarpOptions,
) -> WarpField {
    let spacing = options.patch_size.max(8) as f64;
    let columns = (integral.width() as f64 / spacing).ceil() as usize + 1;
    let rows = (integral.height() as f64 / spacing).ceil() as usize + 1;
    let positions: Vec<(usize, usize)> = (0..rows)
        .flat_map(|row| (0..columns).map(move |column| (column, row)))
        .collect();
    let steps = (options.radius / options.step).ceil() as i32;
    let raw: Vec<[f64; 2]> = positions
        .par_iter()
        .map(|&(column, row)| {
            let center_x = (column as f64 * spacing).min(integral.width() as f64);
            let center_y = (row as f64 * spacing).min(integral.height() as f64);
            let mut best = [0.0, 0.0];
            let mut best_score =
                local_score(integral, grid, [center_x, center_y], spacing, inset, best);
            for dy in -steps..=steps {
                for dx in -steps..=steps {
                    let offset = [
                        (dx as f64 * options.step).clamp(-options.radius, options.radius),
                        (dy as f64 * options.step).clamp(-options.radius, options.radius),
                    ];
                    let score =
                        local_score(integral, grid, [center_x, center_y], spacing, inset, offset);
                    if score < best_score {
                        best = offset;
                        best_score = score;
                    }
                }
            }
            best
        })
        .collect();

    // Minimize data fidelity plus a membrane (neighbor-difference) penalty.
    // Retaining the raw term on every iteration prevents the field collapsing.
    let mut offsets = raw.clone();
    for _ in 0..20 {
        let previous = offsets;
        offsets = (0..raw.len())
            .map(|index| {
                let column = index % columns;
                let row = index / columns;
                let mut neighbor_sum = [0.0; 2];
                let mut neighbor_count = 0.0;
                for (dx, dy) in [(-1_i32, 0_i32), (1, 0), (0, -1), (0, 1)] {
                    let nx = column as i32 + dx;
                    let ny = row as i32 + dy;
                    if nx >= 0 && ny >= 0 && nx < columns as i32 && ny < rows as i32 {
                        let neighbor = previous[ny as usize * columns + nx as usize];
                        neighbor_sum[0] += neighbor[0];
                        neighbor_sum[1] += neighbor[1];
                        neighbor_count += 1.0;
                    }
                }
                if neighbor_count == 0.0 {
                    return raw[index];
                }
                std::array::from_fn(|axis| {
                    let neighbor_mean = neighbor_sum[axis] / neighbor_count;
                    (raw[index][axis] + options.smoothness * neighbor_mean)
                        / (1.0 + options.smoothness)
                })
            })
            .collect();
    }

    WarpField {
        width: integral.width() as u32,
        height: integral.height() as u32,
        spacing,
        columns,
        rows,
        offsets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_of_constant_field_is_constant() {
        let field = WarpField {
            width: 100,
            height: 80,
            spacing: 50.0,
            columns: 3,
            rows: 3,
            offsets: vec![[1.25, -0.75]; 9],
        };
        let actual = field.displacement(37.0, 61.0);
        assert!((actual[0] - 1.25).abs() < 1e-12);
        assert!((actual[1] + 0.75).abs() < 1e-12);
    }
}
