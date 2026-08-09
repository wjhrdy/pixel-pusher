use crate::{
    color::{distance_squared, srgb_to_oklab},
    grid::Candidate,
    integral::{IntegralImage, Moments},
    palette::CellColor,
    warp::WarpField,
};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub struct CellWarpOptions {
    pub radius: f64,
    pub step: f64,
    pub movement_penalty: f64,
    pub min_improvement: f64,
    pub min_variance: f64,
    pub contrast_threshold: f64,
    pub min_contrast_gain: f64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct CellWarpReport {
    pub search_radius: f64,
    pub search_step: f64,
    pub movement_penalty: f64,
    pub min_improvement: f64,
    pub min_variance: f64,
    pub contrast_threshold: f64,
    pub min_contrast_gain: f64,
    pub eligible_cells: u64,
    pub shifted_cells: u64,
    pub mean_variance_reduction: f64,
    pub rms_displacement: f64,
    pub max_displacement: f64,
}

pub struct CellWarpResult {
    pub cells: Vec<CellColor>,
    pub offsets: HashMap<(i32, i32), [f64; 2]>,
    pub report: CellWarpReport,
}

#[derive(Clone)]
struct RefinedCell {
    cell: CellColor,
    offset: [f64; 2],
    eligible: bool,
    variance_reduction: f64,
}

fn sample_moments(
    integral: &IntegralImage,
    grid: Candidate,
    inset: f64,
    warp: Option<&WarpField>,
    cell: &CellColor,
    residual: [f64; 2],
) -> Option<Moments> {
    let nominal_x = grid.phase_x + (cell.cell_x as f64 + 0.5) * grid.cell_width;
    let nominal_y = grid.phase_y + (cell.cell_y as f64 + 0.5) * grid.cell_height;
    let smooth = warp
        .map(|field| field.displacement(nominal_x, nominal_y))
        .unwrap_or([0.0; 2]);
    let half_width = grid.cell_width * (0.5 - inset);
    let half_height = grid.cell_height * (0.5 - inset);
    let center_x = nominal_x + smooth[0] + residual[0];
    let center_y = nominal_y + smooth[1] + residual[1];
    let [x0, y0, x1, y1] = [
        center_x - half_width,
        center_y - half_height,
        center_x + half_width,
        center_y + half_height,
    ];
    if x0 < 0.0 || y0 < 0.0 || x1 > integral.width() as f64 || y1 > integral.height() as f64 {
        return None;
    }
    Some(integral.rect(x0, y0, x1, y1))
}

fn normalized_variance(moments: Moments) -> f64 {
    moments.sse() / (moments.area * 3.0).max(f64::EPSILON)
}

/// Refine only source sampling. Every logical cell retains its original rigid
/// output coordinate even when its sampling window receives a residual shift.
pub fn refine_cell_samples(
    cells: &[CellColor],
    integral: &IntegralImage,
    grid: Candidate,
    inset: f64,
    warp: Option<&WarpField>,
    options: CellWarpOptions,
) -> CellWarpResult {
    let baseline_labs: HashMap<(i32, i32), [f64; 3]> = cells
        .iter()
        .map(|cell| ((cell.cell_x, cell.cell_y), srgb_to_oklab(cell.rgb)))
        .collect();
    let scale_squared = grid.cell_width * grid.cell_height;
    let steps = (options.radius / options.step).ceil() as i32;

    let refined: Vec<RefinedCell> = cells
        .par_iter()
        .map(|cell| {
            let unchanged = || RefinedCell {
                cell: cell.clone(),
                offset: [0.0; 2],
                eligible: false,
                variance_reduction: 0.0,
            };
            let Some(baseline_moments) =
                sample_moments(integral, grid, inset, warp, cell, [0.0; 2])
            else {
                return unchanged();
            };
            let baseline_variance = normalized_variance(baseline_moments);
            if baseline_variance < options.min_variance {
                return unchanged();
            }

            let Some(&baseline_lab) = baseline_labs.get(&(cell.cell_x, cell.cell_y)) else {
                return unchanged();
            };
            let local_contrast = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .into_iter()
                .filter_map(|(dx, dy)| baseline_labs.get(&(cell.cell_x + dx, cell.cell_y + dy)))
                .map(|&neighbor| distance_squared(baseline_lab, neighbor).sqrt())
                .fold(0.0, f64::max);
            if local_contrast < options.contrast_threshold {
                return unchanged();
            }

            let mut best_offset = [0.0; 2];
            let mut best_moments = baseline_moments;
            let mut best_variance = baseline_variance;
            let mut best_score = f64::INFINITY;
            for dy in -steps..=steps {
                for dx in -steps..=steps {
                    let offset = [
                        (dx as f64 * options.step).clamp(-options.radius, options.radius),
                        (dy as f64 * options.step).clamp(-options.radius, options.radius),
                    ];
                    let Some(moments) = sample_moments(integral, grid, inset, warp, cell, offset)
                    else {
                        continue;
                    };
                    let variance = normalized_variance(moments);
                    let candidate_lab = srgb_to_oklab(moments.mean());
                    let candidate_contrast = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                        .into_iter()
                        .filter_map(|(neighbor_x, neighbor_y)| {
                            baseline_labs.get(&(cell.cell_x + neighbor_x, cell.cell_y + neighbor_y))
                        })
                        .map(|&neighbor| distance_squared(candidate_lab, neighbor).sqrt())
                        .fold(0.0, f64::max);
                    if candidate_contrast < local_contrast + options.min_contrast_gain {
                        continue;
                    }
                    let movement = (offset[0] * offset[0] + offset[1] * offset[1])
                        / scale_squared.max(f64::EPSILON);
                    let score = variance + options.movement_penalty * movement;
                    if score < best_score {
                        best_score = score;
                        best_variance = variance;
                        best_moments = moments;
                        best_offset = offset;
                    }
                }
            }

            let relative_improvement =
                (baseline_variance - best_variance) / baseline_variance.max(f64::EPSILON);
            if best_offset == [0.0; 2] || relative_improvement < options.min_improvement {
                return RefinedCell {
                    eligible: true,
                    ..unchanged()
                };
            }
            RefinedCell {
                cell: CellColor {
                    cell_x: cell.cell_x,
                    cell_y: cell.cell_y,
                    rgb: best_moments.mean(),
                    weight: best_moments.area,
                },
                offset: best_offset,
                eligible: true,
                variance_reduction: baseline_variance - best_variance,
            }
        })
        .collect();

    let shifted: Vec<_> = refined
        .iter()
        .filter(|cell| cell.offset != [0.0; 2])
        .collect();
    let shifted_count = shifted.len().max(1) as f64;
    let report = CellWarpReport {
        search_radius: options.radius,
        search_step: options.step,
        movement_penalty: options.movement_penalty,
        min_improvement: options.min_improvement,
        min_variance: options.min_variance,
        contrast_threshold: options.contrast_threshold,
        min_contrast_gain: options.min_contrast_gain,
        eligible_cells: refined.iter().filter(|cell| cell.eligible).count() as u64,
        shifted_cells: shifted.len() as u64,
        mean_variance_reduction: shifted
            .iter()
            .map(|cell| cell.variance_reduction)
            .sum::<f64>()
            / shifted_count,
        rms_displacement: (shifted
            .iter()
            .map(|cell| cell.offset[0].powi(2) + cell.offset[1].powi(2))
            .sum::<f64>()
            / shifted_count)
            .sqrt(),
        max_displacement: shifted
            .iter()
            .map(|cell| cell.offset[0].hypot(cell.offset[1]))
            .fold(0.0, f64::max),
    };
    let offsets = shifted
        .iter()
        .map(|cell| ((cell.cell.cell_x, cell.cell.cell_y), cell.offset))
        .collect();
    CellWarpResult {
        cells: refined.into_iter().map(|cell| cell.cell).collect(),
        offsets,
        report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn shifts_only_sampling_to_isolate_a_high_contrast_cell() {
        let image = RgbImage::from_fn(12, 4, |x, _| {
            if (5..9).contains(&x) {
                Rgb([255, 255, 255])
            } else {
                Rgb([0, 0, 0])
            }
        });
        let integral = IntegralImage::new(&image);
        let cells = vec![
            CellColor {
                cell_x: 0,
                cell_y: 0,
                rgb: [0.0; 3],
                weight: 16.0,
            },
            CellColor {
                cell_x: 1,
                cell_y: 0,
                rgb: [0.75; 3],
                weight: 16.0,
            },
            CellColor {
                cell_x: 2,
                cell_y: 0,
                rgb: [0.25; 3],
                weight: 16.0,
            },
        ];
        let grid = Candidate {
            cell_width: 4.0,
            cell_height: 4.0,
            phase_x: 0.0,
            phase_y: 0.0,
            score: 0.0,
            normalized_residual: 0.0,
            sampled_cells: 3,
            edge_alignment: 1.0,
            auto_score: 0.0,
        };
        let result = refine_cell_samples(
            &cells,
            &integral,
            grid,
            0.0,
            None,
            CellWarpOptions {
                radius: 1.0,
                step: 1.0,
                movement_penalty: 0.0,
                min_improvement: 0.1,
                min_variance: 0.001,
                contrast_threshold: 0.08,
                min_contrast_gain: 0.01,
            },
        );
        let middle = result.cells.iter().find(|cell| cell.cell_x == 1).unwrap();
        assert!(middle.rgb[0] > 0.99);
        assert_eq!(result.offsets[&(1, 0)], [1.0, 0.0]);
    }
}
