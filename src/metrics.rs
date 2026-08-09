use crate::color::{distance_squared, rgb8, srgb_to_oklab};
use serde::Serialize;
use std::{collections::HashMap, ops::Range};

pub const WEAK_EDGE_THRESHOLD: f64 = 0.04;
pub const STRONG_EDGE_THRESHOLD: f64 = 0.08;

/// Measurements made on the recovered logical-pixel lattice, not on the
/// enlarged output bitmap. Oklab distances are roughly perceptual: zero means
/// identical neighbors and larger values mean a more visible separation.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct OutputMetrics {
    pub adjacency_count: u64,
    pub different_neighbor_fraction: f64,
    pub mean_neighbor_distance: f64,
    pub rms_neighbor_distance: f64,
    pub mean_changed_neighbor_distance: f64,
    pub weak_edge_threshold: f64,
    pub weak_transition_fraction_of_changed: f64,
    pub strong_edge_threshold: f64,
    pub strong_edge_fraction: f64,
    pub strong_edge_fraction_of_changed: f64,
    /// A soft-thresholded total-variation score. Higher is crisper, but this
    /// should be read alongside edge density so that random noise is not
    /// mistaken for useful detail.
    pub crispness_score: f64,
}

pub fn measure_output(
    cell_palette: &HashMap<(i32, i32), usize>,
    palette: &[[f64; 3]],
    cells_x: Range<i32>,
    cells_y: Range<i32>,
) -> OutputMetrics {
    let labs: Vec<[f64; 3]> = palette
        .iter()
        .map(|&rgb| {
            let quantized = rgb8(rgb).map(|channel| f64::from(channel) / 255.0);
            srgb_to_oklab(quantized)
        })
        .collect();
    let mut count = 0_u64;
    let mut changed = 0_u64;
    let mut weak = 0_u64;
    let mut strong = 0_u64;
    let mut sum = 0.0;
    let mut sum_squared = 0.0;
    let mut changed_sum = 0.0;
    let mut crispness_sum = 0.0;

    let mut add_pair = |a: (i32, i32), b: (i32, i32)| {
        // Match rendering exactly: partial outer cells without a usable source
        // sample receive palette entry zero.
        let a_index = cell_palette.get(&a).copied().unwrap_or(0);
        let b_index = cell_palette.get(&b).copied().unwrap_or(0);
        let distance = distance_squared(labs[a_index], labs[b_index]).sqrt();
        count += 1;
        sum += distance;
        sum_squared += distance * distance;
        crispness_sum += distance * distance / (distance + WEAK_EDGE_THRESHOLD);
        if distance > f64::EPSILON {
            changed += 1;
            changed_sum += distance;
            weak += u64::from(distance < WEAK_EDGE_THRESHOLD);
            strong += u64::from(distance >= STRONG_EDGE_THRESHOLD);
        }
    };

    for y in cells_y.clone() {
        for x in cells_x.clone() {
            if x + 1 < cells_x.end {
                add_pair((x, y), (x + 1, y));
            }
            if y + 1 < cells_y.end {
                add_pair((x, y), (x, y + 1));
            }
        }
    }

    let total = count.max(1) as f64;
    let changed_total = changed.max(1) as f64;
    OutputMetrics {
        adjacency_count: count,
        different_neighbor_fraction: changed as f64 / total,
        mean_neighbor_distance: sum / total,
        rms_neighbor_distance: (sum_squared / total).sqrt(),
        mean_changed_neighbor_distance: changed_sum / changed_total,
        weak_edge_threshold: WEAK_EDGE_THRESHOLD,
        weak_transition_fraction_of_changed: weak as f64 / changed_total,
        strong_edge_threshold: STRONG_EDGE_THRESHOLD,
        strong_edge_fraction: strong as f64 / total,
        strong_edge_fraction_of_changed: strong as f64 / changed_total,
        crispness_score: crispness_sum / total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_logical_neighbors_without_output_scale_bias() {
        let palette = vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
        let assignments = HashMap::from([((0, 0), 0), ((1, 0), 1), ((0, 1), 0), ((1, 1), 1)]);
        let metrics = measure_output(&assignments, &palette, 0..2, 0..2);
        assert_eq!(metrics.adjacency_count, 4);
        assert_eq!(metrics.different_neighbor_fraction, 0.5);
        assert_eq!(metrics.strong_edge_fraction, 0.5);
        assert!(metrics.mean_changed_neighbor_distance > 0.9);
    }
}
