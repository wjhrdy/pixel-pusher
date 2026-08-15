use anyhow::{Context, Result, bail};
use clap::Parser;
use image::{ImageReader, Rgb};
use pixel_pusher::{
    color::rgb8,
    geometry::{Quad, estimated_size, rectify},
    grid::{Candidate, EdgeProfiles, SearchOptions, search},
    indexed_png,
    integral::IntegralImage,
    lattice::{FittedLattice, LatticeFitReport, LatticeOptions, extract_cells, fit_lattice},
    metrics::{OutputMetrics, measure_output},
    palette::{
        CellColor, PaletteSelectionReport, SmartPaletteOptions, cluster, nearest_palette_index,
        select_smart_palette,
    },
    ramp::{RampOptions, RampReport, penalize_one_cell_ramps},
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
    #[arg(required_unless_present = "gui")]
    input: Option<PathBuf>,

    /// Open the native drag-and-drop desktop application.
    #[arg(long, visible_alias = "ui")]
    gui: bool,

    /// Corrected output image. Defaults to <input>.corrected.png.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Automatically select scale, squeeze, phase, local lattice, palette, and output scale.
    #[arg(long, conflicts_with_all = ["block", "block_width", "block_height"])]
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

    /// Override the adaptive complexity cost for each palette color beyond two.
    #[arg(long)]
    palette_penalty: Option<f64>,

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

    /// Fit a locally deformable 2D junction mesh after regular grid detection.
    #[arg(long)]
    lattice_fit: bool,

    /// Maximum movement of a fitted lattice junction from the regular-grid seed.
    #[arg(long, default_value_t = 2.0)]
    lattice_radius: f64,

    /// Subpixel increment used while fitting lattice junctions.
    #[arg(long, default_value_t = 0.25)]
    lattice_step: f64,

    /// Penalty for neighboring junctions receiving different displacements.
    #[arg(long, default_value_t = 0.01)]
    lattice_regularization: f64,

    /// Weight given to distinct local color boundaries during lattice fitting.
    #[arg(long, default_value_t = 0.08)]
    lattice_edge_weight: f64,

    /// Minimum boundary contrast for corners and anchor-guided line fitting.
    #[arg(long, default_value_t = 0.04)]
    lattice_min_edge: f64,

    /// Alternating horizontal/vertical lattice fitting passes.
    #[arg(long, default_value_t = 4)]
    lattice_iterations: u32,

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
    lattice_fit: Option<LatticeFitReport>,
    color_picker_overrides: usize,
    ramp_cleanup: RampReport,
    selected: Candidate,
    palette: Vec<[u8; 3]>,
    palette_selection: Option<PaletteSelectionReport>,
    output_metrics: OutputMetrics,
    candidates: Vec<Candidate>,
    settings: ReportSettings,
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
    min_cell_width: u32,
    max_cell_width: u32,
    min_cell_height: u32,
    max_cell_height: u32,
    inset_ratio: f64,
    phase_step: f64,
    dimension_step: f64,
    dimension_radius: f64,
    merge_threshold: f64,
    max_colors: Option<usize>,
    smart_palette: bool,
    palette_candidate_max: usize,
    palette_penalty: Option<f64>,
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

fn lattice_fit_enabled(automatic: bool, explicitly_enabled: bool) -> bool {
    automatic || explicitly_enabled
}

fn draw_line(image: &mut image::RgbImage, start: [f64; 2], end: [f64; 2], color: Rgb<u8>) {
    let mut x0 = start[0].round() as i32;
    let mut y0 = start[1].round() as i32;
    let x1 = end[0].round() as i32;
    let y1 = end[1].round() as i32;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        if x0 >= 0 && y0 >= 0 && x0 < image.width() as i32 && y0 < image.height() as i32 {
            image.put_pixel(x0 as u32, y0 as u32, color);
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let doubled = 2 * error;
        if doubled >= dy {
            error += dy;
            x0 += sx;
        }
        if doubled <= dx {
            error += dx;
            y0 += sy;
        }
    }
}

fn draw_node(image: &mut image::RgbImage, position: [f64; 2]) {
    let center_x = position[0].round() as i32;
    let center_y = position[1].round() as i32;
    for delta in -2..=2 {
        for (x, y) in [
            (center_x + delta, center_y - 2),
            (center_x + delta, center_y + 2),
            (center_x - 2, center_y + delta),
            (center_x + 2, center_y + delta),
        ] {
            if x >= 0 && y >= 0 && x < image.width() as i32 && y < image.height() as i32 {
                image.put_pixel(x as u32, y as u32, Rgb([0, 255, 96]));
            }
        }
    }
}

fn draw_color_override(image: &mut image::RgbImage, position: [f64; 2]) {
    let center_x = position[0].round() as i32;
    let center_y = position[1].round() as i32;
    for delta in -3..=3 {
        for (x, y) in [(center_x + delta, center_y), (center_x, center_y + delta)] {
            if x >= 0 && y >= 0 && x < image.width() as i32 && y < image.height() as i32 {
                image.put_pixel(x as u32, y as u32, Rgb([0, 230, 255]));
            }
        }
    }
}

fn find_color_picker_overrides(
    cells: &[CellColor],
    assignments: &[usize],
    regular_colors: &HashMap<(i32, i32), [f64; 3]>,
    palette: &[[f64; 3]],
) -> Vec<(i32, i32)> {
    cells
        .iter()
        .zip(assignments)
        .filter_map(|(cell, &assignment)| {
            let regular = regular_colors.get(&(cell.cell_x, cell.cell_y))?;
            (nearest_palette_index(*regular, palette) != assignment)
                .then_some((cell.cell_x, cell.cell_y))
        })
        .collect()
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.gui {
        return pixel_pusher::gui::run_with_image(args.input.clone())
            .map_err(|error| anyhow::anyhow!(error.to_string()));
    }
    let input = args
        .input
        .as_ref()
        .context("an input image is required unless --gui is used")?;
    let automatic = args.auto;
    let use_lattice_fit = lattice_fit_enabled(automatic, args.lattice_fit);
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
        || args.palette_penalty.is_some_and(|penalty| penalty < 0.0)
        || args.palette_edge_emphasis < 0.0
    {
        bail!(
            "palette settings require max-colors in 1..=256, candidate-max >= 2, and nonnegative penalty/edge emphasis"
        );
    }
    if use_lattice_fit
        && (args.lattice_radius <= 0.0
            || args.lattice_step <= 0.0
            || args.lattice_regularization < 0.0
            || args.lattice_edge_weight < 0.0
            || !(0.0..=1.0).contains(&args.lattice_min_edge)
            || args.lattice_iterations == 0)
    {
        bail!(
            "lattice settings require radius/step > 0, nonnegative regularization/edge weight, min-edge in [0, 1], and at least one iteration"
        );
    }
    if args.ramp_penalty < 0.0
        || args.ramp_contrast <= 0.0
        || args.ramp_line_tolerance <= 0.0
        || args.ramp_continuation <= 0.0
    {
        bail!("ramp penalty must be nonnegative and ramp contrast/tolerance must be positive");
    }

    let source = ImageReader::open(input)
        .with_context(|| format!("could not open {}", input.display()))?
        .decode()
        .with_context(|| format!("could not decode {}", input.display()))?
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
        max_width: forced_width.unwrap_or(args.max_block),
        min_height: forced_height.unwrap_or(args.min_block),
        max_height: forced_height.unwrap_or(args.max_block),
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
    let effective_lattice_radius = if automatic {
        (recovered_scale * 0.6).clamp(1.0, 6.0)
    } else {
        args.lattice_radius
    };
    let effective_lattice_step = if automatic {
        (recovered_scale / 24.0).clamp(0.1, 0.5)
    } else {
        args.lattice_step
    };
    let lattice_options = LatticeOptions {
        radius: effective_lattice_radius,
        step: effective_lattice_step,
        regularization: args.lattice_regularization,
        edge_weight: args.lattice_edge_weight,
        min_edge_strength: args.lattice_min_edge,
        iterations: args.lattice_iterations,
    };
    let lattice = if use_lattice_fit {
        fit_lattice(
            &image,
            &integral,
            selected,
            effective_inset,
            lattice_options,
        )
    } else {
        FittedLattice::regular(selected, integral.width(), integral.height())
    };
    let lattice_report = use_lattice_fit.then(|| lattice.report(lattice_options));
    let cells = extract_cells(&image, &integral, &lattice, effective_inset);
    // Indexed PNG permits at most 256 entries. Keep the default compact in
    // both modes while allowing an explicit larger ceiling when requested.
    let effective_max_color_count = args.max_colors.unwrap_or(24);
    let effective_max_colors = Some(effective_max_color_count);
    let use_smart_palette = automatic || args.smart_palette;
    let (palette, mut assignments, palette_selection) = if use_smart_palette {
        let selection = select_smart_palette(
            &cells,
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
        let (palette, assignments) = cluster(&cells, args.merge_threshold, effective_max_colors);
        (palette, assignments, None)
    };
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
    let regular_lattice = FittedLattice::regular(selected, integral.width(), integral.height());
    let regular_colors: HashMap<(i32, i32), [f64; 3]> =
        extract_cells(&image, &integral, &regular_lattice, effective_inset)
            .into_iter()
            .map(|cell| ((cell.cell_x, cell.cell_y), cell.rgb))
            .collect();
    let color_picker_overrides =
        find_color_picker_overrides(&cells, &assignments, &regular_colors, &palette);
    let cell_palette: HashMap<(i32, i32), usize> = cells
        .iter()
        .zip(&assignments)
        .map(|(cell, &assignment)| ((cell.cell_x, cell.cell_y), assignment))
        .collect();

    let block_width = selected.cell_width;
    let block_height = selected.cell_height;
    let output_block = args
        .output_block
        .unwrap_or_else(|| (block_width * block_height).sqrt().round().max(1.0) as u32);
    let output_cells_x = lattice.cell_x_range();
    let output_cells_y = lattice.cell_y_range();
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
        .unwrap_or_else(|| suffixed_path(input, ".corrected.png"));
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
                // Source squeeze and lattice fitting affect identification only.
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
    for (start, end) in lattice.mesh_segments() {
        draw_line(&mut overlay, start, end, Rgb([225, 48, 48]));
    }
    for (start, end) in lattice.supported_segments() {
        draw_line(&mut overlay, start, end, Rgb([24, 24, 24]));
    }
    for node in lattice.supported_nodes() {
        draw_node(&mut overlay, node);
    }
    for &(cell_x, cell_y) in &color_picker_overrides {
        if let Some(center) = lattice.cell_center(cell_x, cell_y) {
            draw_color_override(&mut overlay, center);
        }
    }
    let overlay_path = suffixed_path(&output, ".grid.png");
    overlay
        .save(&overlay_path)
        .with_context(|| format!("could not save {}", overlay_path.display()))?;

    let report_path = suffixed_path(&output, ".report.json");
    let report = Report {
        source: input.display().to_string(),
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
        lattice_fit: lattice_report,
        color_picker_overrides: color_picker_overrides.len(),
        ramp_cleanup,
        selected,
        palette: palette_rgb8,
        palette_selection,
        output_metrics,
        candidates,
        settings: ReportSettings {
            auto: automatic,
            min_cell_width: options.min_width,
            max_cell_width: options.max_width,
            min_cell_height: options.min_height,
            max_cell_height: options.max_height,
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
    if let Some(fit) = &report.lattice_fit {
        println!(
            "lattice init: {} vertical lines, {} horizontal lines, {} corner seeds",
            fit.initial_vertical_lines, fit.initial_horizontal_lines, fit.initialized_corner_nodes,
        );
        println!(
            "lattice fit: {} corner anchors, {} junctions moved, RMS {:.3} px, max {:.3} px",
            fit.supported_nodes, fit.moved_nodes, fit.rms_displacement, fit.max_displacement,
        );
    }
    println!(
        "color picker overrides: {} cells",
        color_picker_overrides.len()
    );
    println!("corrected: {}", output.display());
    println!("grid overlay: {}", overlay_path.display());
    println!("report: {}", report_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{find_color_picker_overrides, lattice_fit_enabled};
    use pixel_pusher::palette::CellColor;
    use std::collections::HashMap;

    #[test]
    fn auto_mode_always_enables_lattice_fitting() {
        assert!(lattice_fit_enabled(true, false));
        assert!(lattice_fit_enabled(true, true));
        assert!(lattice_fit_enabled(false, true));
        assert!(!lattice_fit_enabled(false, false));
    }

    #[test]
    fn color_picker_overrides_compare_final_and_regular_palette_choices() {
        let cells = vec![
            CellColor {
                cell_x: 2,
                cell_y: 3,
                rgb: [0.95, 0.95, 0.95],
                weight: 1.0,
            },
            CellColor {
                cell_x: 3,
                cell_y: 3,
                rgb: [0.05, 0.05, 0.05],
                weight: 1.0,
            },
        ];
        let regular_colors =
            HashMap::from([((2, 3), [0.05, 0.05, 0.05]), ((3, 3), [0.05, 0.05, 0.05])]);
        let palette = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];

        let overrides = find_color_picker_overrides(&cells, &[1, 0], &regular_colors, &palette);

        assert_eq!(overrides, vec![(2, 3)]);
    }
}
