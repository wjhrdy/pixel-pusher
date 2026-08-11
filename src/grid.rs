use crate::integral::IntegralImage;
use image::RgbImage;
use rayon::prelude::*;
use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub struct SearchOptions {
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
    pub inset_ratio: f64,
    pub phase_step: f64,
    pub dimension_step: f64,
    pub dimension_radius: f64,
    pub complexity: f64,
    pub auto_select: bool,
    pub square_coarse: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Candidate {
    /// Source-image spacing between logical grid lines. These are deliberately
    /// fractional and independent so a globally squeezed source can be fitted.
    pub cell_width: f64,
    pub cell_height: f64,
    pub phase_x: f64,
    pub phase_y: f64,
    pub score: f64,
    pub normalized_residual: f64,
    pub sampled_cells: usize,
    pub edge_alignment: f64,
    pub auto_score: f64,
}

pub struct EdgeProfiles {
    vertical: Vec<f64>,
    horizontal: Vec<f64>,
    vertical_total: f64,
    horizontal_total: f64,
}

impl EdgeProfiles {
    pub fn new(image: &RgbImage) -> Self {
        let width = image.width() as usize;
        let height = image.height() as usize;
        let mut vertical = vec![0.0; width + 1];
        let mut horizontal = vec![0.0; height + 1];
        for y in 0..height {
            for (x, energy) in vertical.iter_mut().enumerate().take(width).skip(1) {
                let left = image.get_pixel((x - 1) as u32, y as u32).0;
                let right = image.get_pixel(x as u32, y as u32).0;
                *energy += (0..3)
                    .map(|channel| {
                        let difference = right[channel] as f64 - left[channel] as f64;
                        difference * difference / (255.0 * 255.0)
                    })
                    .sum::<f64>();
            }
        }
        for (y, energy) in horizontal.iter_mut().enumerate().take(height).skip(1) {
            for x in 0..width {
                let above = image.get_pixel(x as u32, (y - 1) as u32).0;
                let below = image.get_pixel(x as u32, y as u32).0;
                *energy += (0..3)
                    .map(|channel| {
                        let difference = below[channel] as f64 - above[channel] as f64;
                        difference * difference / (255.0 * 255.0)
                    })
                    .sum::<f64>();
            }
        }
        let vertical_total = vertical.iter().sum();
        let horizontal_total = horizontal.iter().sum();
        Self {
            vertical,
            horizontal,
            vertical_total,
            horizontal_total,
        }
    }

    fn axis_enrichment(profile: &[f64], total: f64, spacing: f64, phase: f64) -> f64 {
        if total <= 1e-12 || profile.len() <= 2 {
            return 1.0;
        }
        let band = 1.25_f64.min(spacing * 0.35).max(0.5);
        let mut selected_energy = 0.0;
        let mut selected_weight = 0.0;
        for (coordinate, &energy) in profile.iter().enumerate().skip(1).take(profile.len() - 2) {
            let remainder = (coordinate as f64 - phase).rem_euclid(spacing);
            let distance = remainder.min(spacing - remainder);
            let weight = (1.0 - distance / band).max(0.0);
            selected_energy += energy * weight;
            selected_weight += weight;
        }
        let coverage = selected_weight / (profile.len() - 2) as f64;
        if coverage <= 1e-12 {
            return 1.0;
        }
        ((selected_energy / total) / coverage).clamp(0.1, 20.0)
    }

    fn alignment(&self, candidate: Candidate) -> f64 {
        let vertical = Self::axis_enrichment(
            &self.vertical,
            self.vertical_total,
            candidate.cell_width,
            candidate.phase_x,
        );
        let horizontal = Self::axis_enrichment(
            &self.horizontal,
            self.horizontal_total,
            candidate.cell_height,
            candidate.phase_y,
        );
        let total = self.vertical_total + self.horizontal_total;
        if total <= 1e-12 {
            1.0
        } else {
            (vertical * self.vertical_total + horizontal * self.horizontal_total) / total
        }
    }
}

struct Evaluator<'a> {
    integral: &'a IntegralImage,
    inset_ratio: f64,
    global_variance: f64,
    complexity: f64,
}

impl Evaluator<'_> {
    fn evaluate(&self, cell_width: f64, cell_height: f64, phase_x: f64, phase_y: f64) -> Candidate {
        let block_width = cell_width;
        let block_height = cell_height;
        let margin_x = block_width * self.inset_ratio;
        let margin_y = block_height * self.inset_ratio;
        let width = self.integral.width() as f64;
        let height = self.integral.height() as f64;
        let x_start = ((-phase_x) / block_width).ceil() as i32;
        let x_end = ((width - phase_x) / block_width).floor() as i32;
        let y_start = ((-phase_y) / block_height).ceil() as i32;
        let y_end = ((height - phase_y) / block_height).floor() as i32;

        let mut sse = 0.0;
        let mut area = 0.0;
        let mut sampled_cells = 0;
        for cell_y in y_start..y_end {
            let y0 = phase_y + cell_y as f64 * block_height + margin_y;
            let y1 = phase_y + (cell_y + 1) as f64 * block_height - margin_y;
            for cell_x in x_start..x_end {
                let x0 = phase_x + cell_x as f64 * block_width + margin_x;
                let x1 = phase_x + (cell_x + 1) as f64 * block_width - margin_x;
                let moments = self.integral.rect(x0, y0, x1, y1);
                sse += moments.sse();
                area += moments.area;
                sampled_cells += 1;
            }
        }

        let residual = if area > 0.0 {
            sse / (area * 3.0)
        } else {
            f64::INFINITY
        };
        let normalized_residual = residual / self.global_variance.max(1e-10);
        let score = normalized_residual + self.complexity / (block_width * block_height);
        Candidate {
            cell_width,
            cell_height,
            phase_x,
            phase_y,
            score,
            normalized_residual,
            sampled_cells,
            edge_alignment: 1.0,
            auto_score: f64::INFINITY,
        }
    }
}

fn rank_candidates(
    candidates: &mut [Candidate],
    auto_select: bool,
    edge_profiles: Option<&EdgeProfiles>,
) {
    if auto_select {
        let profiles = edge_profiles.expect("auto selection requires edge profiles");
        for candidate in candidates.iter_mut() {
            candidate.edge_alignment = profiles.alignment(*candidate);
            candidate.auto_score = candidate.normalized_residual / candidate.edge_alignment.powi(2)
                + 1.0 / (candidate.cell_width * candidate.cell_height);
        }
        candidates.sort_by(|a, b| a.auto_score.total_cmp(&b.auto_score));
    } else {
        candidates.sort_by(|a, b| a.score.total_cmp(&b.score));
    }
}

fn reanchor_phase(phase: f64, old_spacing: f64, new_spacing: f64, center: f64) -> f64 {
    let line_index = ((center - phase) / old_spacing).round();
    let anchor = phase + line_index * old_spacing;
    (anchor - line_index * new_spacing).rem_euclid(new_spacing)
}

fn refine_phase(evaluator: &Evaluator<'_>, initial: Candidate, phase_step: f64) -> Candidate {
    let mut best = initial;
    let steps = (1.0 / phase_step).round() as i32;
    let center_x = best.phase_x;
    let center_y = best.phase_y;
    for dy in -steps..=steps {
        for dx in -steps..=steps {
            let phase_x = (center_x + dx as f64 * phase_step).rem_euclid(best.cell_width);
            let phase_y = (center_y + dy as f64 * phase_step).rem_euclid(best.cell_height);
            let candidate = evaluator.evaluate(best.cell_width, best.cell_height, phase_x, phase_y);
            if candidate.score < best.score {
                best = candidate;
            }
        }
    }
    best
}

fn refine_dimensions(
    evaluator: &Evaluator<'_>,
    initial: Candidate,
    options: SearchOptions,
) -> Candidate {
    let mut best = initial;
    let nominal_width = best.cell_width.round();
    let nominal_height = best.cell_height.round();
    let dimension_steps = (options.dimension_radius / options.dimension_step).ceil() as i32;
    let image_center_x = evaluator.integral.width() as f64 * 0.5;
    let image_center_y = evaluator.integral.height() as f64 * 0.5;

    // Coordinate descent is much cheaper than a dense width × height × phase
    // product. Reanchoring at image center prevents spacing changes from being
    // confused with phase changes at the left/top edge.
    for _ in 0..3 {
        let previous = best;
        for step in -dimension_steps..=dimension_steps {
            let width = nominal_width + step as f64 * options.dimension_step;
            if width < options.min_width as f64 || width > options.max_width as f64 {
                continue;
            }
            let phase_x =
                reanchor_phase(previous.phase_x, previous.cell_width, width, image_center_x);
            let candidate = evaluator.evaluate(width, best.cell_height, phase_x, best.phase_y);
            if candidate.score < best.score {
                best = candidate;
            }
        }

        let previous = best;
        for step in -dimension_steps..=dimension_steps {
            let height = nominal_height + step as f64 * options.dimension_step;
            if height < options.min_height as f64 || height > options.max_height as f64 {
                continue;
            }
            let phase_y = reanchor_phase(
                previous.phase_y,
                previous.cell_height,
                height,
                image_center_y,
            );
            let candidate = evaluator.evaluate(best.cell_width, height, best.phase_x, phase_y);
            if candidate.score < best.score {
                best = candidate;
            }
        }
        best = refine_phase(evaluator, best, options.phase_step);
    }
    best
}

pub fn search(
    integral: &IntegralImage,
    options: SearchOptions,
    edge_profiles: Option<&EdgeProfiles>,
) -> Vec<Candidate> {
    let whole = integral.rect(0.0, 0.0, integral.width() as f64, integral.height() as f64);
    let global_variance = whole.sse() / (whole.area * 3.0).max(1.0);
    let evaluator = Evaluator {
        integral,
        inset_ratio: options.inset_ratio,
        global_variance,
        complexity: options.complexity,
    };

    let dimensions: Vec<(u32, u32)> = if options.square_coarse {
        let start = options.min_width.max(options.min_height);
        let end = options.max_width.min(options.max_height);
        (start..=end).map(|size| (size, size)).collect()
    } else {
        (options.min_height..=options.max_height)
            .flat_map(|height| {
                (options.min_width..=options.max_width).map(move |width| (width, height))
            })
            .collect()
    };
    let mut per_size: Vec<Candidate> = dimensions
        .into_par_iter()
        .map(|(cell_width, cell_height)| {
            // Integer-pixel coarse phase search.
            let mut best = Candidate {
                cell_width: cell_width as f64,
                cell_height: cell_height as f64,
                phase_x: 0.0,
                phase_y: 0.0,
                score: f64::INFINITY,
                normalized_residual: f64::INFINITY,
                sampled_cells: 0,
                edge_alignment: 1.0,
                auto_score: f64::INFINITY,
            };
            for phase_y in 0..cell_height {
                for phase_x in 0..cell_width {
                    let candidate = evaluator.evaluate(
                        cell_width as f64,
                        cell_height as f64,
                        phase_x as f64,
                        phase_y as f64,
                    );
                    if candidate.score < best.score {
                        best = candidate;
                    }
                }
            }

            best
        })
        .collect();

    rank_candidates(&mut per_size, options.auto_select, edge_profiles);

    // Rectangular search has a much larger candidate space than square-only
    // search. Refine the strongest coarse fits, rather than paying for every
    // width × height pair. A forced or narrow search still refines every pair.
    let refine_count = per_size.len().min(32);
    let refined: Vec<Candidate> = per_size[..refine_count]
        .par_iter()
        .map(|coarse| refine_phase(&evaluator, *coarse, options.phase_step))
        .collect();
    per_size[..refine_count].copy_from_slice(&refined);
    rank_candidates(&mut per_size, options.auto_select, edge_profiles);

    let dimension_refine_count = per_size.len().min(32);
    let dimension_refined: Vec<Candidate> = per_size[..dimension_refine_count]
        .par_iter()
        .map(|candidate| refine_dimensions(&evaluator, *candidate, options))
        .collect();
    per_size[..dimension_refine_count].copy_from_slice(&dimension_refined);
    rank_candidates(&mut per_size, options.auto_select, edge_profiles);
    per_size
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn source_color(cell_x: i32, cell_y: i32) -> [f64; 3] {
        const PALETTE: [[f64; 3]; 8] = [
            [0.06, 0.08, 0.13],
            [0.88, 0.19, 0.22],
            [0.13, 0.66, 0.31],
            [0.17, 0.38, 0.85],
            [0.95, 0.72, 0.18],
            [0.61, 0.24, 0.75],
            [0.16, 0.78, 0.79],
            [0.91, 0.86, 0.76],
        ];
        let hash = cell_x
            .wrapping_mul(73)
            .wrapping_add(cell_y.wrapping_mul(151))
            .wrapping_add(cell_x.wrapping_mul(cell_y).wrapping_mul(17));
        PALETTE[hash.rem_euclid(PALETTE.len() as i32) as usize]
    }

    #[test]
    fn recovers_a_fractionally_phased_rectangular_grid() {
        let (cell_width, cell_height) = (7.0, 9.0);
        let (phase_x, phase_y) = (1.25, 2.5);
        // Supersampling simulates the softened, boundary-contaminated source.
        let image = RgbImage::from_fn(70, 72, |x, y| {
            let mut sum = [0.0; 3];
            let samples = 4;
            for sy in 0..samples {
                for sx in 0..samples {
                    let px = x as f64 + (sx as f64 + 0.5) / samples as f64;
                    let py = y as f64 + (sy as f64 + 0.5) / samples as f64;
                    let cx = ((px - phase_x) / cell_width).floor() as i32;
                    let cy = ((py - phase_y) / cell_height).floor() as i32;
                    let color = source_color(cx, cy);
                    for channel in 0..3 {
                        sum[channel] += color[channel];
                    }
                }
            }
            Rgb(sum.map(|value| (value / (samples * samples) as f64 * 255.0).round() as u8))
        });
        let integral = IntegralImage::new(&image);
        let candidates = search(
            &integral,
            SearchOptions {
                min_width: 5,
                max_width: 9,
                min_height: 7,
                max_height: 11,
                inset_ratio: 0.14,
                phase_step: 0.25,
                dimension_step: 0.1,
                dimension_radius: 0.75,
                complexity: 0.002,
                auto_select: false,
                square_coarse: false,
            },
            None,
        );
        let best = candidates[0];
        assert!((best.cell_width - 7.0).abs() <= 0.11, "{best:?}");
        assert!((best.cell_height - 9.0).abs() <= 0.11, "{best:?}");
        assert!((best.phase_x - phase_x).abs() <= 0.5, "{best:?}");
        assert!((best.phase_y - phase_y).abs() <= 0.5, "{best:?}");
    }

    #[test]
    fn recovers_fractionally_squeezed_cell_dimensions() {
        let (cell_width, cell_height) = (7.3, 8.6);
        let (phase_x, phase_y) = (1.4, 2.2);
        let image = RgbImage::from_fn(146, 129, |x, y| {
            let mut sum = [0.0; 3];
            let samples = 4;
            for sy in 0..samples {
                for sx in 0..samples {
                    let px = x as f64 + (sx as f64 + 0.5) / samples as f64;
                    let py = y as f64 + (sy as f64 + 0.5) / samples as f64;
                    let color = source_color(
                        ((px - phase_x) / cell_width).floor() as i32,
                        ((py - phase_y) / cell_height).floor() as i32,
                    );
                    for channel in 0..3 {
                        sum[channel] += color[channel];
                    }
                }
            }
            Rgb(sum.map(|value| (value / (samples * samples) as f64 * 255.0).round() as u8))
        });
        let integral = IntegralImage::new(&image);
        let candidates = search(
            &integral,
            SearchOptions {
                min_width: 7,
                max_width: 8,
                min_height: 8,
                max_height: 9,
                inset_ratio: 0.14,
                phase_step: 0.2,
                dimension_step: 0.1,
                dimension_radius: 0.75,
                complexity: 0.002,
                auto_select: false,
                square_coarse: false,
            },
            None,
        );
        let best = candidates[0];
        assert!((best.cell_width - cell_width).abs() <= 0.15, "{best:?}");
        assert!((best.cell_height - cell_height).abs() <= 0.15, "{best:?}");
    }

    #[test]
    fn automatic_ranking_prefers_the_fundamental_scale_over_its_divisor() {
        let cell_size = 9.0;
        let image = RgbImage::from_fn(180, 162, |x, y| {
            let cx = (x as f64 / cell_size).floor() as i32;
            let cy = (y as f64 / cell_size).floor() as i32;
            Rgb(source_color(cx, cy).map(|value| (value * 255.0).round() as u8))
        });
        let integral = IntegralImage::new(&image);
        let profiles = EdgeProfiles::new(&image);
        let candidates = search(
            &integral,
            SearchOptions {
                min_width: 2,
                max_width: 18,
                min_height: 2,
                max_height: 18,
                inset_ratio: 0.14,
                phase_step: 0.25,
                dimension_step: 0.1,
                dimension_radius: 0.75,
                complexity: 0.002,
                auto_select: true,
                square_coarse: true,
            },
            Some(&profiles),
        );
        let best = candidates[0];
        assert!((best.cell_width - cell_size).abs() <= 0.11, "{best:?}");
        assert!((best.cell_height - cell_size).abs() <= 0.11, "{best:?}");
    }

    #[test]
    fn fractional_refinement_never_escapes_requested_grid_bounds() {
        let image = RgbImage::from_fn(80, 80, |x, y| {
            let cell_x = x as i32 / 10;
            let cell_y = y as i32 / 10;
            Rgb(source_color(cell_x, cell_y).map(|value| (value * 255.0).round() as u8))
        });
        let integral = IntegralImage::new(&image);
        let profiles = EdgeProfiles::new(&image);
        let candidates = search(
            &integral,
            SearchOptions {
                min_width: 2,
                max_width: 5,
                min_height: 2,
                max_height: 5,
                inset_ratio: 0.14,
                phase_step: 0.25,
                dimension_step: 0.1,
                dimension_radius: 0.75,
                complexity: 0.002,
                auto_select: true,
                square_coarse: true,
            },
            Some(&profiles),
        );
        assert!(candidates.iter().all(|candidate| {
            (2.0..=5.0).contains(&candidate.cell_width)
                && (2.0..=5.0).contains(&candidate.cell_height)
        }));
    }
}
