use anyhow::{Context, Result, bail};
use clap::Parser;
use image::{ImageReader, Rgb, RgbImage};
use pixel_pusher::{
    cell_warp::{CellWarpOptions, CellWarpReport, refine_cell_samples},
    color::rgb8,
    geometry::{Quad, estimated_size, rectify},
    grid::{Candidate, EdgeProfiles, SearchOptions, search},
    indexed_png,
    integral::IntegralImage,
    metrics::{OutputMetrics, measure_output},
    palette::{
        CellColor, PaletteSelectionReport, SmartPaletteOptions, cluster, nearest_palette_index,
        select_smart_palette,
    },
    ramp::{RampOptions, RampReport, penalize_one_cell_ramps},
    warp::{WarpField, WarpOptions, fit_local_warp},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Parser, Debug)]
#[command(about = "Recover a clean pixel grid and compact palette from imperfect pixel art")]
struct Args {
    /// Source PNG, JPEG, or WebP.
    input: PathBuf,

    /// Corrected output image. Defaults to <input>.corrected.png.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Automatically select scale, squeeze, phase, local warp, palette, and output scale.
    #[arg(
        long,
        conflicts_with_all = ["block", "block_width", "block_height", "local_warp"]
    )]
    auto: bool,

    /// Smallest source-pixel cell dimension to test in both directions.
    #[arg(long, default_value_t = 2)]
    min_block: u32,

    /// Largest source-pixel cell dimension to test in both directions.
    #[arg(long, default_value_t = 16)]
    max_block: u32,

    /// Ignore this fraction of each cell on every side (0.0–0.45).
    #[arg(long, default_value_t = 0.14)]
    inset: f64,

    /// Fractional source-pixel phase increment used during refinement.
    #[arg(long, default_value_t = 0.25)]
    phase_step: f64,

    /// Source cell-dimension increment used to fit global squeeze.
    #[arg(long, default_value_t = 0.1)]
    dimension_step: f64,

    /// Maximum fractional dimension change around each integer candidate.
    #[arg(long, default_value_t = 0.75)]
    dimension_radius: f64,

    /// Perceptual merge radius for nearly identical cell colors.
    #[arg(long, default_value_t = 0.035)]
    merge_threshold: f64,

    /// Hard upper bound for the final palette (maximum 256 for indexed PNG).
    #[arg(long)]
    max_colors: Option<usize>,

    /// Use edge-aware candidate palettes and automatic color-count selection.
    #[arg(long)]
    smart_palette: bool,

    /// Largest fixed palette considered by smart selection.
    #[arg(long, default_value_t = 12)]
    palette_candidate_max: usize,

    /// Complexity cost for each smart-palette color beyond two.
    #[arg(long, default_value_t = 0.08)]
    palette_penalty: f64,

    /// Extra weight given to colors on high-contrast logical-pixel edges.
    #[arg(long, default_value_t = 1.0)]
    palette_edge_emphasis: f64,

    /// Favor larger logical pixels when multiple grids fit equally well.
    #[arg(long, default_value_t = 0.002)]
    complexity: f64,

    /// Force a square cell size instead of selecting the best searched dimensions.
    #[arg(long)]
    block: Option<u32>,

    /// Force a cell width; may be combined with --block-height.
    #[arg(long)]
    block_width: Option<u32>,

    /// Force a cell height; may be combined with --block-width.
    #[arg(long)]
    block_height: Option<u32>,

    /// Square output-pixel scale. Defaults to the area-preserving source size.
    #[arg(long)]
    output_block: Option<u32>,

    /// Photograph corners in TL;TR;BR;BL order: "x,y;x,y;x,y;x,y".
    #[arg(long, value_name = "TL;TR;BR;BL")]
    corners: Option<Quad>,

    /// Width of the perspective-rectified working image (estimated by default).
    #[arg(long, requires = "corners")]
    rectified_width: Option<u32>,

    /// Height of the perspective-rectified working image (estimated by default).
    #[arg(long, requires = "corners")]
    rectified_height: Option<u32>,

    /// Fit a smooth local grid-displacement field after rigid grid detection.
    #[arg(long)]
    local_warp: bool,

    /// Source-pixel spacing between local warp control points.
    #[arg(long, default_value_t = 64)]
    warp_patch: u32,

    /// Maximum local grid displacement in source pixels.
    #[arg(long, default_value_t = 1.5)]
    warp_radius: f64,

    /// Displacement increment used by each local search.
    #[arg(long, default_value_t = 0.5)]
    warp_step: f64,

    /// Neighbor regularization strength for the displacement field.
    #[arg(long, default_value_t = 1.5)]
    warp_smoothness: f64,

    /// Refine individual source-cell sampling after the smooth local warp.
    #[arg(long)]
    cell_warp: bool,

    /// Maximum per-cell residual sampling displacement in source pixels.
    #[arg(long, default_value_t = 1.5)]
    cell_warp_radius: f64,

    /// Per-cell residual displacement search increment.
    #[arg(long, default_value_t = 0.25)]
    cell_warp_step: f64,

    /// Cost that discourages unnecessary per-cell movement.
    #[arg(long, default_value_t = 0.006)]
    cell_warp_movement: f64,

    /// Minimum relative variance reduction required to accept a per-cell shift.
    #[arg(long, default_value_t = 0.18)]
    cell_warp_min_improvement: f64,

    /// Minimum mixed-color variance required before a cell can shift.
    #[arg(long, default_value_t = 0.0008)]
    cell_warp_min_variance: f64,

    /// Minimum Oklab contrast with a neighboring cell required before shifting.
    #[arg(long, default_value_t = 0.10)]
    cell_warp_contrast: f64,

    /// Minimum Oklab neighbor-contrast increase required from a per-cell shift.
    #[arg(long, default_value_t = 0.015)]
    cell_warp_min_contrast_gain: f64,

    /// Penalty for a one-cell color ramp between high-contrast neighbors; 0 disables it.
    #[arg(long, default_value_t = 0.3)]
    ramp_penalty: f64,

    /// Minimum Oklab distance between the two sides of a penalized ramp.
    #[arg(long, default_value_t = 0.08)]
    ramp_contrast: f64,

    /// Maximum Oklab distance of the middle color from the endpoint color segment.
    #[arg(long, default_value_t = 0.035)]
    ramp_line_tolerance: f64,

    /// Maximum color drift allowed as each side continues away from a one-cell ramp.
    #[arg(long, default_value_t = 0.04)]
    ramp_continuation: f64,

    /// Maximum simultaneous ramp-cleanup passes.
    #[arg(long, default_value_t = 1)]
    ramp_passes: u32,
}

#[derive(Serialize)]
struct Report {
    source: String,
    output: String,
    overlay: String,
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    output_block: u32,
    output_color_type: &'static str,
    output_bit_depth: u8,
    source_width: u32,
    source_height: u32,
    perspective: Option<PerspectiveReport>,
    local_warp: Option<WarpReport>,
    cell_warp: Option<CellWarpReport>,
    ramp_cleanup: RampReport,
    selected: Candidate,
    palette: Vec<[u8; 3]>,
    palette_selection: Option<PaletteSelectionReport>,
    output_metrics: OutputMetrics,
    candidates: Vec<Candidate>,
    settings: ReportSettings,
}

#[derive(Serialize)]
struct WarpReport {
    patch_size: u32,
    search_radius: f64,
    search_step: f64,
    smoothness: f64,
    max_displacement: f64,
    rms_displacement: f64,
}

#[derive(Serialize)]
struct PerspectiveReport {
    corners: Quad,
    rectified_width: u32,
    rectified_height: u32,
}

#[derive(Serialize)]
struct ReportSettings {
    auto: bool,
    inset_ratio: f64,
    phase_step: f64,
    dimension_step: f64,
    dimension_radius: f64,
    merge_threshold: f64,
    max_colors: Option<usize>,
    smart_palette: bool,
    palette_candidate_max: usize,
    palette_penalty: f64,
    palette_edge_emphasis: f64,
    complexity: f64,
}

fn suffixed_path(input: &Path, suffix: &str) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    input.with_file_name(format!("{stem}{suffix}"))
}

fn cell_range(phase: f64, block: f64, limit: f64) -> std::ops::Range<i32> {
    let start = ((-phase) / block).floor() as i32;
    let end = ((limit - phase) / block).ceil() as i32;
    start..end
}

fn extract_cells(integral: &IntegralImage, grid: Candidate, inset: f64) -> Vec<CellColor> {
    let block_width = grid.cell_width;
    let block_height = grid.cell_height;
    let margin_x = block_width * inset;
    let margin_y = block_height * inset;
    let mut cells = Vec::new();
    for cell_y in cell_range(grid.phase_y, block_height, integral.height() as f64) {
        let outer_y0 = grid.phase_y + cell_y as f64 * block_height;
        let outer_y1 = outer_y0 + block_height;
        for cell_x in cell_range(grid.phase_x, block_width, integral.width() as f64) {
            let outer_x0 = grid.phase_x + cell_x as f64 * block_width;
            let outer_x1 = outer_x0 + block_width;
            let x0 = (outer_x0 + margin_x).max(outer_x0.max(0.0));
            let y0 = (outer_y0 + margin_y).max(outer_y0.max(0.0));
            let x1 = (outer_x1 - margin_x).min(outer_x1.min(integral.width() as f64));
            let y1 = (outer_y1 - margin_y).min(outer_y1.min(integral.height() as f64));
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

fn extract_warped_cells(
    image: &RgbImage,
    grid: Candidate,
    inset: f64,
    warp: &WarpField,
) -> Vec<CellColor> {
    #[derive(Default)]
    struct Accumulator {
        sum: [f64; 3],
        count: f64,
    }

    let mut accumulators: HashMap<(i32, i32), Accumulator> = HashMap::new();
    let block_width = grid.cell_width;
    let block_height = grid.cell_height;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let center_x = x as f64 + 0.5;
            let center_y = y as f64 + 0.5;
            let displacement = warp.displacement(center_x, center_y);
            let logical_x = (center_x - grid.phase_x - displacement[0]) / block_width;
            let logical_y = (center_y - grid.phase_y - displacement[1]) / block_height;
            let fraction_x = logical_x.rem_euclid(1.0);
            let fraction_y = logical_y.rem_euclid(1.0);
            if fraction_x < inset
                || fraction_x > 1.0 - inset
                || fraction_y < inset
                || fraction_y > 1.0 - inset
            {
                continue;
            }
            let accumulator = accumulators
                .entry((logical_x.floor() as i32, logical_y.floor() as i32))
                .or_default();
            let pixel = image.get_pixel(x, y).0;
            for (sum, value) in accumulator.sum.iter_mut().zip(pixel) {
                *sum += value as f64 / 255.0;
            }
            accumulator.count += 1.0;
        }
    }
    let mut cells: Vec<CellColor> = accumulators
        .into_iter()
        .filter(|(_, accumulator)| accumulator.count > 0.0)
        .map(|((cell_x, cell_y), accumulator)| CellColor {
            cell_x,
            cell_y,
            rgb: accumulator.sum.map(|sum| sum / accumulator.count),
            weight: accumulator.count,
        })
        .collect();
    // Palette threshold merging is intentionally incremental, so stabilize
    // cell order rather than inheriting randomized HashMap iteration order.
    cells.sort_by_key(|cell| (cell.cell_y, cell.cell_x));
    cells
}

fn main() -> Result<()> {
    let args = Args::parse();
    let automatic = args.auto;
    if args.min_block < 1 || args.max_block < args.min_block {
        bail!("block range must satisfy 1 <= min-block <= max-block");
    }
    if !(0.0..0.45).contains(&args.inset) {
        bail!("inset must be at least 0 and less than 0.45");
    }
    if !(0.0..=1.0).contains(&args.phase_step) || args.phase_step == 0.0 {
        bail!("phase-step must be greater than 0 and no more than 1");
    }
    if args.dimension_step <= 0.0
        || args.dimension_step > 1.0
        || args.dimension_radius < 0.0
        || args.dimension_radius > 1.0
    {
        bail!("dimension-step must be in (0, 1] and dimension-radius in [0, 1]");
    }
    if args.output_block == Some(0) {
        bail!("output-block must be at least 1");
    }
    if args.max_colors == Some(0)
        || args.max_colors.is_some_and(|colors| colors > 256)
        || args.palette_candidate_max < 2
        || args.palette_penalty < 0.0
        || args.palette_edge_emphasis < 0.0
    {
        bail!(
            "palette settings require max-colors in 1..=256, candidate-max >= 2, and nonnegative penalty/edge emphasis"
        );
    }
    if args.local_warp
        && (args.warp_patch < 8
            || args.warp_radius <= 0.0
            || args.warp_step <= 0.0
            || args.warp_smoothness < 0.0)
    {
        bail!("warp settings require patch >= 8, radius/step > 0, and smoothness >= 0");
    }
    if (args.cell_warp || automatic)
        && (args.cell_warp_radius <= 0.0
            || args.cell_warp_step <= 0.0
            || args.cell_warp_movement < 0.0
            || !(0.0..1.0).contains(&args.cell_warp_min_improvement)
            || args.cell_warp_min_variance < 0.0
            || args.cell_warp_contrast <= 0.0
            || args.cell_warp_min_contrast_gain < 0.0)
    {
        bail!(
            "cell-warp settings require radius/step/contrast > 0, nonnegative movement/variance, and improvement in [0, 1)"
        );
    }
    if args.ramp_penalty < 0.0
        || args.ramp_contrast <= 0.0
        || args.ramp_line_tolerance <= 0.0
        || args.ramp_continuation <= 0.0
    {
        bail!("ramp penalty must be nonnegative and ramp contrast/tolerance must be positive");
    }

    let source = ImageReader::open(&args.input)
        .with_context(|| format!("could not open {}", args.input.display()))?
        .decode()
        .with_context(|| format!("could not decode {}", args.input.display()))?
        .to_rgb8();
    let (image, perspective) = if let Some(corners) = args.corners {
        let estimated = estimated_size(corners);
        let width = args.rectified_width.unwrap_or(estimated.0);
        let height = args.rectified_height.unwrap_or(estimated.1);
        let image = rectify(&source, corners, width, height)?;
        (
            image,
            Some(PerspectiveReport {
                corners,
                rectified_width: width,
                rectified_height: height,
            }),
        )
    } else {
        (source.clone(), None)
    };
    let integral = IntegralImage::new(&image);
    let edge_profiles = automatic.then(|| EdgeProfiles::new(&image));
    let forced_width = args.block_width.or(args.block);
    let forced_height = args.block_height.or(args.block);
    if forced_width == Some(0) || forced_height == Some(0) {
        bail!("forced cell dimensions must be at least 1");
    }
    let effective_inset = if automatic { 0.18 } else { args.inset };
    let options = SearchOptions {
        min_width: forced_width.unwrap_or(args.min_block),
        max_width: forced_width.unwrap_or(if automatic {
            args.max_block.max(32)
        } else {
            args.max_block
        }),
        min_height: forced_height.unwrap_or(args.min_block),
        max_height: forced_height.unwrap_or(if automatic {
            args.max_block.max(32)
        } else {
            args.max_block
        }),
        inset_ratio: effective_inset,
        phase_step: args.phase_step,
        dimension_step: args.dimension_step,
        dimension_radius: args.dimension_radius,
        complexity: args.complexity,
        auto_select: automatic,
        square_coarse: automatic,
    };
    let candidates = search(&integral, options, edge_profiles.as_ref());
    let selected = *candidates
        .first()
        .context("grid search produced no candidates")?;
    let recovered_scale = (selected.cell_width * selected.cell_height).sqrt();
    let effective_warp_patch = if automatic {
        (recovered_scale * 4.0).round().max(8.0) as u32
    } else {
        args.warp_patch
    };
    let effective_warp_radius = if automatic {
        (recovered_scale * 0.24).clamp(1.0, 6.0)
    } else {
        args.warp_radius
    };
    let effective_warp_step = if automatic {
        (recovered_scale / 36.0).clamp(0.25, 1.0)
    } else {
        args.warp_step
    };
    let effective_warp_smoothness = if automatic {
        1.25
    } else {
        args.warp_smoothness
    };
    let warp = (args.local_warp || automatic).then(|| {
        fit_local_warp(
            &integral,
            selected,
            effective_inset,
            WarpOptions {
                patch_size: effective_warp_patch,
                radius: effective_warp_radius,
                step: effective_warp_step,
                smoothness: effective_warp_smoothness,
            },
        )
    });
    let baseline_cells = if let Some(warp) = &warp {
        extract_warped_cells(&image, selected, effective_inset, warp)
    } else {
        extract_cells(&integral, selected, effective_inset)
    };
    // Indexed PNG permits at most 256 entries. Auto mode uses the tighter
    // flexible ceiling; manual mode still remains safely indexable.
    let effective_max_color_count = args.max_colors.unwrap_or(if automatic { 32 } else { 256 });
    let effective_max_colors = Some(effective_max_color_count);
    let use_smart_palette = automatic || args.smart_palette;
    let (palette, mut assignments, palette_selection) = if use_smart_palette {
        let selection = select_smart_palette(
            &baseline_cells,
            SmartPaletteOptions {
                candidate_max: args.palette_candidate_max,
                max_colors: effective_max_color_count,
                complexity_penalty: args.palette_penalty,
                edge_emphasis: args.palette_edge_emphasis,
                merge_threshold: args.merge_threshold,
            },
        );
        (
            selection.palette,
            selection.assignments,
            Some(selection.report),
        )
    } else {
        let (palette, assignments) =
            cluster(&baseline_cells, args.merge_threshold, effective_max_colors);
        (palette, assignments, None)
    };
    let effective_cell_warp_radius = if automatic {
        (recovered_scale * 0.18).clamp(0.75, 2.5)
    } else {
        args.cell_warp_radius
    };
    let effective_cell_warp_step = if automatic {
        (recovered_scale / 32.0).clamp(0.2, 0.5)
    } else {
        args.cell_warp_step
    };
    let cell_warp = (args.cell_warp || automatic).then(|| {
        refine_cell_samples(
            &baseline_cells,
            &integral,
            selected,
            effective_inset,
            warp.as_ref(),
            CellWarpOptions {
                radius: effective_cell_warp_radius,
                step: effective_cell_warp_step,
                movement_penalty: args.cell_warp_movement,
                min_improvement: args.cell_warp_min_improvement,
                min_variance: args.cell_warp_min_variance,
                contrast_threshold: args.cell_warp_contrast,
                min_contrast_gain: args.cell_warp_min_contrast_gain,
            },
        )
    });
    let cells = cell_warp
        .as_ref()
        .map(|result| result.cells.clone())
        .unwrap_or(baseline_cells);
    if let Some(refinement) = &cell_warp {
        for (index, cell) in cells.iter().enumerate() {
            if refinement.offsets.contains_key(&(cell.cell_x, cell.cell_y)) {
                assignments[index] = nearest_palette_index(cell.rgb, &palette);
            }
        }
    }
    let ramp_cleanup = penalize_one_cell_ramps(
        &cells,
        &palette,
        &mut assignments,
        RampOptions {
            penalty: args.ramp_penalty,
            contrast_threshold: args.ramp_contrast,
            line_tolerance: args.ramp_line_tolerance,
            continuation_threshold: args.ramp_continuation,
            max_passes: args.ramp_passes,
        },
    );
    let cell_palette: HashMap<(i32, i32), usize> = cells
        .iter()
        .zip(assignments)
        .map(|(cell, assignment)| ((cell.cell_x, cell.cell_y), assignment))
        .collect();

    let block_width = selected.cell_width;
    let block_height = selected.cell_height;
    let output_block = args
        .output_block
        .unwrap_or_else(|| (block_width * block_height).sqrt().round().max(1.0) as u32);
    let output_cells_x = cell_range(selected.phase_x, block_width, image.width() as f64);
    let output_cells_y = cell_range(selected.phase_y, block_height, image.height() as f64);
    let output_width = (output_cells_x.end - output_cells_x.start) as u32 * output_block;
    let output_height = (output_cells_y.end - output_cells_y.start) as u32 * output_block;
    let output_metrics = measure_output(
        &cell_palette,
        &palette,
        output_cells_x.clone(),
        output_cells_y.clone(),
    );
    let output = args
        .output
        .unwrap_or_else(|| suffixed_path(&args.input, ".corrected.png"));
    if output
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("png"))
    {
        bail!("corrected output must use a .png extension");
    }
    let palette_rgb8: Vec<[u8; 3]> = palette.iter().copied().map(rgb8).collect();
    let indexed_pixels: Vec<u8> = (0..output_height)
        .flat_map(|y| {
            let cell_palette = &cell_palette;
            let output_cells_x = output_cells_x.clone();
            let output_cells_y = output_cells_y.clone();
            (0..output_width).map(move |x| {
                // Source squeeze and local deformation affect identification only.
                // Every recovered logical cell is repainted as an exact output square.
                let cell_x = output_cells_x.start + (x / output_block) as i32;
                let cell_y = output_cells_y.start + (y / output_block) as i32;
                cell_palette.get(&(cell_x, cell_y)).copied().unwrap_or(0) as u8
            })
        })
        .collect();
    let output_bit_depth = indexed_png::save(
        &output,
        output_width,
        output_height,
        &indexed_pixels,
        &palette_rgb8,
    )?;

    let mut overlay = image.clone();
    for y in 0..overlay.height() {
        for x in 0..overlay.width() {
            let center_x = x as f64 + 0.5;
            let center_y = y as f64 + 0.5;
            let displacement = warp
                .as_ref()
                .map(|field| field.displacement(center_x, center_y))
                .unwrap_or([0.0, 0.0]);
            let dx = (center_x - selected.phase_x - displacement[0]).rem_euclid(block_width);
            let dy = (center_y - selected.phase_y - displacement[1]).rem_euclid(block_height);
            if dx.min(block_width - dx) < 0.65 || dy.min(block_height - dy) < 0.65 {
                let source = overlay.get_pixel(x, y).0;
                overlay.put_pixel(
                    x,
                    y,
                    Rgb([
                        (source[0] as u16 / 2 + 127) as u8,
                        (source[1] as u16 / 2) as u8,
                        (source[2] as u16 / 2) as u8,
                    ]),
                );
            }
        }
    }
    if let Some(refinement) = &cell_warp {
        for (&(cell_x, cell_y), &residual) in &refinement.offsets {
            let nominal_x = selected.phase_x + (cell_x as f64 + 0.5) * block_width;
            let nominal_y = selected.phase_y + (cell_y as f64 + 0.5) * block_height;
            let smooth = warp
                .as_ref()
                .map(|field| field.displacement(nominal_x, nominal_y))
                .unwrap_or([0.0; 2]);
            let sample_x = (nominal_x + smooth[0] + residual[0]).round() as i32;
            let sample_y = (nominal_y + smooth[1] + residual[1]).round() as i32;
            for delta in -2..=2 {
                for (x, y) in [(sample_x + delta, sample_y), (sample_x, sample_y + delta)] {
                    if x >= 0 && y >= 0 && x < overlay.width() as i32 && y < overlay.height() as i32
                    {
                        overlay.put_pixel(x as u32, y as u32, Rgb([0, 255, 255]));
                    }
                }
            }
        }
    }
    let overlay_path = suffixed_path(&output, ".grid.png");
    overlay
        .save(&overlay_path)
        .with_context(|| format!("could not save {}", overlay_path.display()))?;

    let report_path = suffixed_path(&output, ".report.json");
    let report = Report {
        source: args.input.display().to_string(),
        output: output.display().to_string(),
        overlay: overlay_path.display().to_string(),
        width: image.width(),
        height: image.height(),
        output_width,
        output_height,
        output_block,
        output_color_type: "indexed",
        output_bit_depth,
        source_width: source.width(),
        source_height: source.height(),
        perspective,
        local_warp: warp.as_ref().map(|field| WarpReport {
            patch_size: effective_warp_patch,
            search_radius: effective_warp_radius,
            search_step: effective_warp_step,
            smoothness: effective_warp_smoothness,
            max_displacement: field.max_displacement(),
            rms_displacement: field.rms_displacement(),
        }),
        cell_warp: cell_warp.as_ref().map(|result| result.report),
        ramp_cleanup,
        selected,
        palette: palette_rgb8,
        palette_selection,
        output_metrics,
        candidates,
        settings: ReportSettings {
            auto: automatic,
            inset_ratio: effective_inset,
            phase_step: args.phase_step,
            dimension_step: args.dimension_step,
            dimension_radius: options.dimension_radius,
            merge_threshold: args.merge_threshold,
            max_colors: effective_max_colors,
            smart_palette: use_smart_palette,
            palette_candidate_max: args.palette_candidate_max,
            palette_penalty: args.palette_penalty,
            palette_edge_emphasis: args.palette_edge_emphasis,
            complexity: args.complexity,
        },
    };
    serde_json::to_writer_pretty(
        File::create(&report_path)
            .with_context(|| format!("could not create {}", report_path.display()))?,
        &report,
    )?;

    println!(
        "source grid: {:.3} × {:.3} px, phase ({:.2}, {:.2})",
        selected.cell_width, selected.cell_height, selected.phase_x, selected.phase_y
    );
    println!(
        "square output grid: {} px, dimensions {} × {}",
        output_block, output_width, output_height
    );
    println!(
        "fit score: {:.6}, palette: {} colors",
        selected.score,
        palette.len()
    );
    if let Some(selection) = &report.palette_selection {
        println!(
            "palette selection: {} ({} histogram peaks, fixed candidates 2..={})",
            selection.mode, selection.histogram_peaks, selection.fixed_candidate_limit
        );
    }
    println!(
        "output contrast: mean {:.6}, RMS {:.6}, strong edges {:.2}%, weak transitions {:.2}%, crispness {:.6}",
        output_metrics.mean_neighbor_distance,
        output_metrics.rms_neighbor_distance,
        output_metrics.strong_edge_fraction * 100.0,
        output_metrics.weak_transition_fraction_of_changed * 100.0,
        output_metrics.crispness_score,
    );
    println!(
        "one-cell ramps: {} corrections in {} passes ({} horizontal, {} vertical)",
        ramp_cleanup.corrected_cells,
        ramp_cleanup.passes_run,
        ramp_cleanup.horizontal_corrections,
        ramp_cleanup.vertical_corrections,
    );
    if automatic {
        println!(
            "auto score: {:.6}, edge alignment: {:.3}×",
            selected.auto_score, selected.edge_alignment
        );
    }
    if let Some(refinement) = &cell_warp {
        println!(
            "per-cell sampling: {} of {} eligible cells shifted, RMS {:.3} px, max {:.3} px",
            refinement.report.shifted_cells,
            refinement.report.eligible_cells,
            refinement.report.rms_displacement,
            refinement.report.max_displacement,
        );
    }
    println!("corrected: {}", output.display());
    println!("grid overlay: {}", overlay_path.display());
    println!("report: {}", report_path.display());
    Ok(())
}
