use crate::{
    color::{distance_squared, srgb_to_oklab},
    palette::CellColor,
};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub struct RampOptions {
    /// Dimensionless weight relative to the squared endpoint contrast.
    pub penalty: f64,
    pub contrast_threshold: f64,
    pub line_tolerance: f64,
    pub continuation_threshold: f64,
    pub max_passes: u32,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct RampReport {
    pub penalty: f64,
    pub contrast_threshold: f64,
    pub line_tolerance: f64,
    pub continuation_threshold: f64,
    pub max_passes: u32,
    pub passes_run: u32,
    pub corrected_cells: u64,
    pub horizontal_corrections: u64,
    pub vertical_corrections: u64,
}

#[derive(Clone, Copy)]
struct Proposal {
    assignment: usize,
    gain: f64,
    horizontal: bool,
}

#[derive(Clone, Copy)]
struct Neighborhood {
    endpoints: [usize; 2],
    outers: [usize; 2],
    horizontal: bool,
}

fn ramp_proposal(
    center_cell: usize,
    before: &[usize],
    source_labs: &[[f64; 3]],
    palette_labs: &[[f64; 3]],
    neighborhood: Neighborhood,
    options: RampOptions,
) -> Option<Proposal> {
    let center_assignment = before[center_cell];
    let endpoint_assignments = neighborhood.endpoints.map(|index| before[index]);
    if endpoint_assignments[0] == endpoint_assignments[1]
        || endpoint_assignments.contains(&center_assignment)
    {
        return None;
    }

    let a = palette_labs[endpoint_assignments[0]];
    let b = palette_labs[endpoint_assignments[1]];
    let center = palette_labs[center_assignment];
    let outer_assignments = neighborhood.outers.map(|index| before[index]);
    let continuation_squared = options.continuation_threshold * options.continuation_threshold;
    if distance_squared(palette_labs[outer_assignments[0]], a) > continuation_squared
        || distance_squared(palette_labs[outer_assignments[1]], b) > continuation_squared
    {
        return None;
    }
    let axis = std::array::from_fn::<_, 3, _>(|channel| b[channel] - a[channel]);
    let contrast_squared: f64 = axis.iter().map(|value| value * value).sum();
    if contrast_squared < options.contrast_threshold * options.contrast_threshold {
        return None;
    }

    let relative = std::array::from_fn::<_, 3, _>(|channel| center[channel] - a[channel]);
    let t = relative
        .iter()
        .zip(axis)
        .map(|(relative, axis)| relative * axis)
        .sum::<f64>()
        / contrast_squared;
    if !(0.12..=0.88).contains(&t) {
        return None;
    }
    let projected = std::array::from_fn(|channel| a[channel] + t * axis[channel]);
    let line_error = distance_squared(center, projected).sqrt();
    if line_error > options.line_tolerance {
        return None;
    }

    let source = source_labs[center_cell];
    let current_data_cost = distance_squared(source, center);
    let (assignment, replacement_data_cost) = endpoint_assignments
        .into_iter()
        .map(|assignment| {
            (
                assignment,
                distance_squared(source, palette_labs[assignment]),
            )
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))?;

    // The penalty peaks for a midpoint and fades as the middle color approaches
    // either endpoint. Off-axis colors are also less likely to be antialiasing.
    let interpolation_strength = 4.0 * t * (1.0 - t);
    let collinearity = 1.0 - line_error / options.line_tolerance;
    let ramp_cost = options.penalty * contrast_squared * interpolation_strength * collinearity;
    let fidelity_loss = replacement_data_cost - current_data_cost;
    let gain = ramp_cost - fidelity_loss;
    (gain > 0.0).then_some(Proposal {
        assignment,
        gain,
        horizontal: neighborhood.horizontal,
    })
}

/// Remove a one-cell intermediate-color ramp only when its opposite neighbors
/// are high contrast and the center palette color lies between them.
pub fn penalize_one_cell_ramps(
    cells: &[CellColor],
    palette: &[[f64; 3]],
    assignments: &mut [usize],
    options: RampOptions,
) -> RampReport {
    let mut report = RampReport {
        penalty: options.penalty,
        contrast_threshold: options.contrast_threshold,
        line_tolerance: options.line_tolerance,
        continuation_threshold: options.continuation_threshold,
        max_passes: options.max_passes,
        ..RampReport::default()
    };
    if options.penalty == 0.0 || options.max_passes == 0 {
        return report;
    }

    let cell_indices: HashMap<(i32, i32), usize> = cells
        .iter()
        .enumerate()
        .map(|(index, cell)| ((cell.cell_x, cell.cell_y), index))
        .collect();
    let source_labs: Vec<_> = cells.iter().map(|cell| srgb_to_oklab(cell.rgb)).collect();
    let palette_labs: Vec<_> = palette.iter().copied().map(srgb_to_oklab).collect();

    for _ in 0..options.max_passes {
        let before = assignments.to_vec();
        let mut proposals = vec![None; cells.len()];
        for (index, cell) in cells.iter().enumerate() {
            for (delta, horizontal) in [([1, 0], true), ([0, 1], false)] {
                let endpoints = [
                    (cell.cell_x - delta[0], cell.cell_y - delta[1]),
                    (cell.cell_x + delta[0], cell.cell_y + delta[1]),
                ];
                let outers = [
                    (cell.cell_x - 2 * delta[0], cell.cell_y - 2 * delta[1]),
                    (cell.cell_x + 2 * delta[0], cell.cell_y + 2 * delta[1]),
                ];
                let (Some(&first), Some(&second), Some(&outer_first), Some(&outer_second)) = (
                    cell_indices.get(&endpoints[0]),
                    cell_indices.get(&endpoints[1]),
                    cell_indices.get(&outers[0]),
                    cell_indices.get(&outers[1]),
                ) else {
                    continue;
                };
                let Some(proposal) = ramp_proposal(
                    index,
                    &before,
                    &source_labs,
                    &palette_labs,
                    Neighborhood {
                        endpoints: [first, second],
                        outers: [outer_first, outer_second],
                        horizontal,
                    },
                    options,
                ) else {
                    continue;
                };
                if proposals[index].is_none_or(|current: Proposal| proposal.gain > current.gain) {
                    proposals[index] = Some(proposal);
                }
            }
        }

        let mut changed_this_pass = 0_u64;
        for (assignment, proposal) in assignments.iter_mut().zip(proposals) {
            let Some(proposal) = proposal else { continue };
            if *assignment != proposal.assignment {
                *assignment = proposal.assignment;
                changed_this_pass += 1;
                report.horizontal_corrections += u64::from(proposal.horizontal);
                report.vertical_corrections += u64::from(!proposal.horizontal);
            }
        }
        report.passes_run += 1;
        report.corrected_cells += changed_this_pass;
        if changed_this_pass == 0 {
            break;
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(x: i32, rgb: [f64; 3]) -> CellColor {
        CellColor {
            cell_x: x,
            cell_y: 0,
            rgb,
            weight: 1.0,
        }
    }

    #[test]
    fn coerces_a_one_cell_collinear_ramp_toward_an_endpoint() {
        let cells = vec![
            cell(-1, [0.0; 3]),
            cell(0, [0.0; 3]),
            cell(1, [0.5; 3]),
            cell(2, [1.0; 3]),
            cell(3, [1.0; 3]),
        ];
        let palette = vec![[0.0; 3], [0.5; 3], [1.0; 3]];
        let mut assignments = vec![0, 0, 1, 2, 2];
        let report = penalize_one_cell_ramps(
            &cells,
            &palette,
            &mut assignments,
            RampOptions {
                penalty: 0.3,
                contrast_threshold: 0.08,
                line_tolerance: 0.04,
                continuation_threshold: 0.04,
                max_passes: 1,
            },
        );
        assert_eq!(report.corrected_cells, 1);
        assert_ne!(assignments[2], 1);
    }

    #[test]
    fn preserves_a_middle_color_that_is_not_on_the_endpoint_ramp() {
        let cells = vec![
            cell(-1, [0.0, 0.0, 0.0]),
            cell(0, [0.0, 0.0, 0.0]),
            cell(1, [1.0, 0.0, 0.0]),
            cell(2, [1.0, 1.0, 1.0]),
            cell(3, [1.0, 1.0, 1.0]),
        ];
        let palette = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
        let mut assignments = vec![0, 0, 1, 2, 2];
        let report = penalize_one_cell_ramps(
            &cells,
            &palette,
            &mut assignments,
            RampOptions {
                penalty: 1.0,
                contrast_threshold: 0.08,
                line_tolerance: 0.02,
                continuation_threshold: 0.04,
                max_passes: 1,
            },
        );
        assert_eq!(report.corrected_cells, 0);
        assert_eq!(assignments[2], 1);
    }
}
