# Pixel Pusher

Pixel Pusher is an experimental Rust CLI for recovering clean, grid-aligned pixel art from AI-generated approximations, rescaled artwork, and imperfect captures.

It searches logical-pixel widths, heights, and grid phases independently, including fractional source-pixel dimensions and offsets. Cells can therefore fit a source that has been slightly squeezed or stretched along one axis—for example, a `4.12 × 3.84` source grid. Each candidate is scored by the color variance *inside* its cells. A configurable inset excludes anti-aliased or misaligned borders from both scoring and final color estimation. Auto mode generates several edge-aware palette candidates in Oklab, then balances reconstruction error against palette complexity.

The output is always rendered onto a new, rigid lattice of exact square pixels. Fractional squeeze, perspective correction, and local warp affect source sampling only; they never produce subpixels or warped boundaries in the result.

## Features

- Automatic logical-pixel scale, x/y phase, fractional squeeze, palette, smooth and per-cell sampling warps, and output-scale selection
- Independent source cell width and height with square output reconstruction
- Fractional-area RGB/RGB² sampling through summed-area tables
- Edge-aware, histogram-peak-seeded Oklab palette candidates with automatic color-count selection
- Optional hard palette limit and flexible threshold-clustering fallback
- Compact indexed-color PNG output using 1, 2, 4, or 8 bits per pixel
- One-cell color-ramp suppression with source-fidelity protection
- Optional four-corner perspective rectification for photographed art
- Perceptual output metrics for edge strength, weak transitions, and crispness
- PNG, JPEG, and WebP input

## Install

Pixel Pusher requires a current stable Rust toolchain.

### Homebrew

```sh
brew tap wjhrdy/pixel-pusher https://github.com/wjhrdy/pixel-pusher
brew install pixel-pusher
```

### Prebuilt binaries

Release archives for Linux, macOS, and Windows are available from the [GitHub releases page](https://github.com/wjhrdy/pixel-pusher/releases). Each archive is accompanied by a SHA-256 checksum.

### Build from source

```sh
git clone https://github.com/wjhrdy/pixel-pusher.git
cd pixel-pusher
cargo build --release
```

The executable will be at `target/release/pixel-pusher`. You can also install it into Cargo's binary directory:

```sh
cargo install --path .
```

## Quick start

```sh
pixel-pusher input.png --auto
```

Auto mode selects the fundamental logical-pixel scale using both cell-interior consistency and periodic concentration of source gradients on candidate grid boundaries. It then refines fractional width, height, and phase; derives a scale-relative local sampling warp; compacts the palette; and renders an unsqueezed square-cell output. Only the input path and `--auto` are required.

The command writes three files next to the input:

- `input.corrected.png` — reconstructed pixel art as a lossless indexed-color PNG
- `input.corrected.grid.png` — detected grid over the source
- `input.corrected.report.json` — selected grid, palette, output-contrast metrics, settings, and ranked block-size candidates

Useful controls:

```sh
# Search a wider range and force no more than 24 colors
pixel-pusher input.png --auto --min-block 3 --max-block 32 --max-colors 24

# Use smart palette selection with a manually constrained grid
pixel-pusher input.png --block 8 --smart-palette

# Ignore more of each logical pixel's contaminated border
pixel-pusher input.png --inset 0.22

# Inspect a particular square block size while still optimizing its sub-pixel phase
pixel-pusher input.png --block 8

# Force rectangular logical pixels
pixel-pusher input.png --block-width 7 --block-height 9

# Paint each recovered logical cell as an exact 6 × 6 output square
pixel-pusher input.png --output-block 6
```

Run `pixel-pusher --help` for every option.

A typical automatic run reports the selected source geometry and final-image measurements:

```text
source grid: 3.200 × 3.800 px, phase (2.55, 1.30)
square output grid: 3 px, dimensions 1569 × 747
fit score: 0.079981, palette: 8 colors
palette selection: fixed (14 histogram peaks, fixed candidates 2..=12)
output contrast: mean 0.043356, RMS 0.097066, strong edges 14.01%, weak transitions 49.91%, crispness 0.031726
```

## How the search works

1. Build summed-area tables for RGB and RGB².
2. Coarsely test every integer phase for every requested width × height pair.
3. Refine the strongest candidates' phase and source width/height fractionally.
4. Select the grid with the lowest normalized within-cell variance, with a small preference for larger cells to resolve exact divisor ties.
5. Estimate one color per logical cell from only its inset interior.
6. Build edge-weighted, histogram-peak-seeded Oklab k-means candidates for palette sizes 2 through 12.
7. Score their reconstruction error and complexity, then choose a fixed candidate or a flexible threshold-clustered palette.
8. Penalize one-cell intermediate-color ramps whose opposite neighbors are high contrast, while retaining a sampled-color fidelity cost.
9. Paint every recovered cell onto a separate, perfectly square output lattice.

The report measures the final logical-pixel lattice in perceptual Oklab space. It includes mean and RMS neighbor distance, strong-edge density, the share of changed boundaries that remain weak, and a soft-thresholded crispness score. These measurements do not depend on output enlargement. Read crispness together with edge density: maximizing contrast alone can reward unwanted random noise.

### Smart palette selection

Smart selection is enabled by `--auto`, or independently with `--smart-palette`. It builds a 16-bin-per-RGB-channel histogram of recovered cell colors, gives extra weight to cells on strong logical-pixel edges, finds radius-one local peaks, and uses the strongest peaks to seed deterministic weighted k-means candidates. Candidate sizes default to 2 through 12. Each candidate records weighted Oklab SSE, `log10(SSE + 1)`, a per-color complexity penalty, normalized fit, and its smallest cluster fraction in the JSON report.

The lowest rate-distortion cost wins. If the fit curve is still improving at the end of a peak-rich candidate bank, a flexible threshold-clustered palette may be used up to `--max-colors` (32 in auto mode). This is an explicit, auditable substitute for a proprietary learned classifier; it deliberately does not reproduce an unknown decision tree. Tune the fixed bank with `--palette-candidate-max`, the complexity tradeoff with `--palette-penalty` (default `0.08`; larger values produce smaller palettes), and outline/highlight preservation with `--palette-edge-emphasis`. `--max-colors` always remains a hard ceiling.

Corrected images are written as indexed-color PNGs. Palettes of 1–2, 3–4, 5–16, and 17–256 colors use 1, 2, 4, and 8 bits per pixel respectively. The JSON report records `output_color_type` and `output_bit_depth`; grid overlays remain ordinary RGB PNGs. Indexed PNG cannot represent more than 256 colors, so `--max-colors` is limited accordingly.

One-cell ramp cleanup is enabled by default. A middle cell is eligible only for a five-cell `A–A–B–C–C` pattern: `B` lies near the Oklab color segment between high-contrast `A` and `C`, and both sides remain locally stable. The optimizer snaps it to the better-supported endpoint only when the ramp penalty outweighs the increase in source-color error. Tune it with `--ramp-penalty`, `--ramp-contrast`, `--ramp-line-tolerance`, and `--ramp-continuation`; use `--ramp-penalty 0` to disable it.

Fractional rectangle statistics are computed analytically from pixel-area coverage; the source is not repeatedly resized for candidate grids.

## Photographed or perspective-skewed art

Supply the four outside corners in top-left, top-right, bottom-right, bottom-left order. Coordinates are measured from the source image's top-left edge:

```sh
pixel-pusher photo.jpg \
  --corners "112,84; 913,121; 861,742; 146,711" \
  --max-colors 24
```

Pixel Pusher estimates a rectified width and height from the four sides, maps the quadrilateral into an axis-aligned working image with a projective transform, and then runs the normal grid optimizer. Override that estimate with `--rectified-width` and `--rectified-height` when the desired dimensions are known.

This follows the geometry/sampling separation used in QR readers, but does not assume that pixel art contains QR-style finder patterns. Automatic quadrilateral and periodic-grid detection is not yet included; the four corners are the current seed for perspective recovery.

## Locally irregular grids

For generated art whose logical-pixel alignment drifts across the image, enable the regularized local warp:

```sh
pixel-pusher input.png --block 4 --local-warp --inset 0.18
```

The image is covered by a coarse control mesh. Each control point searches a small neighborhood for the phase with the lowest mean inset cell variance. The displacement field is then smoothed across neighboring controls. This lets the *input sampling regions* bend gradually toward local separations without allowing the regional grid to tear.

The main controls are `--warp-patch` (control spacing), `--warp-radius` (maximum displacement), `--warp-step`, and `--warp-smoothness`.

Auto mode follows the smooth field with a finer per-cell residual search. Only internally mixed cells next to a high-contrast neighbor are eligible, and a shift is accepted only when it both materially reduces inset variance and increases neighbor contrast after a movement penalty. This can align a tooth, highlight, or other one-logical-pixel feature without moving an entire local patch. Enable it explicitly outside auto mode with `--cell-warp`; tune it with `--cell-warp-radius`, `--cell-warp-step`, `--cell-warp-movement`, `--cell-warp-min-improvement`, `--cell-warp-min-variance`, `--cell-warp-contrast`, and `--cell-warp-min-contrast-gain`. Cyan crosses in the grid overlay mark cells whose sampling centers shifted.

Neither warp is used for output geometry: estimated colors are always painted back onto the original rigid, axis-aligned logical grid, so curved or subpixel cell boundaries cannot appear in the corrected image.

## Current proof-of-concept limits

- It assumes an axis-aligned, regularly spaced grid; width and height are independent.
- It searches fractional source cell dimensions and x/y phase offsets; output cells remain integer squares.
- Perspective and rotation are handled from four supplied corners; automatic corner detection and locally warped grids are not yet modeled.
- Palette membership is global, although topology-derived edge weights protect rare outlines and highlights. Region-specific palettes are not yet modeled.

## Development

```sh
cargo test --release
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The test suite covers fractional sampling, grid and squeeze recovery, homography mapping, local-warp interpolation, edge-aware palette selection, one-cell ramp handling, and scale-independent output metrics.
