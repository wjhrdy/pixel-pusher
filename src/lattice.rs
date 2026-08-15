use crate::{grid::Candidate, integral::IntegralImage, palette::CellColor};
use image::RgbImage;
use serde::Serialize;
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub struct LatticeOptions {
    /// Maximum movement of one fitted junction from the regular-grid seed.
    pub radius: f64,
    /// Subpixel increment used while optimizing junction positions.
    pub step: f64,
    /// Penalty for neighboring junctions receiving different displacements.
    pub regularization: f64,
    /// Reward for placing junction axes on coherent source-color boundaries.
    pub edge_weight: f64,
    /// Minimum normalized RGB contrast required before an axis may move.
    pub min_edge_strength: f64,
    /// Number of alternating x/y coordinate-descent passes.
    pub iterations: u32,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct LatticeFitReport {
    pub search_radius: f64,
    pub search_step: f64,
    pub regularization: f64,
    pub edge_weight: f64,
    pub min_edge_strength: f64,
    pub iterations: u32,
    pub initial_vertical_lines: usize,
    pub initial_horizontal_lines: usize,
    pub initialized_corner_nodes: usize,
    pub fitted_nodes: usize,
    pub supported_nodes: usize,
    pub moved_nodes: usize,
    pub x_adjusted_nodes: usize,
    pub y_adjusted_nodes: usize,
    pub max_displacement: f64,
    pub rms_displacement: f64,
    pub min_cell_width: f64,
    pub max_cell_width: f64,
    pub min_cell_height: f64,
    pub max_cell_height: f64,
}

#[derive(Clone, Debug)]
struct AxisSeed {
    first_cell: i32,
    coordinates: Vec<f64>,
}

impl AxisSeed {
    fn new(phase: f64, spacing: f64, limit: f64) -> Self {
        let first_cell = ((-phase) / spacing).floor() as i32;
        let end = ((limit - phase) / spacing).ceil() as i32;
        Self {
            first_cell,
            coordinates: (first_cell..=end)
                .map(|index| phase + index as f64 * spacing)
                .collect(),
        }
    }

    fn cell_range(&self) -> Range<i32> {
        self.first_cell..self.first_cell + self.coordinates.len() as i32 - 1
    }
}

#[derive(Clone, Copy, Debug)]
struct DetectedLine {
    coordinate: f64,
    strength: f64,
}

fn pixel_edge_strength(image: &RgbImage, coordinate: usize, along: usize, axis: usize) -> f64 {
    let (left, right) = if axis == 0 {
        (
            image.get_pixel((coordinate - 1) as u32, along as u32),
            image.get_pixel(coordinate as u32, along as u32),
        )
    } else {
        (
            image.get_pixel(along as u32, (coordinate - 1) as u32),
            image.get_pixel(along as u32, coordinate as u32),
        )
    };
    ((0..3)
        .map(|channel| {
            let difference = right[channel] as f64 - left[channel] as f64;
            (difference / 255.0).powi(2)
        })
        .sum::<f64>()
        / 3.0)
        .sqrt()
}

fn coherent_line_strength(
    image: &RgbImage,
    coordinate: usize,
    axis: usize,
    threshold: f64,
    max_gap: usize,
    minimum_span: usize,
) -> Option<f64> {
    let length = if axis == 0 {
        image.height() as usize
    } else {
        image.width() as usize
    };
    let mut run_start = 0;
    let mut last_strong = 0;
    let mut hits = 0_usize;
    let mut sum_sq = 0.0;
    let mut best = 0.0_f64;
    for along in 0..length {
        let strength = pixel_edge_strength(image, coordinate, along, axis);
        if strength < threshold {
            continue;
        }
        if hits == 0 || along.saturating_sub(last_strong) > max_gap + 1 {
            run_start = along;
            hits = 0;
            sum_sq = 0.0;
        }
        last_strong = along;
        hits += 1;
        sum_sq += strength * strength;
        let span = last_strong - run_start + 1;
        if hits >= 2 && span >= minimum_span {
            let rms = (sum_sq / hits as f64).sqrt();
            best = best.max(rms * (span as f64 / minimum_span as f64).sqrt());
        }
    }
    (best > 0.0).then_some(best)
}

fn cluster_detected_lines(mut lines: Vec<DetectedLine>, threshold: f64) -> Vec<DetectedLine> {
    lines.sort_by(|left, right| left.coordinate.total_cmp(&right.coordinate));
    let mut clustered = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = start + 1;
        while end < lines.len() && lines[end].coordinate - lines[end - 1].coordinate <= threshold {
            end += 1;
        }
        let cluster = &lines[start..end];
        clustered.push(DetectedLine {
            coordinate: cluster[cluster.len() / 2].coordinate,
            strength: cluster.iter().map(|line| line.strength).fold(0.0, f64::max),
        });
        start = end;
    }
    clustered
}

fn detect_axis_lines(
    image: &RgbImage,
    axis: usize,
    spacing: f64,
    orthogonal_spacing: f64,
    minimum_edge_strength: f64,
) -> Vec<DetectedLine> {
    let limit = if axis == 0 {
        image.width() as usize
    } else {
        image.height() as usize
    };
    let maximum_gap = (orthogonal_spacing * 0.45).round().max(1.0) as usize;
    let minimum_span = (orthogonal_spacing * 0.65).round().max(2.0) as usize;
    let candidates = (1..limit)
        .filter_map(|coordinate| {
            coherent_line_strength(
                image,
                coordinate,
                axis,
                minimum_edge_strength,
                maximum_gap,
                minimum_span,
            )
            .map(|strength| DetectedLine {
                coordinate: coordinate as f64,
                strength,
            })
        })
        .collect();
    cluster_detected_lines(candidates, (spacing * 0.35).clamp(1.0, 4.0))
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        0.5 * (values[middle - 1] + values[middle])
    } else {
        values[middle]
    })
}

fn completed_axis_lines(lines: &[DetectedLine], nominal_spacing: f64) -> Vec<f64> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut gaps: Vec<f64> = lines
        .windows(2)
        .map(|pair| pair[1].coordinate - pair[0].coordinate)
        .filter(|gap| *gap > nominal_spacing * 0.45)
        .collect();
    gaps.sort_by(f64::total_cmp);
    let trim = (gaps.len() as f64 * 0.2).floor() as usize;
    let trimmed_end = gaps.len().saturating_sub(trim);
    let estimated = (trim < trimmed_end)
        .then(|| median(&mut gaps[trim..trimmed_end]))
        .flatten()
        .filter(|spacing| *spacing >= nominal_spacing * 0.7 && *spacing <= nominal_spacing * 1.3)
        .unwrap_or(nominal_spacing);

    let mut completed = vec![lines[0].coordinate];
    for pair in lines.windows(2) {
        let gap = pair[1].coordinate - pair[0].coordinate;
        let divisions = (gap / estimated).round().max(1.0) as usize;
        let section_spacing = gap / divisions as f64;
        if divisions > 1
            && section_spacing >= nominal_spacing * 0.6
            && section_spacing <= nominal_spacing * 1.4
        {
            completed.extend(
                (1..divisions)
                    .map(|division| pair[0].coordinate + division as f64 * section_spacing),
            );
        }
        completed.push(pair[1].coordinate);
    }
    completed.sort_by(f64::total_cmp);
    completed.dedup_by(|left, right| (*left - *right).abs() < 1e-9);
    completed
}

fn nearest_line(lines: &[f64], nominal: f64, radius: f64) -> Option<f64> {
    lines
        .iter()
        .copied()
        .min_by(|left, right| (left - nominal).abs().total_cmp(&(right - nominal).abs()))
        .filter(|coordinate| (coordinate - nominal).abs() <= radius + 1e-9)
}

/// A locally deformable 2D source lattice. Junctions may move independently,
/// but every edge remains shared by adjacent cells and row/column ordering is
/// preserved, preventing gaps, overlaps, and isolated per-cell sampling jumps.
#[derive(Clone, Debug)]
pub struct FittedLattice {
    x_seed: AxisSeed,
    y_seed: AxisSeed,
    nominal: Vec<[f64; 2]>,
    nodes: Vec<[f64; 2]>,
    supported: Vec<[bool; 2]>,
    corner_anchors: Vec<bool>,
    initial_lines: [usize; 2],
    initialized_corner_nodes: usize,
    columns: usize,
    rows: usize,
    nominal_width: f64,
    nominal_height: f64,
    width: f64,
    height: f64,
}

impl FittedLattice {
    pub fn regular(grid: Candidate, width: usize, height: usize) -> Self {
        let x_seed = AxisSeed::new(grid.phase_x, grid.cell_width, width as f64);
        let y_seed = AxisSeed::new(grid.phase_y, grid.cell_height, height as f64);
        let columns = x_seed.coordinates.len();
        let rows = y_seed.coordinates.len();
        let nominal: Vec<[f64; 2]> = y_seed
            .coordinates
            .iter()
            .flat_map(|&y| x_seed.coordinates.iter().map(move |&x| [x, y]))
            .collect();
        Self {
            x_seed,
            y_seed,
            nodes: nominal.clone(),
            supported: vec![[false; 2]; nominal.len()],
            corner_anchors: vec![false; nominal.len()],
            initial_lines: [0; 2],
            initialized_corner_nodes: 0,
            nominal,
            columns,
            rows,
            nominal_width: grid.cell_width,
            nominal_height: grid.cell_height,
            width: width as f64,
            height: height as f64,
        }
    }

    fn index(&self, column: usize, row: usize) -> usize {
        row * self.columns + column
    }

    fn position(&self, column: usize, row: usize) -> [f64; 2] {
        self.nodes[self.index(column, row)]
    }

    fn nominal_position(&self, column: usize, row: usize) -> [f64; 2] {
        self.nominal[self.index(column, row)]
    }

    fn position_with(
        &self,
        column: usize,
        row: usize,
        candidate_index: usize,
        candidate: [f64; 2],
    ) -> [f64; 2] {
        let index = self.index(column, row);
        if index == candidate_index {
            candidate
        } else {
            self.nodes[index]
        }
    }

    pub fn cell_x_range(&self) -> Range<i32> {
        self.x_seed.cell_range()
    }

    pub fn cell_y_range(&self) -> Range<i32> {
        self.y_seed.cell_range()
    }

    fn cell_indices(&self, cell_x: i32, cell_y: i32) -> Option<(usize, usize)> {
        let column = usize::try_from(cell_x - self.x_seed.first_cell).ok()?;
        let row = usize::try_from(cell_y - self.y_seed.first_cell).ok()?;
        (column + 1 < self.columns && row + 1 < self.rows).then_some((column, row))
    }

    pub fn cell_corners(&self, cell_x: i32, cell_y: i32) -> Option<[[f64; 2]; 4]> {
        let (column, row) = self.cell_indices(cell_x, cell_y)?;
        Some([
            self.position(column, row),
            self.position(column + 1, row),
            self.position(column + 1, row + 1),
            self.position(column, row + 1),
        ])
    }

    pub fn cell_center(&self, cell_x: i32, cell_y: i32) -> Option<[f64; 2]> {
        self.cell_corners(cell_x, cell_y)
            .map(|corners| bilinear_point(corners, 0.5, 0.5))
    }

    fn cell_corners_with(
        &self,
        column: usize,
        row: usize,
        candidate_index: usize,
        candidate: [f64; 2],
    ) -> [[f64; 2]; 4] {
        [
            self.position_with(column, row, candidate_index, candidate),
            self.position_with(column + 1, row, candidate_index, candidate),
            self.position_with(column + 1, row + 1, candidate_index, candidate),
            self.position_with(column, row + 1, candidate_index, candidate),
        ]
    }

    fn is_regular(&self) -> bool {
        self.nodes.iter().zip(&self.nominal).all(|(node, nominal)| {
            (node[0] - nominal[0]).abs() <= 1e-12 && (node[1] - nominal[1]).abs() <= 1e-12
        })
    }

    pub fn mesh_segments(&self) -> Vec<([f64; 2], [f64; 2])> {
        let horizontal = (0..self.rows).flat_map(|row| {
            (0..self.columns - 1)
                .map(move |column| (self.position(column, row), self.position(column + 1, row)))
        });
        let vertical = (0..self.rows - 1).flat_map(|row| {
            (0..self.columns)
                .map(move |column| (self.position(column, row), self.position(column, row + 1)))
        });
        horizontal.chain(vertical).collect()
    }

    pub fn supported_segments(&self) -> Vec<([f64; 2], [f64; 2])> {
        let mut segments = Vec::new();
        for row in 0..self.rows {
            for column in 0..self.columns - 1 {
                let left = self.index(column, row);
                let right = self.index(column + 1, row);
                if self.supported[left][1] && self.supported[right][1] {
                    segments.push((self.nodes[left], self.nodes[right]));
                }
            }
        }
        for row in 0..self.rows - 1 {
            for column in 0..self.columns {
                let top = self.index(column, row);
                let bottom = self.index(column, row + 1);
                if self.supported[top][0] && self.supported[bottom][0] {
                    segments.push((self.nodes[top], self.nodes[bottom]));
                }
            }
        }
        segments
    }

    pub fn supported_nodes(&self) -> Vec<[f64; 2]> {
        self.nodes
            .iter()
            .zip(&self.corner_anchors)
            .filter(|(_, corner_anchor)| **corner_anchor)
            .map(|(&node, _)| node)
            .collect()
    }

    fn visible_node(&self, node: [f64; 2]) -> bool {
        node[0] > 0.0 && node[0] < self.width && node[1] > 0.0 && node[1] < self.height
    }

    pub fn report(&self, options: LatticeOptions) -> LatticeFitReport {
        let mut fitted_nodes = 0;
        let mut supported_nodes = 0;
        let mut moved_nodes = 0;
        let mut x_adjusted_nodes = 0;
        let mut y_adjusted_nodes = 0;
        let mut max_displacement = 0.0_f64;
        let mut sum_sq = 0.0;
        for ((&node, &nominal), &corner_anchor) in self
            .nodes
            .iter()
            .zip(&self.nominal)
            .zip(&self.corner_anchors)
        {
            if !self.visible_node(nominal) {
                continue;
            }
            fitted_nodes += 1;
            supported_nodes += usize::from(corner_anchor);
            let dx = node[0] - nominal[0];
            let dy = node[1] - nominal[1];
            x_adjusted_nodes += usize::from(dx.abs() > 1e-9);
            y_adjusted_nodes += usize::from(dy.abs() > 1e-9);
            moved_nodes += usize::from(dx.abs() > 1e-9 || dy.abs() > 1e-9);
            let displacement = dx.hypot(dy);
            max_displacement = max_displacement.max(displacement);
            sum_sq += displacement * displacement;
        }
        let (min_cell_width, max_cell_width, min_cell_height, max_cell_height) =
            self.cell_size_ranges();
        LatticeFitReport {
            search_radius: options.radius,
            search_step: options.step,
            regularization: options.regularization,
            edge_weight: options.edge_weight,
            min_edge_strength: options.min_edge_strength,
            iterations: options.iterations,
            initial_vertical_lines: self.initial_lines[0],
            initial_horizontal_lines: self.initial_lines[1],
            initialized_corner_nodes: self.initialized_corner_nodes,
            fitted_nodes,
            supported_nodes,
            moved_nodes,
            x_adjusted_nodes,
            y_adjusted_nodes,
            max_displacement,
            rms_displacement: if fitted_nodes == 0 {
                0.0
            } else {
                (sum_sq / fitted_nodes as f64).sqrt()
            },
            min_cell_width,
            max_cell_width,
            min_cell_height,
            max_cell_height,
        }
    }

    fn cell_size_ranges(&self) -> (f64, f64, f64, f64) {
        let mut min_width = f64::INFINITY;
        let mut max_width = 0.0_f64;
        let mut min_height = f64::INFINITY;
        let mut max_height = 0.0_f64;
        for row in 0..self.rows - 1 {
            for column in 0..self.columns - 1 {
                let corners = self.cell_corners_with(column, row, usize::MAX, [0.0; 2]);
                let width =
                    0.5 * (distance(corners[0], corners[1]) + distance(corners[3], corners[2]));
                let height =
                    0.5 * (distance(corners[0], corners[3]) + distance(corners[1], corners[2]));
                min_width = min_width.min(width);
                max_width = max_width.max(width);
                min_height = min_height.min(height);
                max_height = max_height.max(height);
            }
        }
        (min_width, max_width, min_height, max_height)
    }
}

fn distance(left: [f64; 2], right: [f64; 2]) -> f64 {
    (right[0] - left[0]).hypot(right[1] - left[1])
}

fn bilinear_point(corners: [[f64; 2]; 4], u: f64, v: f64) -> [f64; 2] {
    let [top_left, top_right, bottom_right, bottom_left] = corners;
    std::array::from_fn(|axis| {
        top_left[axis] * (1.0 - u) * (1.0 - v)
            + top_right[axis] * u * (1.0 - v)
            + bottom_right[axis] * u * v
            + bottom_left[axis] * (1.0 - u) * v
    })
}

fn inset_bounds(corners: [[f64; 2]; 4], inset: f64) -> [f64; 4] {
    let points = [
        bilinear_point(corners, inset, inset),
        bilinear_point(corners, 1.0 - inset, inset),
        bilinear_point(corners, 1.0 - inset, 1.0 - inset),
        bilinear_point(corners, inset, 1.0 - inset),
    ];
    points.iter().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |[x0, y0, x1, y1], point| {
            [
                x0.min(point[0]),
                y0.min(point[1]),
                x1.max(point[0]),
                y1.max(point[1]),
            ]
        },
    )
}

fn cell_variance(integral: &IntegralImage, corners: [[f64; 2]; 4], inset: f64) -> Option<f64> {
    let [x0, y0, x1, y1] = inset_bounds(corners, inset);
    let x0 = x0.max(0.0);
    let y0 = y0.max(0.0);
    let x1 = x1.min(integral.width() as f64);
    let y1 = y1.min(integral.height() as f64);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let moments = integral.rect(x0, y0, x1, y1);
    (moments.area > 0.0).then(|| moments.sse() / (moments.area * 3.0))
}

fn color_boundary_strength(left: crate::integral::Moments, right: crate::integral::Moments) -> f64 {
    let left = left.mean();
    let right = right.mean();
    ((0..3)
        .map(|channel| (left[channel] - right[channel]).powi(2))
        .sum::<f64>()
        / 3.0)
        .sqrt()
}

fn rms(strengths: impl Iterator<Item = f64>) -> f64 {
    let (sum_sq, count) = strengths.fold((0.0, 0_u64), |(sum_sq, count), strength| {
        (sum_sq + strength * strength, count + 1)
    });
    if count == 0 {
        0.0
    } else {
        (sum_sq / count as f64).sqrt()
    }
}

fn vertical_edge_strength(
    integral: &IntegralImage,
    lattice: &FittedLattice,
    position: [f64; 2],
) -> f64 {
    let band = (lattice.nominal_width * 0.18).clamp(0.35, 1.0);
    let extent = lattice.nominal_height * 0.48;
    rms([
        (position[1] - extent, position[1]),
        (position[1], position[1] + extent),
    ]
    .into_iter()
    .filter_map(|(y0, y1)| {
        let x0 = (position[0] - band).max(0.0);
        let x1 = (position[0] + band).min(integral.width() as f64);
        let y0 = y0.max(0.0);
        let y1 = y1.min(integral.height() as f64);
        (position[0] > x0 && x1 > position[0] && y1 > y0).then(|| {
            color_boundary_strength(
                integral.rect(x0, y0, position[0], y1),
                integral.rect(position[0], y0, x1, y1),
            )
        })
    }))
}

fn horizontal_edge_strength(
    integral: &IntegralImage,
    lattice: &FittedLattice,
    position: [f64; 2],
) -> f64 {
    let band = (lattice.nominal_height * 0.18).clamp(0.35, 1.0);
    let extent = lattice.nominal_width * 0.48;
    rms([
        (position[0] - extent, position[0]),
        (position[0], position[0] + extent),
    ]
    .into_iter()
    .filter_map(|(x0, x1)| {
        let x0 = x0.max(0.0);
        let x1 = x1.min(integral.width() as f64);
        let y0 = (position[1] - band).max(0.0);
        let y1 = (position[1] + band).min(integral.height() as f64);
        (position[1] > y0 && y1 > position[1] && x1 > x0).then(|| {
            color_boundary_strength(
                integral.rect(x0, y0, x1, position[1]),
                integral.rect(x0, position[1], x1, y1),
            )
        })
    }))
}

fn local_cell_score(
    integral: &IntegralImage,
    lattice: &FittedLattice,
    column: usize,
    row: usize,
    candidate: [f64; 2],
    inset: f64,
) -> f64 {
    let candidate_index = lattice.index(column, row);
    let mut score = 0.0;
    let mut count = 0;
    for cell_row in row.saturating_sub(1)..=(row.min(lattice.rows - 2)) {
        for cell_column in column.saturating_sub(1)..=(column.min(lattice.columns - 2)) {
            let corners =
                lattice.cell_corners_with(cell_column, cell_row, candidate_index, candidate);
            if let Some(variance) = cell_variance(integral, corners, inset) {
                score += variance;
                count += 1;
            }
        }
    }
    score / count.max(1) as f64
}

fn regularization_score(
    lattice: &FittedLattice,
    column: usize,
    row: usize,
    candidate: [f64; 2],
    axis: usize,
) -> f64 {
    let nominal = lattice.nominal_position(column, row);
    let scale = if axis == 0 {
        lattice.nominal_width
    } else {
        lattice.nominal_height
    };
    let displacement = candidate[axis] - nominal[axis];
    let mut score = 0.2 * (displacement / scale).powi(2);
    let mut count = 0;
    for (delta_column, delta_row) in [(-1_i32, 0_i32), (1, 0), (0, -1), (0, 1)] {
        let neighbor_column = column as i32 + delta_column;
        let neighbor_row = row as i32 + delta_row;
        if neighbor_column < 0
            || neighbor_row < 0
            || neighbor_column >= lattice.columns as i32
            || neighbor_row >= lattice.rows as i32
        {
            continue;
        }
        let neighbor_column = neighbor_column as usize;
        let neighbor_row = neighbor_row as usize;
        let neighbor = lattice.position(neighbor_column, neighbor_row);
        let neighbor_nominal = lattice.nominal_position(neighbor_column, neighbor_row);
        let neighbor_displacement = neighbor[axis] - neighbor_nominal[axis];
        score += ((displacement - neighbor_displacement) / scale).powi(2);
        count += 1;
    }
    score / count.max(1) as f64
}

fn candidate_positions(nominal: f64, current: f64, radius: f64, step: f64) -> Vec<f64> {
    let steps = (radius / step).ceil() as i32;
    let mut candidates: Vec<f64> = (-steps..=steps)
        .map(|offset| (nominal + offset as f64 * step).clamp(nominal - radius, nominal + radius))
        .collect();
    candidates.push(current);
    candidates.sort_by(f64::total_cmp);
    candidates.dedup_by(|left, right| (*left - *right).abs() < 1e-9);
    candidates
}

fn axis_edge_strength(
    integral: &IntegralImage,
    lattice: &FittedLattice,
    position: [f64; 2],
    axis: usize,
) -> f64 {
    if axis == 0 {
        vertical_edge_strength(integral, lattice, position)
    } else {
        horizontal_edge_strength(integral, lattice, position)
    }
}

fn corner_evidence(
    integral: &IntegralImage,
    lattice: &FittedLattice,
    position: [f64; 2],
) -> (f64, f64, f64) {
    let vertical = vertical_edge_strength(integral, lattice, position);
    let horizontal = horizontal_edge_strength(integral, lattice, position);
    (vertical, horizontal, (vertical * horizontal).sqrt())
}

fn initialize_lattice_from_lines(
    image: &RgbImage,
    integral: &IntegralImage,
    lattice: &mut FittedLattice,
    options: LatticeOptions,
) {
    let detected_x = detect_axis_lines(
        image,
        0,
        lattice.nominal_width,
        lattice.nominal_height,
        options.min_edge_strength,
    );
    let detected_y = detect_axis_lines(
        image,
        1,
        lattice.nominal_height,
        lattice.nominal_width,
        options.min_edge_strength,
    );
    lattice.initial_lines = [detected_x.len(), detected_y.len()];
    if detected_x.is_empty() || detected_y.is_empty() {
        return;
    }
    let completed_x = completed_axis_lines(&detected_x, lattice.nominal_width);
    let completed_y = completed_axis_lines(&detected_y, lattice.nominal_height);
    let x_matches: Vec<Option<f64>> = lattice
        .x_seed
        .coordinates
        .iter()
        .map(|&nominal| nearest_line(&completed_x, nominal, options.radius))
        .collect();
    let y_matches: Vec<Option<f64>> = lattice
        .y_seed
        .coordinates
        .iter()
        .map(|&nominal| nearest_line(&completed_y, nominal, options.radius))
        .collect();
    for (row, y_match) in y_matches.iter().enumerate() {
        let Some(y) = *y_match else { continue };
        for (column, x_match) in x_matches.iter().enumerate() {
            let Some(x) = *x_match else { continue };
            let nominal = lattice.nominal_position(column, row);
            if !lattice.visible_node(nominal) {
                continue;
            }
            let candidate = [x, y];
            let (lower, upper) = coordinate_bounds(lattice, column, row);
            if !candidate_is_valid(candidate, nominal, lower, upper, options.radius) {
                continue;
            }
            let (vertical, horizontal, _) = corner_evidence(integral, lattice, candidate);
            if vertical >= options.min_edge_strength && horizontal >= options.min_edge_strength {
                let index = lattice.index(column, row);
                lattice.nodes[index] = candidate;
                lattice.initialized_corner_nodes += 1;
            }
        }
    }
}

fn within_radius(position: [f64; 2], nominal: [f64; 2], radius: f64) -> bool {
    distance(position, nominal) <= radius + 1e-9
}

fn coordinate_bounds(lattice: &FittedLattice, column: usize, row: usize) -> ([f64; 2], [f64; 2]) {
    let minimum_width = lattice.nominal_width * 0.35;
    let minimum_height = lattice.nominal_height * 0.35;
    let lower = [
        if column > 0 {
            lattice.position(column - 1, row)[0] + minimum_width
        } else {
            f64::NEG_INFINITY
        },
        if row > 0 {
            lattice.position(column, row - 1)[1] + minimum_height
        } else {
            f64::NEG_INFINITY
        },
    ];
    let upper = [
        if column + 1 < lattice.columns {
            lattice.position(column + 1, row)[0] - minimum_width
        } else {
            f64::INFINITY
        },
        if row + 1 < lattice.rows {
            lattice.position(column, row + 1)[1] - minimum_height
        } else {
            f64::INFINITY
        },
    ];
    (lower, upper)
}

fn candidate_is_valid(
    candidate: [f64; 2],
    nominal: [f64; 2],
    lower: [f64; 2],
    upper: [f64; 2],
    radius: f64,
) -> bool {
    candidate[0] >= lower[0]
        && candidate[0] <= upper[0]
        && candidate[1] >= lower[1]
        && candidate[1] <= upper[1]
        && within_radius(candidate, nominal, radius)
}

struct CornerSearch<'a> {
    integral: &'a IntegralImage,
    lattice: &'a FittedLattice,
    column: usize,
    row: usize,
    nominal: [f64; 2],
    inset: f64,
    options: LatticeOptions,
}

#[derive(Clone, Copy)]
struct CornerBest {
    position: [f64; 2],
    score: f64,
    strength: f64,
}

impl CornerSearch<'_> {
    fn consider(&self, candidate: [f64; 2], best: &mut CornerBest) {
        let (vertical, horizontal, corner) =
            corner_evidence(self.integral, self.lattice, candidate);
        if vertical < self.options.min_edge_strength || horizontal < self.options.min_edge_strength
        {
            return;
        }
        let regularization = 0.5
            * (regularization_score(self.lattice, self.column, self.row, candidate, 0)
                + regularization_score(self.lattice, self.column, self.row, candidate, 1));
        let score = local_cell_score(
            self.integral,
            self.lattice,
            self.column,
            self.row,
            candidate,
            self.inset,
        ) + self.options.regularization * regularization
            - self.options.edge_weight * corner;
        if score < best.score - 1e-12
            || ((score - best.score).abs() <= 1e-12
                && distance(candidate, self.nominal) < distance(best.position, self.nominal))
        {
            *best = CornerBest {
                position: candidate,
                score,
                strength: corner,
            };
        }
    }
}

fn fit_corner_node(
    integral: &IntegralImage,
    lattice: &mut FittedLattice,
    column: usize,
    row: usize,
    inset: f64,
    options: LatticeOptions,
) {
    let index = lattice.index(column, row);
    let nominal = lattice.nominal[index];
    if !lattice.visible_node(nominal) {
        return;
    }
    let current = lattice.nodes[index];
    let (lower, upper) = coordinate_bounds(lattice, column, row);
    let (_, _, current_corner) = corner_evidence(integral, lattice, current);
    let search = CornerSearch {
        integral,
        lattice,
        column,
        row,
        nominal,
        inset,
        options,
    };
    let mut best = CornerBest {
        position: current,
        score: f64::INFINITY,
        strength: 0.0,
    };
    search.consider(current, &mut best);

    let coarse_step = options.step.max(options.radius / 4.0);
    let coarse_x = candidate_positions(nominal[0], current[0], options.radius, coarse_step);
    let coarse_y = candidate_positions(nominal[1], current[1], options.radius, coarse_step);
    for &y in &coarse_y {
        for &x in &coarse_x {
            let candidate = [x, y];
            if candidate_is_valid(candidate, nominal, lower, upper, options.radius) {
                search.consider(candidate, &mut best);
            }
        }
    }
    if best.score.is_infinite() {
        return;
    }

    let refine_x = candidate_positions(
        best.position[0],
        best.position[0],
        coarse_step,
        options.step,
    );
    let refine_y = candidate_positions(
        best.position[1],
        best.position[1],
        coarse_step,
        options.step,
    );
    for &y in &refine_y {
        for &x in &refine_x {
            let candidate = [x, y];
            if candidate_is_valid(candidate, nominal, lower, upper, options.radius) {
                search.consider(candidate, &mut best);
            }
        }
    }

    let required_gain = if distance(current, nominal) <= 1e-9 {
        options.min_edge_strength * 0.1
    } else {
        0.0
    };
    if best.strength + 1e-12 >= current_corner + required_gain {
        lattice.nodes[index] = best.position;
    }
}

fn anchor_target(
    lattice: &FittedLattice,
    column: usize,
    row: usize,
    axis: usize,
) -> Option<(f64, f64)> {
    let mut weighted_displacement = 0.0;
    let mut weight_sum = 0.0;
    if axis == 0 {
        for anchor_row in 0..lattice.rows {
            let index = lattice.index(column, anchor_row);
            if !lattice.corner_anchors[index] || anchor_row == row {
                continue;
            }
            let distance = row.abs_diff(anchor_row) as f64;
            let weight = 1.0 / distance.max(1.0);
            weighted_displacement += (lattice.nodes[index][0] - lattice.nominal[index][0]) * weight;
            weight_sum += weight;
        }
    } else {
        for anchor_column in 0..lattice.columns {
            let index = lattice.index(anchor_column, row);
            if !lattice.corner_anchors[index] || anchor_column == column {
                continue;
            }
            let distance = column.abs_diff(anchor_column) as f64;
            let weight = 1.0 / distance.max(1.0);
            weighted_displacement += (lattice.nodes[index][1] - lattice.nominal[index][1]) * weight;
            weight_sum += weight;
        }
    }
    (weight_sum > 0.0).then(|| (weighted_displacement / weight_sum, weight_sum.min(2.0)))
}

fn fit_line_axis(
    integral: &IntegralImage,
    lattice: &mut FittedLattice,
    column: usize,
    row: usize,
    axis: usize,
    inset: f64,
    options: LatticeOptions,
) {
    let index = lattice.index(column, row);
    if lattice.corner_anchors[index] {
        return;
    }
    let nominal = lattice.nominal[index];
    if !lattice.visible_node(nominal) {
        return;
    }
    let Some((target_displacement, anchor_confidence)) = anchor_target(lattice, column, row, axis)
    else {
        return;
    };
    let current = lattice.nodes[index];
    let other_axis = 1 - axis;
    let other_displacement = current[other_axis] - nominal[other_axis];
    let axis_radius = (options.radius * options.radius - other_displacement * other_displacement)
        .max(0.0)
        .sqrt();
    let (lower, upper) = coordinate_bounds(lattice, column, row);
    let scale = if axis == 0 {
        lattice.nominal_width
    } else {
        lattice.nominal_height
    };
    let anchor_weight = options.edge_weight + options.regularization;
    let score = |candidate: [f64; 2], edge: f64| {
        let displacement = candidate[axis] - nominal[axis];
        local_cell_score(integral, lattice, column, row, candidate, inset)
            + options.regularization * regularization_score(lattice, column, row, candidate, axis)
            + anchor_weight
                * anchor_confidence
                * ((displacement - target_displacement) / scale).powi(2)
            - options.edge_weight * edge
    };

    let current_edge = axis_edge_strength(integral, lattice, current, axis);
    let mut best = current;
    let mut best_edge = current_edge;
    let mut best_score = if current_edge >= options.min_edge_strength {
        score(current, current_edge)
    } else {
        f64::INFINITY
    };
    for coordinate in candidate_positions(nominal[axis], current[axis], axis_radius, options.step) {
        let mut candidate = current;
        candidate[axis] = coordinate;
        if candidate[axis] < lower[axis]
            || candidate[axis] > upper[axis]
            || !within_radius(candidate, nominal, options.radius)
        {
            continue;
        }
        let edge = axis_edge_strength(integral, lattice, candidate, axis);
        if edge < options.min_edge_strength {
            continue;
        }
        let candidate_score = score(candidate, edge);
        if candidate_score < best_score - 1e-12
            || ((candidate_score - best_score).abs() <= 1e-12
                && (coordinate - (nominal[axis] + target_displacement)).abs()
                    < (best[axis] - (nominal[axis] + target_displacement)).abs())
        {
            best = candidate;
            best_edge = edge;
            best_score = candidate_score;
        }
    }
    if best_score.is_finite() && best_edge >= options.min_edge_strength {
        lattice.nodes[index] = best;
    }
}

pub fn fit_lattice(
    image: &RgbImage,
    integral: &IntegralImage,
    grid: Candidate,
    inset: f64,
    options: LatticeOptions,
) -> FittedLattice {
    let mut lattice = FittedLattice::regular(grid, integral.width(), integral.height());

    // Seed a rectilinear mesh from coherent, clustered source boundaries and
    // complete missing lines between reliable corner intersections. This is
    // adapted from proper-pixel-art's line-first mesh initialization; the
    // corner-first and distance-weighted local refinements below remain ours.
    initialize_lattice_from_lines(image, integral, &mut lattice, options);

    // First establish reliable 2D anchors from coincident vertical and
    // horizontal evidence. These are the highest-confidence lattice points.
    for _ in 0..options.iterations {
        for row in 0..lattice.rows {
            for column in 0..lattice.columns {
                fit_corner_node(integral, &mut lattice, column, row, inset, options);
            }
        }
    }
    for row in 0..lattice.rows {
        for column in 0..lattice.columns {
            let index = lattice.index(column, row);
            let position = lattice.nodes[index];
            let (vertical, horizontal, _) = corner_evidence(integral, &lattice, position);
            lattice.corner_anchors[index] =
                vertical >= options.min_edge_strength && horizontal >= options.min_edge_strength;
        }
    }

    // Then allow edge-only points to refine one axis, but only when corner
    // anchors in that logical row or column provide a distance-weighted target.
    for _ in 0..options.iterations {
        for row in 0..lattice.rows {
            for column in 0..lattice.columns {
                fit_line_axis(integral, &mut lattice, column, row, 0, inset, options);
                fit_line_axis(integral, &mut lattice, column, row, 1, inset, options);
            }
        }
    }

    for row in 0..lattice.rows {
        for column in 0..lattice.columns {
            let index = lattice.index(column, row);
            if lattice.corner_anchors[index] {
                lattice.supported[index] = [true; 2];
                continue;
            }
            let position = lattice.nodes[index];
            lattice.supported[index] = [
                anchor_target(&lattice, column, row, 0).is_some()
                    && vertical_edge_strength(integral, &lattice, position)
                        >= options.min_edge_strength,
                anchor_target(&lattice, column, row, 1).is_some()
                    && horizontal_edge_strength(integral, &lattice, position)
                        >= options.min_edge_strength,
            ];
        }
    }
    lattice
}

fn extract_regular_cells(
    integral: &IntegralImage,
    lattice: &FittedLattice,
    inset: f64,
) -> Vec<CellColor> {
    let mut cells = Vec::new();
    for cell_y in lattice.cell_y_range() {
        for cell_x in lattice.cell_x_range() {
            let Some(corners) = lattice.cell_corners(cell_x, cell_y) else {
                continue;
            };
            let [outer_x0, outer_y0] = corners[0];
            let [outer_x1, outer_y1] = corners[2];
            let width = outer_x1 - outer_x0;
            let height = outer_y1 - outer_y0;
            let x0 = (outer_x0 + width * inset).max(outer_x0.max(0.0));
            let y0 = (outer_y0 + height * inset).max(outer_y0.max(0.0));
            let x1 = (outer_x1 - width * inset).min(outer_x1.min(integral.width() as f64));
            let y1 = (outer_y1 - height * inset).min(outer_y1.min(integral.height() as f64));
            let moments = if x1 > x0 && y1 > y0 {
                integral.rect(x0, y0, x1, y1)
            } else {
                integral.rect(
                    outer_x0.max(0.0),
                    outer_y0.max(0.0),
                    outer_x1.min(integral.width() as f64),
                    outer_y1.min(integral.height() as f64),
                )
            };
            if moments.area > 0.0 {
                cells.push(CellColor {
                    cell_x,
                    cell_y,
                    rgb: moments.mean(),
                    weight: moments.area,
                });
            }
        }
    }
    cells
}

fn bilinear_rgb(image: &RgbImage, point: [f64; 2]) -> Option<[f64; 3]> {
    if point[0] < 0.0
        || point[1] < 0.0
        || point[0] > image.width() as f64
        || point[1] > image.height() as f64
    {
        return None;
    }
    let x = (point[0] - 0.5).clamp(0.0, image.width().saturating_sub(1) as f64);
    let y = (point[1] - 0.5).clamp(0.0, image.height().saturating_sub(1) as f64);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let tx = x - x0 as f64;
    let ty = y - y0 as f64;
    let samples = [
        (image.get_pixel(x0, y0).0, (1.0 - tx) * (1.0 - ty)),
        (image.get_pixel(x1, y0).0, tx * (1.0 - ty)),
        (image.get_pixel(x1, y1).0, tx * ty),
        (image.get_pixel(x0, y1).0, (1.0 - tx) * ty),
    ];
    Some(std::array::from_fn(|channel| {
        samples
            .iter()
            .map(|(pixel, weight)| pixel[channel] as f64 / 255.0 * weight)
            .sum()
    }))
}

fn quadrilateral_area(corners: [[f64; 2]; 4]) -> f64 {
    let mut twice_area = 0.0;
    for index in 0..4 {
        let next = (index + 1) % 4;
        twice_area += corners[index][0] * corners[next][1] - corners[next][0] * corners[index][1];
    }
    twice_area.abs() * 0.5
}

pub fn extract_cells(
    image: &RgbImage,
    integral: &IntegralImage,
    lattice: &FittedLattice,
    inset: f64,
) -> Vec<CellColor> {
    if lattice.is_regular() {
        return extract_regular_cells(integral, lattice, inset);
    }

    let interior = 1.0 - 2.0 * inset;
    let mut cells = Vec::new();
    for cell_y in lattice.cell_y_range() {
        for cell_x in lattice.cell_x_range() {
            let Some(corners) = lattice.cell_corners(cell_x, cell_y) else {
                continue;
            };
            let width = 0.5 * (distance(corners[0], corners[1]) + distance(corners[3], corners[2]));
            let height =
                0.5 * (distance(corners[0], corners[3]) + distance(corners[1], corners[2]));
            let samples_x = (width * interior * 2.0).ceil().max(2.0) as usize;
            let samples_y = (height * interior * 2.0).ceil().max(2.0) as usize;
            let mut sum = [0.0; 3];
            let mut count = 0_u64;
            for sample_y in 0..samples_y {
                let v = inset + (sample_y as f64 + 0.5) / samples_y as f64 * interior;
                for sample_x in 0..samples_x {
                    let u = inset + (sample_x as f64 + 0.5) / samples_x as f64 * interior;
                    let point = bilinear_point(corners, u, v);
                    let Some(rgb) = bilinear_rgb(image, point) else {
                        continue;
                    };
                    for channel in 0..3 {
                        sum[channel] += rgb[channel];
                    }
                    count += 1;
                }
            }
            if count > 0 {
                let area_per_sample = quadrilateral_area(corners) * interior * interior
                    / (samples_x * samples_y) as f64;
                cells.push(CellColor {
                    cell_x,
                    cell_y,
                    rgb: sum.map(|value| value / count as f64),
                    weight: area_per_sample * count as f64,
                });
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn candidate(width: f64, height: f64) -> Candidate {
        Candidate {
            cell_width: width,
            cell_height: height,
            phase_x: 0.0,
            phase_y: 0.0,
            score: 0.0,
            normalized_residual: 0.0,
            sampled_cells: 3,
            edge_alignment: 1.0,
            auto_score: 0.0,
        }
    }

    fn options() -> LatticeOptions {
        LatticeOptions {
            radius: 2.0,
            step: 0.25,
            regularization: 0.002,
            edge_weight: 0.08,
            min_edge_strength: 0.04,
            iterations: 3,
        }
    }

    #[test]
    fn corner_anchor_guides_edge_points_in_the_same_column() {
        let image = RgbImage::from_fn(12, 12, |x, y| {
            if y < 4 {
                if x < 9 {
                    Rgb([255, 0, 0])
                } else {
                    Rgb([0, 0, 255])
                }
            } else if x < 9 {
                Rgb([0, 255, 0])
            } else {
                Rgb([255, 255, 0])
            }
        });
        let integral = IntegralImage::new(&image);
        let lattice = fit_lattice(&image, &integral, candidate(4.0, 4.0), 0.1, options());
        let corner = lattice.position(2, 1);
        let edge_only = lattice.position(2, 2);
        assert!(distance(corner, [9.0, 4.0]) <= 0.25, "corner: {corner:?}");
        assert!(
            (edge_only[0] - 9.0).abs() <= 0.25 && (edge_only[1] - 8.0).abs() < 1e-9,
            "edge-only point: {edge_only:?}"
        );
    }

    #[test]
    fn an_unanchored_hard_edge_does_not_deform_the_mesh() {
        let image = RgbImage::from_fn(12, 12, |x, _| {
            if x < 9 {
                Rgb([255, 255, 255])
            } else {
                Rgb([0, 0, 0])
            }
        });
        let integral = IntegralImage::new(&image);
        let lattice = fit_lattice(&image, &integral, candidate(4.0, 4.0), 0.1, options());
        let report = lattice.report(options());
        assert_eq!(report.supported_nodes, 0);
        assert_eq!(report.moved_nodes, 0);
    }

    #[test]
    fn flat_regions_keep_every_junction_on_the_seed_mesh() {
        let image = RgbImage::from_pixel(12, 8, Rgb([32, 32, 32]));
        let integral = IntegralImage::new(&image);
        let lattice = fit_lattice(&image, &integral, candidate(4.0, 4.0), 0.1, options());
        let report = lattice.report(options());
        assert_eq!(report.supported_nodes, 0);
        assert_eq!(report.moved_nodes, 0);
        assert!(report.max_displacement < 1e-9);
    }

    #[test]
    fn low_contrast_detail_cannot_pull_a_junction() {
        let image = RgbImage::from_fn(12, 8, |x, _| {
            if x < 9 {
                Rgb([100, 100, 100])
            } else {
                Rgb([106, 106, 106])
            }
        });
        let integral = IntegralImage::new(&image);
        let mut strict = options();
        strict.regularization = 0.0;
        strict.edge_weight = 1.0;
        let lattice = fit_lattice(&image, &integral, candidate(4.0, 4.0), 0.1, strict);
        let report = lattice.report(strict);
        assert_eq!(report.moved_nodes, 0);
    }

    #[test]
    fn every_cell_keeps_shared_ordered_corners() {
        let image = RgbImage::from_fn(12, 8, |x, y| {
            if x < 9 && y < 5 {
                Rgb([255, 255, 255])
            } else {
                Rgb([0, 0, 0])
            }
        });
        let integral = IntegralImage::new(&image);
        let lattice = fit_lattice(&image, &integral, candidate(4.0, 4.0), 0.1, options());
        assert!(lattice.report(options()).max_displacement <= options().radius + 1e-9);
        for cell_y in lattice.cell_y_range() {
            for cell_x in lattice.cell_x_range() {
                let corners = lattice.cell_corners(cell_x, cell_y).unwrap();
                assert!(corners[0][0] < corners[1][0]);
                assert!(corners[3][0] < corners[2][0]);
                assert!(corners[0][1] < corners[3][1]);
                assert!(corners[1][1] < corners[2][1]);
            }
        }
    }

    #[test]
    fn line_initializer_seeds_a_shifted_rectilinear_corner_before_refinement() {
        let x_lines = [0_u32, 4, 9, 13, 17, 21, 24];
        let y_lines = [0_u32, 4, 9, 13, 16];
        let image = RgbImage::from_fn(24, 16, |x, y| {
            let cell_x = x_lines
                .windows(2)
                .position(|bounds| x >= bounds[0] && x < bounds[1])
                .unwrap();
            let cell_y = y_lines
                .windows(2)
                .position(|bounds| y >= bounds[0] && y < bounds[1])
                .unwrap();
            if (cell_x + cell_y) % 2 == 0 {
                Rgb([245, 245, 245])
            } else {
                Rgb([15, 15, 15])
            }
        });
        let integral = IntegralImage::new(&image);
        let mut initializer_only = options();
        initializer_only.iterations = 0;

        let lattice = fit_lattice(
            &image,
            &integral,
            candidate(4.0, 4.0),
            0.1,
            initializer_only,
        );

        let initialized = lattice.position(2, 2);
        assert!(distance(initialized, [9.0, 9.0]) <= 0.25, "{initialized:?}");
        let report = lattice.report(initializer_only);
        assert!(report.initial_vertical_lines >= 4, "{report:?}");
        assert!(report.initial_horizontal_lines >= 3, "{report:?}");
        assert!(report.initialized_corner_nodes > 0, "{report:?}");
    }

    #[test]
    fn line_initializer_completes_missing_boundaries_at_nominal_spacing() {
        let lines = [
            DetectedLine {
                coordinate: 4.0,
                strength: 1.0,
            },
            DetectedLine {
                coordinate: 12.0,
                strength: 1.0,
            },
        ];

        assert_eq!(completed_axis_lines(&lines, 4.0), vec![4.0, 8.0, 12.0]);
    }
}
