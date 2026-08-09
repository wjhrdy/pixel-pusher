use crate::color::{distance_squared, srgb_to_oklab};

#[derive(Clone, Debug)]
pub struct CellColor {
    pub cell_x: i32,
    pub cell_y: i32,
    pub rgb: [f64; 3],
    pub weight: f64,
}

#[derive(Clone, Copy, Debug)]
struct Center {
    lab: [f64; 3],
    rgb: [f64; 3],
    weight: f64,
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
