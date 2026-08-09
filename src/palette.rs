use crate::color::{distance_squared, srgb_to_oklab};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct CellColor {
    pub cell_x: i32,
    pub cell_y: i32,
    pub rgb: [f64; 3],
    pub weight: f64,
}

pub fn nearest_palette_index(rgb: [f64; 3], palette: &[[f64; 3]]) -> usize {
    let lab = srgb_to_oklab(rgb);
    palette
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            distance_squared(lab, srgb_to_oklab(**a))
                .total_cmp(&distance_squared(lab, srgb_to_oklab(**b)))
        })
        .map(|(index, _)| index)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug)]
struct Center {
    lab: [f64; 3],
    rgb: [f64; 3],
    weight: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct SmartPaletteOptions {
    /// Maximum size of the fixed-palette candidate bank.
    pub candidate_max: usize,
    /// Hard limit used by the flexible fallback.
    pub max_colors: usize,
    /// Cost paid for each color beyond two.
    pub complexity_penalty: f64,
    /// Multiplier for cells lying on strong logical-pixel edges.
    pub edge_emphasis: f64,
    /// Merge radius used by the flexible fallback.
    pub merge_threshold: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct PaletteCandidateReport {
    pub colors: usize,
    pub sse: f64,
    pub fit: f64,
    pub penalty: f64,
    pub total: f64,
    pub normalized_fit: f64,
    pub minimum_cluster_fraction: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct PaletteSelectionReport {
    /// `fixed` chooses one candidate; `flexible` uses threshold clustering.
    pub mode: String,
    pub selected_colors: usize,
    pub distinct_histogram_bins: usize,
    pub histogram_peaks: usize,
    pub fixed_candidate_limit: usize,
    pub candidates: Vec<PaletteCandidateReport>,
}

pub struct PaletteSelection {
    pub palette: Vec<[f64; 3]>,
    pub assignments: Vec<usize>,
    pub report: PaletteSelectionReport,
}

#[derive(Clone, Copy)]
struct WeightedSample {
    lab: [f64; 3],
    rgb: [f64; 3],
    weight: f64,
}

struct CandidatePalette {
    palette: Vec<[f64; 3]>,
    assignments: Vec<usize>,
    report: PaletteCandidateReport,
}

/// Online threshold merging produces an automatic palette estimate. An optional
/// cap then uses weighted farthest-point seeds, followed by Lloyd refinement.
pub fn cluster(
    cells: &[CellColor],
    threshold: f64,
    max_colors: Option<usize>,
) -> (Vec<[f64; 3]>, Vec<usize>) {
    let labs: Vec<[f64; 3]> = cells.iter().map(|cell| srgb_to_oklab(cell.rgb)).collect();
    let mut centers: Vec<Center> = Vec::new();

    for (cell, &lab) in cells.iter().zip(&labs) {
        let nearest = centers
            .iter()
            .enumerate()
            .map(|(index, center)| (index, distance_squared(lab, center.lab)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((index, _distance)) =
            nearest.filter(|(_, distance)| *distance <= threshold * threshold)
        {
            let center = &mut centers[index];
            let total = center.weight + cell.weight;
            for (channel, &lab_value) in lab.iter().enumerate() {
                center.lab[channel] =
                    (center.lab[channel] * center.weight + lab_value * cell.weight) / total;
                center.rgb[channel] =
                    (center.rgb[channel] * center.weight + cell.rgb[channel] * cell.weight) / total;
            }
            center.weight = total;
        } else {
            centers.push(Center {
                lab,
                rgb: cell.rgb,
                weight: cell.weight,
            });
        }
    }

    if let Some(limit) = max_colors.map(|value| value.max(1))
        && centers.len() > limit
    {
        let source = centers;
        let first = source
            .iter()
            .max_by(|a, b| a.weight.total_cmp(&b.weight))
            .copied()
            .unwrap();
        centers = vec![first];
        while centers.len() < limit {
            let next = source
                .iter()
                .max_by(|a, b| {
                    let merit = |sample: &&Center| {
                        centers
                            .iter()
                            .map(|center| distance_squared(sample.lab, center.lab))
                            .fold(f64::INFINITY, f64::min)
                            * sample.weight.sqrt()
                    };
                    merit(a).total_cmp(&merit(b))
                })
                .copied()
                .unwrap();
            centers.push(next);
        }
    }

    let mut assignments = vec![0; cells.len()];
    for _ in 0..12 {
        let mut lab_sums = vec![[0.0; 3]; centers.len()];
        let mut rgb_sums = vec![[0.0; 3]; centers.len()];
        let mut weights = vec![0.0; centers.len()];
        let mut changed = false;
        for (index, (cell, &lab)) in cells.iter().zip(&labs).enumerate() {
            let nearest = centers
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    distance_squared(lab, a.lab).total_cmp(&distance_squared(lab, b.lab))
                })
                .map(|(index, _)| index)
                .unwrap_or(0);
            changed |= assignments[index] != nearest;
            assignments[index] = nearest;
            weights[nearest] += cell.weight;
            for channel in 0..3 {
                lab_sums[nearest][channel] += lab[channel] * cell.weight;
                rgb_sums[nearest][channel] += cell.rgb[channel] * cell.weight;
            }
        }
        for index in 0..centers.len() {
            if weights[index] > 0.0 {
                centers[index].weight = weights[index];
                for channel in 0..3 {
                    centers[index].lab[channel] = lab_sums[index][channel] / weights[index];
                    centers[index].rgb[channel] = rgb_sums[index][channel] / weights[index];
                }
            }
        }
        if !changed {
            break;
        }
    }

    (
        centers.into_iter().map(|center| center.rgb).collect(),
        assignments,
    )
}

fn edge_weighted_samples(cells: &[CellColor], edge_emphasis: f64) -> Vec<WeightedSample> {
    let labs: Vec<_> = cells.iter().map(|cell| srgb_to_oklab(cell.rgb)).collect();
    let positions: HashMap<_, _> = cells
        .iter()
        .enumerate()
        .map(|(index, cell)| ((cell.cell_x, cell.cell_y), index))
        .collect();

    cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let mut edge_signal = 0.0_f64;
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                if let Some(&neighbor) = positions.get(&(cell.cell_x + dx, cell.cell_y + dy)) {
                    edge_signal += distance_squared(labs[index], labs[neighbor]).sqrt();
                }
            }
            // About 0.08 Oklab is already a visibly strong logical-pixel edge.
            // Summing all four sides favors an isolated highlight or thin outline
            // over the large flat region bordering it. Clamp the boost so the
            // feature cannot overwhelm the whole image.
            let edge_strength = (edge_signal / 0.08).clamp(0.0, 3.0);
            WeightedSample {
                lab: labs[index],
                rgb: cell.rgb,
                weight: cell.weight * (1.0 + edge_emphasis * edge_strength),
            }
        })
        .collect()
}

fn histogram_index(rgb: [f64; 3]) -> usize {
    let bins = rgb.map(|value| (value.clamp(0.0, 1.0) * 16.0).floor().min(15.0) as usize);
    (bins[0] * 16 + bins[1]) * 16 + bins[2]
}

fn histogram_rgb(index: usize) -> [f64; 3] {
    let red = index / 256;
    let green = (index / 16) % 16;
    let blue = index % 16;
    [red, green, blue].map(|bin| (bin as f64 + 0.5) / 16.0)
}

fn histogram_peaks(samples: &[WeightedSample]) -> (usize, Vec<[f64; 3]>) {
    // Cell extraction has already discarded alpha, so this is the RGB analogue
    // of a 16-bin-per-channel RGBA histogram.
    let mut histogram = vec![0.0_f64; 16 * 16 * 16];
    for sample in samples {
        histogram[histogram_index(sample.rgb)] += sample.weight;
    }
    let distinct = histogram.iter().filter(|&&weight| weight > 0.0).count();
    let mut peaks = Vec::new();
    for red in 0..16_i32 {
        for green in 0..16_i32 {
            for blue in 0..16_i32 {
                let index = ((red * 16 + green) * 16 + blue) as usize;
                let weight = histogram[index];
                if weight < 2.0 {
                    continue;
                }
                let mut is_peak = true;
                'neighbors: for dr in -1..=1 {
                    for dg in -1..=1 {
                        for db in -1..=1 {
                            let (r, g, b) = (red + dr, green + dg, blue + db);
                            if r < 0 || g < 0 || b < 0 || r >= 16 || g >= 16 || b >= 16 {
                                continue;
                            }
                            let neighbor = ((r * 16 + g) * 16 + b) as usize;
                            if histogram[neighbor] > weight {
                                is_peak = false;
                                break 'neighbors;
                            }
                        }
                    }
                }
                if is_peak {
                    peaks.push((weight, index));
                }
            }
        }
    }
    peaks.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    (
        distinct,
        peaks
            .into_iter()
            .map(|(_, index)| histogram_rgb(index))
            .collect(),
    )
}

fn seed_centers(samples: &[WeightedSample], peaks: &[[f64; 3]], count: usize) -> Vec<Center> {
    let mut centers: Vec<Center> = peaks
        .iter()
        .take(count)
        .copied()
        .map(|rgb| Center {
            lab: srgb_to_oklab(rgb),
            rgb,
            weight: 0.0,
        })
        .collect();

    // Real inputs occasionally have fewer local maxima than requested colors.
    // Use deterministic weighted farthest-point filling instead of random seeds.
    while centers.len() < count {
        let next = samples
            .iter()
            .max_by(|a, b| {
                let merit = |sample: &&WeightedSample| {
                    let distance = if centers.is_empty() {
                        1.0
                    } else {
                        centers
                            .iter()
                            .map(|center| distance_squared(sample.lab, center.lab))
                            .fold(f64::INFINITY, f64::min)
                    };
                    distance * sample.weight.sqrt()
                };
                merit(a).total_cmp(&merit(b))
            })
            .copied()
            .unwrap();
        centers.push(Center {
            lab: next.lab,
            rgb: next.rgb,
            weight: 0.0,
        });
    }
    centers
}

fn build_candidate(
    samples: &[WeightedSample],
    peaks: &[[f64; 3]],
    colors: usize,
    complexity_penalty: f64,
) -> CandidatePalette {
    let mut centers = seed_centers(samples, peaks, colors);
    let mut assignments = vec![usize::MAX; samples.len()];

    for _ in 0..10 {
        let mut lab_sums = vec![[0.0; 3]; colors];
        let mut rgb_sums = vec![[0.0; 3]; colors];
        let mut weights = vec![0.0; colors];
        for (index, sample) in samples.iter().enumerate() {
            let nearest = centers
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    distance_squared(sample.lab, a.lab)
                        .total_cmp(&distance_squared(sample.lab, b.lab))
                })
                .map(|(index, _)| index)
                .unwrap_or(0);
            assignments[index] = nearest;
            weights[nearest] += sample.weight;
            for channel in 0..3 {
                lab_sums[nearest][channel] += sample.lab[channel] * sample.weight;
                rgb_sums[nearest][channel] += sample.rgb[channel] * sample.weight;
            }
        }

        let mut max_component_delta = 0.0_f64;
        for index in 0..colors {
            if weights[index] == 0.0 {
                continue;
            }
            let old_rgb = centers[index].rgb;
            centers[index].weight = weights[index];
            for channel in 0..3 {
                centers[index].lab[channel] = lab_sums[index][channel] / weights[index];
                centers[index].rgb[channel] = rgb_sums[index][channel] / weights[index];
                max_component_delta =
                    max_component_delta.max((old_rgb[channel] - centers[index].rgb[channel]).abs());
            }
        }
        if max_component_delta < 0.01 {
            break;
        }
    }

    let mut sse = 0.0;
    let mut cluster_weights = vec![0.0; colors];
    for (index, sample) in samples.iter().enumerate() {
        let nearest = centers
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                distance_squared(sample.lab, a.lab).total_cmp(&distance_squared(sample.lab, b.lab))
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        assignments[index] = nearest;
        cluster_weights[nearest] += sample.weight;
        sse += sample.weight * distance_squared(sample.lab, centers[nearest].lab);
    }
    let total_weight = cluster_weights.iter().sum::<f64>();
    let minimum_cluster_fraction = cluster_weights
        .iter()
        .map(|weight| weight / total_weight.max(f64::EPSILON))
        .fold(f64::INFINITY, f64::min);
    let fit = (sse + 1.0).log10();
    let penalty = complexity_penalty * colors.saturating_sub(2) as f64;
    // Quantize stored colors to the same precision as the PNG output. This also
    // makes a later reassignment of locally shifted cells exactly reproducible.
    let palette = centers
        .into_iter()
        .map(|center| center.rgb.map(|value| (value * 255.0).round() / 255.0))
        .collect();

    CandidatePalette {
        palette,
        assignments,
        report: PaletteCandidateReport {
            colors,
            sse,
            fit,
            penalty,
            total: fit + penalty,
            normalized_fit: 0.0,
            minimum_cluster_fraction,
        },
    }
}

/// Build edge-aware, peak-seeded fixed candidates and select a compact palette.
///
/// The candidate machinery follows the supplied clean-room design. Because its
/// learned decision tree is not available, selection uses an explicit
/// rate-distortion decision: the lowest penalized candidate wins. If the curve
/// is still improving at the end of the fixed bank and the histogram remains
/// peak-rich, threshold clustering supplies a flexible palette instead.
pub fn select_smart_palette(cells: &[CellColor], options: SmartPaletteOptions) -> PaletteSelection {
    if cells.is_empty() {
        return PaletteSelection {
            palette: Vec::new(),
            assignments: Vec::new(),
            report: PaletteSelectionReport {
                mode: "fixed".into(),
                selected_colors: 0,
                distinct_histogram_bins: 0,
                histogram_peaks: 0,
                fixed_candidate_limit: 0,
                candidates: Vec::new(),
            },
        };
    }

    let hard_limit = options.max_colors.max(1).min(cells.len());
    if hard_limit == 1 {
        let (palette, assignments) = cluster(cells, options.merge_threshold, Some(1));
        return PaletteSelection {
            report: PaletteSelectionReport {
                mode: "fixed".into(),
                selected_colors: palette.len(),
                distinct_histogram_bins: 1,
                histogram_peaks: 1,
                fixed_candidate_limit: 1,
                candidates: Vec::new(),
            },
            palette,
            assignments,
        };
    }

    let samples = edge_weighted_samples(cells, options.edge_emphasis.max(0.0));
    let (distinct_histogram_bins, peaks) = histogram_peaks(&samples);
    let candidate_limit = options.candidate_max.clamp(2, hard_limit);
    let mut candidates: Vec<_> = (2..=candidate_limit)
        .map(|colors| {
            build_candidate(
                &samples,
                &peaks,
                colors,
                options.complexity_penalty.max(0.0),
            )
        })
        .collect();

    let min_fit = candidates
        .iter()
        .map(|candidate| candidate.report.fit)
        .fold(f64::INFINITY, f64::min);
    let max_fit = candidates
        .iter()
        .map(|candidate| candidate.report.fit)
        .fold(f64::NEG_INFINITY, f64::max);
    let fit_range = max_fit - min_fit;
    for candidate in &mut candidates {
        candidate.report.normalized_fit = if fit_range > f64::EPSILON {
            (candidate.report.fit - min_fit) / fit_range
        } else {
            0.0
        };
    }

    let best_index = candidates
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.report.total.total_cmp(&b.report.total))
        .map(|(index, _)| index)
        .unwrap_or(0);
    let last_fit_gain = candidates
        .iter()
        .rev()
        .take(2)
        .map(|candidate| candidate.report.fit)
        .collect::<Vec<_>>();
    let last_fit_gain = if last_fit_gain.len() == 2 {
        last_fit_gain[1] - last_fit_gain[0]
    } else {
        0.0
    };
    let use_flexible = hard_limit > candidate_limit
        && best_index + 1 == candidates.len()
        && peaks.len() > candidate_limit
        && last_fit_gain > options.complexity_penalty * 0.8;

    let reports = candidates
        .iter()
        .map(|candidate| candidate.report.clone())
        .collect();
    if use_flexible {
        let (palette, assignments) = cluster(cells, options.merge_threshold, Some(hard_limit));
        PaletteSelection {
            report: PaletteSelectionReport {
                mode: "flexible".into(),
                selected_colors: palette.len(),
                distinct_histogram_bins,
                histogram_peaks: peaks.len(),
                fixed_candidate_limit: candidate_limit,
                candidates: reports,
            },
            palette,
            assignments,
        }
    } else {
        let selected = candidates.swap_remove(best_index);
        PaletteSelection {
            report: PaletteSelectionReport {
                mode: "fixed".into(),
                selected_colors: selected.palette.len(),
                distinct_histogram_bins,
                histogram_peaks: peaks.len(),
                fixed_candidate_limit: candidate_limit,
                candidates: reports,
            },
            palette: selected.palette,
            assignments: selected.assignments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(x: i32, y: i32, rgb: [f64; 3]) -> CellColor {
        CellColor {
            cell_x: x,
            cell_y: y,
            rgb,
            weight: 16.0,
        }
    }

    #[test]
    fn smart_palette_finds_a_clean_two_color_image() {
        let cells: Vec<_> = (0..8)
            .flat_map(|y| {
                (0..8).map(move |x| cell(x, y, if x < 4 { [0.05; 3] } else { [0.95; 3] }))
            })
            .collect();
        let result = select_smart_palette(
            &cells,
            SmartPaletteOptions {
                candidate_max: 8,
                max_colors: 16,
                complexity_penalty: 0.3,
                edge_emphasis: 1.0,
                merge_threshold: 0.035,
            },
        );
        assert_eq!(result.palette.len(), 2);
        assert_eq!(result.report.mode, "fixed");
        assert_eq!(result.assignments.len(), cells.len());
    }

    #[test]
    fn smart_palette_respects_hard_limit() {
        let cells: Vec<_> = (0..20)
            .map(|x| cell(x, 0, [x as f64 / 20.0, 0.2, 1.0 - x as f64 / 20.0]))
            .collect();
        let result = select_smart_palette(
            &cells,
            SmartPaletteOptions {
                candidate_max: 12,
                max_colors: 4,
                complexity_penalty: 0.0,
                edge_emphasis: 1.0,
                merge_threshold: 0.01,
            },
        );
        assert!(result.palette.len() <= 4);
        assert_eq!(result.report.fixed_candidate_limit, 4);
    }

    #[test]
    fn edge_weights_favor_high_contrast_cells() {
        let cells = vec![
            cell(0, 0, [0.1; 3]),
            cell(1, 0, [0.1; 3]),
            cell(2, 0, [0.9; 3]),
        ];
        let samples = edge_weighted_samples(&cells, 1.0);
        assert!(samples[1].weight > samples[0].weight);
        assert!(samples[2].weight > samples[0].weight);
    }
}
