use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Pos2, Rect, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Vec2,
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
};

const ACCENT: Color32 = Color32::from_rgb(240, 93, 61);
const TEAL: Color32 = Color32::from_rgb(43, 111, 117);

pub fn run() -> eframe::Result {
    run_with_image(None)
}

pub fn run_with_image(initial_image: Option<PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Pixel Pusher")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([820.0, 600.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Pixel Pusher",
        options,
        Box::new(move |context| {
            Ok(Box::new(PixelPusherApp::new(
                context,
                initial_image.clone(),
            )))
        }),
    )
}

struct PixelPusherApp {
    source: Option<SourceImage>,
    result: Option<ProcessedResult>,
    pending: Option<Receiver<Result<ProcessedPayload, String>>>,
    dialog_pending: Option<Receiver<DialogResult>>,
    controls: Controls,
    status: String,
    error: bool,
    show_overlay: bool,
    dragging_corner: Option<usize>,
}

enum DialogResult {
    Open(Option<PathBuf>),
    Saved(Result<Option<PathBuf>, String>),
}

struct SourceImage {
    path: PathBuf,
    texture: TextureHandle,
    width: u32,
    height: u32,
    corners: [[f32; 2]; 4],
}

struct ProcessedResult {
    corrected_texture: TextureHandle,
    overlay_texture: TextureHandle,
    corrected: Vec<u8>,
    overlay: Vec<u8>,
    report: Vec<u8>,
    summary: Summary,
    stem: String,
}

struct ProcessedPayload {
    corrected: Vec<u8>,
    overlay: Vec<u8>,
    report: Vec<u8>,
    summary: Summary,
    stem: String,
}

#[derive(Default)]
struct Summary {
    source_grid: String,
    output_size: String,
    palette_colors: usize,
    color_picker_overrides: u64,
    bit_depth: u64,
    fit_score: f64,
    perspective: bool,
}

#[derive(Clone, PartialEq)]
struct Controls {
    automatic: bool,
    custom_modified: bool,
    min_block: u32,
    max_block: u32,
    max_colors: usize,
    block_width: u32,
    block_height: u32,
    inset: f64,
    phase_step: f64,
    dimension_step: f64,
    dimension_radius: f64,
    complexity: f64,
    output_block: u32,
    perspective: bool,
    rectified_width: u32,
    rectified_height: u32,
    lattice_fit: bool,
    lattice_radius: f64,
    lattice_step: f64,
    lattice_regularization: f64,
    lattice_edge_weight: f64,
    lattice_min_edge: f64,
    lattice_iterations: u32,
    smart_palette: bool,
    palette_candidate_max: usize,
    merge_threshold: f64,
    palette_penalty_enabled: bool,
    palette_penalty: f64,
    palette_edge_emphasis: f64,
    ramp_penalty: f64,
    ramp_contrast: f64,
    ramp_line_tolerance: f64,
    ramp_continuation: f64,
    ramp_passes: u32,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            automatic: true,
            custom_modified: false,
            min_block: 2,
            max_block: 16,
            max_colors: 0,
            block_width: 0,
            block_height: 0,
            inset: 0.14,
            phase_step: 0.25,
            dimension_step: 0.1,
            dimension_radius: 0.75,
            complexity: 0.002,
            output_block: 0,
            perspective: false,
            rectified_width: 0,
            rectified_height: 0,
            lattice_fit: false,
            lattice_radius: 2.0,
            lattice_step: 0.25,
            lattice_regularization: 0.01,
            lattice_edge_weight: 0.08,
            lattice_min_edge: 0.04,
            lattice_iterations: 4,
            smart_palette: true,
            palette_candidate_max: 12,
            merge_threshold: 0.035,
            palette_penalty_enabled: false,
            palette_penalty: 0.0,
            palette_edge_emphasis: 1.0,
            ramp_penalty: 0.3,
            ramp_contrast: 0.08,
            ramp_line_tolerance: 0.035,
            ramp_continuation: 0.04,
            ramp_passes: 1,
        }
    }
}

impl PixelPusherApp {
    fn new(context: &eframe::CreationContext<'_>, initial_image: Option<PathBuf>) -> Self {
        context.egui_ctx.set_theme(egui::Theme::Light);
        let mut style = (*context.egui_ctx.style_of(egui::Theme::Light)).clone();
        style.visuals = egui::Visuals::light();
        style.visuals.panel_fill = Color32::from_rgb(244, 241, 232);
        style.visuals.window_fill = Color32::from_rgb(255, 253, 247);
        style.visuals.widgets.active.bg_fill = TEAL;
        style.visuals.selection.bg_fill = TEAL;
        style.visuals.selection.stroke = Stroke::new(1.5, Color32::WHITE);
        style.spacing.item_spacing = Vec2::new(9.0, 8.0);
        context.egui_ctx.set_style_of(egui::Theme::Light, style);
        let mut app = Self {
            source: None,
            result: None,
            pending: None,
            dialog_pending: None,
            controls: Controls::default(),
            status: "Drop an image or choose a file to begin.".to_owned(),
            error: false,
            show_overlay: false,
            dragging_corner: None,
        };
        if let Some(path) = initial_image {
            app.open_image(&context.egui_ctx, path);
        }
        app
    }

    fn open_image(&mut self, context: &egui::Context, path: PathBuf) {
        match load_texture(context, &path, "source") {
            Ok((texture, width, height)) => {
                let x = width as f32 * 0.035;
                let y = height as f32 * 0.035;
                self.source = Some(SourceImage {
                    path,
                    texture,
                    width,
                    height,
                    corners: [
                        [x, y],
                        [width as f32 - x, y],
                        [width as f32 - x, height as f32 - y],
                        [x, height as f32 - y],
                    ],
                });
                self.result = None;
                self.show_overlay = false;
                self.set_status("Ready to find the grid.", false);
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn set_status(&mut self, message: impl Into<String>, error: bool) {
        self.status = message.into();
        self.error = error;
    }

    fn begin_processing(&mut self, context: &egui::Context) {
        let Some(source) = &self.source else {
            self.set_status("Choose an image first.", true);
            return;
        };
        if self.pending.is_some() {
            return;
        }
        let path = source.path.clone();
        let corners = source.corners;
        let controls = self.controls.clone();
        let repaint = context.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = process_image(&path, corners, &controls);
            let _ = sender.send(result);
            repaint.request_repaint();
        });
        self.pending = Some(receiver);
        self.set_status(
            "Searching grids, fitting local samples, and selecting colors…",
            false,
        );
    }

    fn poll_processing(&mut self, context: &egui::Context) {
        let received = self
            .pending
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        let Some(result) = received else { return };
        self.pending = None;
        match result {
            Ok(payload) => {
                let corrected_texture =
                    match texture_from_bytes(context, &payload.corrected, "corrected") {
                        Ok(texture) => texture,
                        Err(error) => {
                            self.set_status(error, true);
                            return;
                        }
                    };
                let overlay_texture = match texture_from_bytes(context, &payload.overlay, "overlay")
                {
                    Ok(texture) => texture,
                    Err(error) => {
                        self.set_status(error, true);
                        return;
                    }
                };
                self.result = Some(ProcessedResult {
                    corrected_texture,
                    overlay_texture,
                    corrected: payload.corrected,
                    overlay: payload.overlay,
                    report: payload.report,
                    summary: payload.summary,
                    stem: payload.stem,
                });
                self.show_overlay = true;
                self.set_status("Done. The output uses only rigid square pixels.", false);
            }
            Err(error) => self.set_status(error, true),
        }
    }

    fn handle_dropped_files(&mut self, context: &egui::Context) -> bool {
        let files = context.input(|input| input.raw.dropped_files.clone());
        if let Some(path) = files.into_iter().find_map(|file| file.path) {
            self.open_image(context, path);
            true
        } else {
            false
        }
    }

    fn begin_pick_image(&mut self, context: &egui::Context) {
        if self.dialog_pending.is_some() {
            return;
        }
        let future = rfd::AsyncFileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file();
        let (sender, receiver) = mpsc::channel();
        let repaint = context.clone();
        std::thread::spawn(move || {
            let path = pollster::block_on(future).map(|file| file.path().to_path_buf());
            let _ = sender.send(DialogResult::Open(path));
            repaint.request_repaint();
        });
        self.dialog_pending = Some(receiver);
    }

    fn begin_save_artifact(
        &mut self,
        context: &egui::Context,
        bytes: Vec<u8>,
        filename: String,
        label: &'static str,
        extensions: &'static [&'static str],
    ) {
        if self.dialog_pending.is_some() {
            return;
        }
        let future = rfd::AsyncFileDialog::new()
            .add_filter(label, extensions)
            .set_file_name(filename)
            .save_file();
        let (sender, receiver) = mpsc::channel();
        let repaint = context.clone();
        std::thread::spawn(move || {
            let result = pollster::block_on(future).map_or(Ok(None), |file| {
                let path = file.path().to_path_buf();
                std::fs::write(&path, bytes)
                    .map(|()| Some(path))
                    .map_err(|error| error.to_string())
            });
            let _ = sender.send(DialogResult::Saved(result));
            repaint.request_repaint();
        });
        self.dialog_pending = Some(receiver);
    }

    fn poll_dialog(&mut self, context: &egui::Context) {
        let received = self
            .dialog_pending
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        let Some(result) = received else { return };
        self.dialog_pending = None;
        match result {
            DialogResult::Open(Some(path)) => self.open_image(context, path),
            DialogResult::Open(None) | DialogResult::Saved(Ok(None)) => {}
            DialogResult::Saved(Ok(Some(path))) => {
                self.set_status(format!("Saved {}", path.display()), false);
            }
            DialogResult::Saved(Err(error)) => {
                self.set_status(format!("Could not save file: {error}"), true);
            }
        }
    }

    fn show_header(&self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(Color32::from_rgb(244, 241, 232))
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (mark, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                    for row in 0..3 {
                        for column in 0..3 {
                            let origin =
                                mark.min + Vec2::new(column as f32 * 11.0, row as f32 * 11.0);
                            ui.painter().rect_filled(
                                Rect::from_min_size(origin, Vec2::splat(8.0)),
                                1.5,
                                if matches!((row, column), (0, 1) | (1, 0) | (2, 1)) {
                                    ACCENT
                                } else {
                                    Color32::from_gray(28)
                                },
                            );
                        }
                    }
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Pixel Pusher").size(22.0).strong());
                        ui.label(
                            egui::RichText::new("Find the hidden grid. Keep the good pixels.")
                                .color(Color32::from_gray(100)),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("●  Native · processing stays on this computer")
                                .small()
                                .color(Color32::from_rgb(60, 130, 80)),
                        );
                    });
                });
            });
    }

    fn show_controls(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        egui::Frame::new()
            .fill(Color32::from_rgb(255, 253, 247))
            .inner_margin(16.0)
            .corner_radius(14.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("controls-scroll")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.heading("Alignment");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if !self.controls.automatic
                                        && ui.small_button("Reset defaults").clicked()
                                    {
                                        self.controls = Controls::default();
                                        self.controls.automatic = false;
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let auto = ui
                                .selectable_label(self.controls.automatic, "Auto")
                                .on_hover_text("Runs grid scale, fractional squeeze, phase, line-seeded and corner-anchored lattice fitting, palette, and output-scale selection automatically. Coherent boundaries initialize corner candidates first; nearby row and column points may then follow supported edges. This is the normal starting mode.");
                            if auto.clicked() {
                                self.controls.automatic = true;
                                self.controls.smart_palette = true;
                            }
                            let custom = ui
                                .selectable_label(!self.controls.automatic, "Custom")
                                .on_hover_text("Starts with the exact Auto pipeline. It switches to the explicit settings below only after you change a control, so selecting Custom by itself does not alter the result.");
                            if custom.clicked() {
                                self.controls.automatic = false;
                            }
                            info_badge(ui, "Processing mode. Auto is recommended for an unknown image; Custom is intended for controlled experiments and known grids.");
                        });

                        ui.separator();
                        if self.controls.automatic {
                            ui.label(
                                egui::RichText::new(
                                    "Auto uses the built-in grid, clustered-line lattice initialization, edge-gated local fitting, sampling, and palette search with a default ceiling of 24 colors.",
                                )
                                .small()
                                .color(Color32::from_gray(90)),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Choose Custom to configure the search or correct perspective with corner handles.",
                                )
                                .small()
                                .color(Color32::from_gray(110)),
                            );
                        } else {
                            let custom_before = self.controls.clone();
                            ui.label(
                                egui::RichText::new(if self.controls.custom_modified {
                                    "Explicit Custom settings are active. Reset defaults to return to the Auto baseline."
                                } else {
                                    "Auto baseline is active. Change any control below to create a Custom override."
                                })
                                .small()
                                .color(if self.controls.custom_modified {
                                    Color32::from_gray(90)
                                } else {
                                    TEAL
                                }),
                            );
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Palette limit").strong());
                                info_badge(ui, "Hard ceiling for indexed-PNG colors. The default allows up to 24 while selecting fewer when appropriate. Common fixed limits are 12–32; use 64–256 only for unusually color-rich art.");
                            });
                            egui::ComboBox::from_id_salt("palette-limit")
                                .selected_text(if self.controls.max_colors == 0 {
                                    "Default (up to 24)".to_owned()
                                } else {
                                    self.controls.max_colors.to_string()
                                })
                                .width(ui.available_width().max(1.0))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.controls.max_colors,
                                        0,
                                        "Default (up to 24)",
                                    );
                                    for colors in [12, 16, 24, 32, 64, 128, 256] {
                                        ui.selectable_value(
                                            &mut self.controls.max_colors,
                                            colors,
                                            colors.to_string(),
                                        );
                                    }
                                });
                            ui.label(
                                egui::RichText::new("Grid search · source pixels per cell")
                                    .strong(),
                            );
                            ui.horizontal(|ui| {
                                drag_u32(ui, "Min block", &mut self.controls.min_block, 1..=64);
                                drag_u32(ui, "Max block", &mut self.controls.max_block, 1..=128);
                            });
                            checkbox_with_help(
                                ui,
                                &mut self.controls.perspective,
                                "Correct perspective with four corners",
                                "Rectifies a photographed, rotated, or trapezoidal source before grid analysis. Normally off for generated or already axis-aligned images; enable it only when straight artwork edges are visibly skewed.",
                            );
                            if self.controls.perspective {
                                ui.label(
                                    egui::RichText::new(
                                        "Drag handles 1–4 over the artwork corners.",
                                    )
                                    .small()
                                    .color(Color32::from_gray(100)),
                                );
                            }

                            egui::CollapsingHeader::new("Grid & sampling")
                                .default_open(false)
                                .show(ui, |ui| self.grid_controls(ui));
                            egui::CollapsingHeader::new("Lattice fitting")
                                .default_open(false)
                                .show(ui, |ui| self.lattice_controls(ui));
                            egui::CollapsingHeader::new("Palette & edges")
                                .default_open(false)
                                .show(ui, |ui| self.palette_controls(ui));
                            egui::CollapsingHeader::new("Perspective output")
                                .default_open(false)
                                .show(ui, |ui| self.perspective_controls(ui));
                            if self.controls != custom_before {
                                self.controls.custom_modified = true;
                            }
                        }

                        ui.add_space(10.0);
                        let processing = self.pending.is_some();
                        let button = egui::Button::new(
                            egui::RichText::new(if processing {
                                "Optimizing…"
                            } else {
                                "Align pixels"
                            })
                            .strong()
                            .color(Color32::WHITE),
                        )
                        .fill(ACCENT)
                        .min_size(Vec2::new(ui.available_width(), 42.0));
                        if ui.add_enabled(!processing, button).clicked() {
                            self.begin_processing(context);
                        }
                        ui.label(
                            egui::RichText::new(&self.status)
                                .small()
                                .color(if self.error {
                                    Color32::from_rgb(170, 45, 35)
                                } else {
                                    Color32::from_gray(95)
                                }),
                        );
                        if let Some(result) = &self.result {
                            ui.separator();
                            ui.label(egui::RichText::new("Export").strong());
                            let mut save_request = None;
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Save PNG…").clicked() {
                                    save_request = Some((
                                        result.corrected.clone(),
                                        format!("{}.aligned.png", result.stem),
                                        "PNG",
                                        &["png"][..],
                                    ));
                                }
                                if ui.button("Save grid…").clicked() {
                                    save_request = Some((
                                        result.overlay.clone(),
                                        format!("{}.grid.png", result.stem),
                                        "PNG",
                                        &["png"][..],
                                    ));
                                }
                                if ui.button("Save report…").clicked() {
                                    save_request = Some((
                                        result.report.clone(),
                                        format!("{}.report.json", result.stem),
                                        "JSON",
                                        &["json"][..],
                                    ));
                                }
                            });
                            if let Some((bytes, filename, label, extensions)) = save_request {
                                self.begin_save_artifact(
                                    context, bytes, filename, label, extensions,
                                );
                            }
                        }
                    });
            });
    }

    fn grid_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Manual cell size (0 = search)").small());
        ui.horizontal(|ui| {
            drag_u32(ui, "Cell width", &mut self.controls.block_width, 0..=128);
            drag_u32(ui, "Cell height", &mut self.controls.block_height, 0..=128);
        });
        ui.horizontal(|ui| {
            drag_f64(ui, "Inset", &mut self.controls.inset, 0.0..=0.44, 0.01);
            drag_f64(
                ui,
                "Phase step",
                &mut self.controls.phase_step,
                0.01..=1.0,
                0.05,
            );
        });
        ui.horizontal(|ui| {
            drag_f64(
                ui,
                "Dimension step",
                &mut self.controls.dimension_step,
                0.01..=1.0,
                0.05,
            );
            drag_f64(
                ui,
                "Dimension radius",
                &mut self.controls.dimension_radius,
                0.0..=1.0,
                0.05,
            );
        });
        ui.horizontal(|ui| {
            drag_f64(
                ui,
                "Complexity",
                &mut self.controls.complexity,
                0.0..=1.0,
                0.001,
            );
            drag_u32(ui, "Output block", &mut self.controls.output_block, 0..=128);
        });
    }

    fn lattice_controls(&mut self, ui: &mut egui::Ui) {
        checkbox_with_help(
            ui,
            &mut self.controls.lattice_fit,
            "Fit non-uniform source lattice",
            "Fits a locally deformable 2D mesh around the detected grid. True corners snap first, then distance-weighted corner anchors guide edge-only points in the same row or column. Unanchored edges and low-detail areas stay fixed.",
        );
        ui.horizontal(|ui| {
            drag_f64(
                ui,
                "Lattice radius",
                &mut self.controls.lattice_radius,
                0.01..=32.0,
                0.25,
            );
            drag_f64(
                ui,
                "Lattice step",
                &mut self.controls.lattice_step,
                0.01..=8.0,
                0.05,
            );
        });
        ui.horizontal(|ui| {
            drag_f64(
                ui,
                "Lattice regularization",
                &mut self.controls.lattice_regularization,
                0.0..=1.0,
                0.005,
            );
            drag_f64(
                ui,
                "Edge weight",
                &mut self.controls.lattice_edge_weight,
                0.0..=1.0,
                0.01,
            );
        });
        ui.horizontal(|ui| {
            drag_f64(
                ui,
                "Min edge strength",
                &mut self.controls.lattice_min_edge,
                0.0..=1.0,
                0.01,
            );
            drag_u32(
                ui,
                "Lattice passes",
                &mut self.controls.lattice_iterations,
                1..=20,
            );
        });
    }

    fn palette_controls(&mut self, ui: &mut egui::Ui) {
        checkbox_with_help(
            ui,
            &mut self.controls.smart_palette,
            "Edge-aware smart palette",
            "Builds and scores perceptual palette candidates while protecting rare colors on strong edges. Normally enabled in Auto; disable it to use threshold clustering directly.",
        );
        ui.horizontal(|ui| {
            drag_usize(
                ui,
                "Candidate max",
                &mut self.controls.palette_candidate_max,
                2..=256,
            );
            drag_f64(
                ui,
                "Merge radius",
                &mut self.controls.merge_threshold,
                0.0..=1.0,
                0.005,
            );
        });
        checkbox_with_help(
            ui,
            &mut self.controls.palette_penalty_enabled,
            "Override adaptive palette penalty",
            "Replaces the image-adaptive color-complexity cost with the value below. Normally off; enable only when Auto consistently retains too many or too few colors.",
        );
        ui.add_enabled_ui(self.controls.palette_penalty_enabled, |ui| {
            drag_f64(
                ui,
                "Palette penalty",
                &mut self.controls.palette_penalty,
                0.0..=10.0,
                0.001,
            );
        });
        drag_f64(
            ui,
            "Edge emphasis",
            &mut self.controls.palette_edge_emphasis,
            0.0..=20.0,
            0.1,
        );
        ui.separator();
        ui.label(egui::RichText::new("One-cell ramp cleanup").strong());
        ui.horizontal(|ui| {
            drag_f64(
                ui,
                "Ramp penalty",
                &mut self.controls.ramp_penalty,
                0.0..=20.0,
                0.05,
            );
            drag_f64(
                ui,
                "Ramp contrast",
                &mut self.controls.ramp_contrast,
                0.001..=2.0,
                0.01,
            );
        });
        ui.horizontal(|ui| {
            drag_f64(
                ui,
                "Line tolerance",
                &mut self.controls.ramp_line_tolerance,
                0.001..=2.0,
                0.005,
            );
            drag_f64(
                ui,
                "Continuation",
                &mut self.controls.ramp_continuation,
                0.001..=2.0,
                0.005,
            );
        });
        drag_u32(ui, "Ramp passes", &mut self.controls.ramp_passes, 0..=32);
    }

    fn perspective_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("Leave these at 0 to estimate the flat working size.").small(),
        );
        ui.horizontal(|ui| {
            drag_u32(
                ui,
                "Rectified width",
                &mut self.controls.rectified_width,
                0..=16384,
            );
            drag_u32(
                ui,
                "Rectified height",
                &mut self.controls.rectified_height,
                0..=16384,
            );
        });
        if ui.button("Reset corner handles").clicked()
            && let Some(source) = &mut self.source
        {
            let x = source.width as f32 * 0.035;
            let y = source.height as f32 * 0.035;
            source.corners = [
                [x, y],
                [source.width as f32 - x, y],
                [source.width as f32 - x, source.height as f32 - y],
                [x, source.height as f32 - y],
            ];
        }
    }

    fn show_canvas(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        suppress_picker_click: bool,
    ) {
        egui::Frame::new()
            .fill(Color32::from_rgb(255, 253, 247))
            .inner_margin(16.0)
            .corner_radius(14.0)
            .show(ui, |ui| {
                if self.source.is_none() {
                    self.show_drop_zone(ui, context, suppress_picker_click);
                    return;
                }
                let source = self.source.as_ref().expect("source checked");
                let source_label = file_label(&source.path);
                let source_dimensions = format!("{} × {}", source.width, source.height);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(source_label).strong());
                    ui.label(
                        egui::RichText::new(source_dimensions)
                            .small()
                            .color(Color32::from_gray(100)),
                    );
                    if ui
                        .add_enabled(
                            !suppress_picker_click && self.dialog_pending.is_none(),
                            egui::Button::new("Choose another…").small(),
                        )
                        .clicked()
                    {
                        self.begin_pick_image(context);
                    }
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("input-output-scroll")
                    .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Input");
                        ui.selectable_value(&mut self.show_overlay, false, "Original");
                        if self.result.is_some() {
                            ui.selectable_value(
                                &mut self.show_overlay,
                                true,
                                "Detected grid",
                            );
                        }
                    });
                    if self.show_overlay {
                        if let Some(result) = &self.result {
                            egui::ScrollArea::horizontal()
                                .id_salt("detected-grid-horizontal")
                                .show(ui, |ui| {
                                    show_texture_unscaled(ui, &result.overlay_texture);
                                });
                            ui.label(
                                egui::RichText::new(
                                    "100% view · red mesh; dark supported edges; green corner anchors; cyan color-pick overrides.",
                                )
                                .small()
                                .color(Color32::from_gray(95)),
                            );
                        } else {
                            egui::ScrollArea::horizontal()
                                .id_salt("source-horizontal")
                                .show(ui, |ui| self.show_source_editor(ui));
                        }
                    } else {
                        egui::ScrollArea::horizontal()
                            .id_salt("source-horizontal")
                            .show(ui, |ui| self.show_source_editor(ui));
                        ui.label(
                            egui::RichText::new("100% view · no preview resampling")
                                .small()
                                .color(Color32::from_gray(95)),
                        );
                    }
                    if self.result.is_some() {
                        ui.add_space(14.0);
                        ui.separator();
                        ui.add_space(8.0);
                        self.show_result(ui);
                    }
                });
            });
    }

    fn show_drop_zone(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        suppress_picker_click: bool,
    ) {
        let available = ui.available_size();
        let size = Vec2::new(available.x.max(300.0), available.y.max(380.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        ui.painter().rect(
            rect.shrink(4.0),
            16.0,
            Color32::from_rgb(250, 247, 239),
            Stroke::new(2.0, Color32::from_rgb(198, 190, 172)),
            StrokeKind::Inside,
        );
        let center = rect.center();
        ui.painter().text(
            center - Vec2::new(0.0, 35.0),
            Align2::CENTER_CENTER,
            "+",
            FontId::proportional(56.0),
            ACCENT,
        );
        ui.painter().text(
            center + Vec2::new(0.0, 22.0),
            Align2::CENTER_CENTER,
            "Drop in pixel art",
            FontId::proportional(25.0),
            Color32::from_gray(25),
        );
        ui.painter().text(
            center + Vec2::new(0.0, 53.0),
            Align2::CENTER_CENTER,
            "PNG, JPEG, or WebP",
            FontId::proportional(14.0),
            Color32::from_gray(100),
        );
        if response.clicked() && !suppress_picker_click && self.dialog_pending.is_none() {
            self.begin_pick_image(context);
        }
        if context.input(|input| !input.raw.hovered_files.is_empty()) {
            ui.painter().rect_filled(
                rect.shrink(4.0),
                16.0,
                Color32::from_rgba_unmultiplied(240, 93, 61, 45),
            );
        }
    }

    fn show_source_editor(&mut self, ui: &mut egui::Ui) {
        let source = self.source.as_mut().expect("source exists");
        let size = Vec2::new(source.width as f32, source.height as f32);
        let (rect, response) = ui.allocate_exact_size(size, Sense::drag());
        ui.painter().image(
            source.texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        if !self.controls.perspective {
            return;
        }
        let to_screen = |point: [f32; 2]| {
            Pos2::new(
                rect.left() + point[0] / source.width as f32 * rect.width(),
                rect.top() + point[1] / source.height as f32 * rect.height(),
            )
        };
        let screen_points = source.corners.map(to_screen);
        for index in 0..4 {
            ui.painter().line_segment(
                [screen_points[index], screen_points[(index + 1) % 4]],
                Stroke::new(3.0, ACCENT),
            );
        }
        if response.drag_started()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.dragging_corner = screen_points
                .iter()
                .enumerate()
                .min_by(|(_, left), (_, right)| {
                    left.distance(pointer).total_cmp(&right.distance(pointer))
                })
                .and_then(|(index, point)| (point.distance(pointer) <= 28.0).then_some(index));
        }
        if response.dragged()
            && let (Some(index), Some(pointer)) =
                (self.dragging_corner, response.interact_pointer_pos())
        {
            source.corners[index] = [
                ((pointer.x - rect.left()) / rect.width() * source.width as f32)
                    .clamp(0.0, source.width as f32),
                ((pointer.y - rect.top()) / rect.height() * source.height as f32)
                    .clamp(0.0, source.height as f32),
            ];
        }
        if response.drag_stopped() {
            self.dragging_corner = None
        }
        for (index, point) in source.corners.map(to_screen).into_iter().enumerate() {
            ui.painter().circle(
                point,
                13.0,
                Color32::from_rgb(255, 253, 247),
                Stroke::new(2.0, Color32::from_gray(25)),
            );
            ui.painter().text(
                point,
                Align2::CENTER_CENTER,
                (index + 1).to_string(),
                FontId::proportional(12.0),
                Color32::from_gray(20),
            );
        }
        ui.painter().text(
            rect.left_top() + Vec2::new(12.0, 12.0),
            Align2::LEFT_TOP,
            "Drag handles onto TL · TR · BR · BL",
            FontId::proportional(13.0),
            Color32::WHITE,
        );
    }

    fn show_result(&self, ui: &mut egui::Ui) {
        let result = self.result.as_ref().expect("result exists");
        ui.horizontal(|ui| {
            ui.heading("Output");
            ui.label(
                egui::RichText::new("Rigid square pixels · indexed PNG")
                    .small()
                    .color(Color32::from_gray(95)),
            );
        });
        egui::ScrollArea::horizontal()
            .id_salt("corrected-output-horizontal")
            .show(ui, |ui| {
                show_texture_unscaled(ui, &result.corrected_texture);
            });
        ui.horizontal_wrapped(|ui| {
            metric(ui, "Source grid", &result.summary.source_grid);
            metric(ui, "Output", &result.summary.output_size);
            metric(
                ui,
                "Palette",
                &format!("{} colors", result.summary.palette_colors),
            );
            metric(
                ui,
                "Pick overrides",
                &result.summary.color_picker_overrides.to_string(),
            );
            metric(ui, "PNG", &format!("{}-bit", result.summary.bit_depth));
            metric(ui, "Fit", &format!("{:.5}", result.summary.fit_score));
            metric(
                ui,
                "Perspective",
                if result.summary.perspective {
                    "corrected"
                } else {
                    "off"
                },
            );
        });
    }
}

impl eframe::App for PixelPusherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, Color32::from_rgb(244, 241, 232));
        let dropped_file = self.handle_dropped_files(&context);
        self.poll_dialog(&context);
        self.poll_processing(&context);
        self.show_header(ui);
        ui.add_space(8.0);
        let available = ui.available_size().max(Vec2::splat(1.0));
        if available.x >= 720.0 {
            let controls_width = (available.x * 0.36).clamp(300.0, 400.0);
            let canvas_width = (available.x - controls_width - 10.0).max(1.0);
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(canvas_width, available.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.show_canvas(ui, &context, dropped_file),
                );
                ui.add_space(6.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(controls_width, available.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.show_controls(ui, &context),
                );
            });
        } else {
            self.show_canvas(ui, &context, dropped_file);
            ui.add_space(8.0);
            self.show_controls(ui, &context);
        }
    }
}

fn process_image(
    path: &Path,
    corners: [[f32; 2]; 4],
    controls: &Controls,
) -> Result<ProcessedPayload, String> {
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let output = temp.path().join("result.png");
    let mut command = Command::new(processor_executable()?);
    command.arg(path).arg("--output").arg(&output);
    if uses_automatic_pipeline(controls) {
        command.arg("--auto");
    } else {
        if controls.lattice_fit {
            command.arg("--lattice-fit");
        }
        if controls.smart_palette {
            command.arg("--smart-palette");
        }
        append(&mut command, "--min-block", controls.min_block);
        append(&mut command, "--max-block", controls.max_block);
        append(&mut command, "--inset", controls.inset);
        append(&mut command, "--phase-step", controls.phase_step);
        append(&mut command, "--dimension-step", controls.dimension_step);
        append(
            &mut command,
            "--dimension-radius",
            controls.dimension_radius,
        );
        append(&mut command, "--complexity", controls.complexity);
        append(&mut command, "--merge-threshold", controls.merge_threshold);
        append(
            &mut command,
            "--palette-candidate-max",
            controls.palette_candidate_max,
        );
        append(
            &mut command,
            "--palette-edge-emphasis",
            controls.palette_edge_emphasis,
        );
        if controls.max_colors > 0 {
            append(&mut command, "--max-colors", controls.max_colors);
        }
        if controls.palette_penalty_enabled {
            append(&mut command, "--palette-penalty", controls.palette_penalty);
        }
        if controls.block_width > 0 {
            append(&mut command, "--block-width", controls.block_width);
        }
        if controls.block_height > 0 {
            append(&mut command, "--block-height", controls.block_height);
        }
        if controls.output_block > 0 {
            append(&mut command, "--output-block", controls.output_block);
        }
        append(&mut command, "--lattice-radius", controls.lattice_radius);
        append(&mut command, "--lattice-step", controls.lattice_step);
        append(
            &mut command,
            "--lattice-regularization",
            controls.lattice_regularization,
        );
        append(
            &mut command,
            "--lattice-edge-weight",
            controls.lattice_edge_weight,
        );
        append(
            &mut command,
            "--lattice-min-edge",
            controls.lattice_min_edge,
        );
        append(
            &mut command,
            "--lattice-iterations",
            controls.lattice_iterations,
        );
        append(&mut command, "--ramp-penalty", controls.ramp_penalty);
        append(&mut command, "--ramp-contrast", controls.ramp_contrast);
        append(
            &mut command,
            "--ramp-line-tolerance",
            controls.ramp_line_tolerance,
        );
        append(
            &mut command,
            "--ramp-continuation",
            controls.ramp_continuation,
        );
        append(&mut command, "--ramp-passes", controls.ramp_passes);
        if controls.perspective {
            command.arg("--corners").arg(
                corners
                    .iter()
                    .map(|point| format!("{:.2},{:.2}", point[0], point[1]))
                    .collect::<Vec<_>>()
                    .join(";"),
            );
            if controls.rectified_width > 0 {
                append(&mut command, "--rectified-width", controls.rectified_width);
            }
            if controls.rectified_height > 0 {
                append(
                    &mut command,
                    "--rectified-height",
                    controls.rectified_height,
                );
            }
        }
    }
    let completed = command.output().map_err(|error| error.to_string())?;
    if !completed.status.success() {
        let stderr = String::from_utf8_lossy(&completed.stderr);
        return Err(stderr
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .unwrap_or("Pixel Pusher could not process this image")
            .trim_start_matches("Error: ")
            .to_owned());
    }
    let overlay_path = suffixed_path(&output, ".grid.png");
    let report_path = suffixed_path(&output, ".report.json");
    let corrected = std::fs::read(&output).map_err(|error| error.to_string())?;
    let overlay = std::fs::read(overlay_path).map_err(|error| error.to_string())?;
    let report = std::fs::read(report_path).map_err(|error| error.to_string())?;
    let json: Value = serde_json::from_slice(&report).map_err(|error| error.to_string())?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("pixel-art")
        .to_owned();
    Ok(ProcessedPayload {
        corrected,
        overlay,
        report,
        summary: report_summary(&json),
        stem,
    })
}

fn uses_automatic_pipeline(controls: &Controls) -> bool {
    controls.automatic || !controls.custom_modified
}

fn processor_executable() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let stem = current
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if stem == "pixel-pusher-gui" {
        let sibling =
            current.with_file_name(format!("pixel-pusher{}", std::env::consts::EXE_SUFFIX));
        if sibling.is_file() {
            Ok(sibling)
        } else {
            Err("pixel-pusher-gui needs the pixel-pusher executable beside it".to_owned())
        }
    } else {
        Ok(current)
    }
}

fn append(command: &mut Command, flag: &str, value: impl ToString) {
    command.arg(flag).arg(value.to_string());
}

fn load_texture(
    context: &egui::Context,
    path: &Path,
    name: &str,
) -> Result<(TextureHandle, u32, u32), String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let decoded = image::load_from_memory(&bytes)
        .map_err(|error| format!("Could not decode {}: {error}", path.display()))?;
    let width = decoded.width();
    let height = decoded.height();
    let rgba = decoded.to_rgba8();
    let image =
        ColorImage::from_rgba_unmultiplied([width as usize, height as usize], rgba.as_raw());
    Ok((
        context.load_texture(name, image, TextureOptions::NEAREST),
        width,
        height,
    ))
}

fn texture_from_bytes(
    context: &egui::Context,
    bytes: &[u8],
    name: &str,
) -> Result<TextureHandle, String> {
    let decoded = image::load_from_memory(bytes)
        .map_err(|error| error.to_string())?
        .to_rgba8();
    let size = [decoded.width() as usize, decoded.height() as usize];
    Ok(context.load_texture(
        name,
        ColorImage::from_rgba_unmultiplied(size, decoded.as_raw()),
        TextureOptions::NEAREST,
    ))
}

fn report_summary(report: &Value) -> Summary {
    let selected = &report["selected"];
    Summary {
        source_grid: format!(
            "{:.2} × {:.2} px",
            selected["cell_width"].as_f64().unwrap_or_default(),
            selected["cell_height"].as_f64().unwrap_or_default()
        ),
        output_size: format!(
            "{} × {}",
            report["output_width"].as_u64().unwrap_or_default(),
            report["output_height"].as_u64().unwrap_or_default()
        ),
        palette_colors: report["palette"].as_array().map_or(0, Vec::len),
        color_picker_overrides: report["color_picker_overrides"]
            .as_u64()
            .unwrap_or_default(),
        bit_depth: report["output_bit_depth"].as_u64().unwrap_or_default(),
        fit_score: selected["score"].as_f64().unwrap_or_default(),
        perspective: !report["perspective"].is_null(),
    }
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("image")
        .to_owned()
}

fn suffixed_path(input: &Path, suffix: &str) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    input.with_file_name(format!("{stem}{suffix}"))
}

fn drag_u32(ui: &mut egui::Ui, label: &str, value: &mut u32, range: std::ops::RangeInclusive<u32>) {
    ui.horizontal(|ui| {
        ui.label(label);
        info_badge(ui, parameter_help(label));
        ui.add(egui::DragValue::new(value).range(range).speed(1));
    });
}

fn drag_usize(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut usize,
    range: std::ops::RangeInclusive<usize>,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        info_badge(ui, parameter_help(label));
        ui.add(egui::DragValue::new(value).range(range).speed(1));
    });
}

fn drag_f64(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    speed: f64,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        info_badge(ui, parameter_help(label));
        ui.add(egui::DragValue::new(value).range(range).speed(speed));
    });
}

fn checkbox_with_help(ui: &mut egui::Ui, value: &mut bool, label: &str, help: &str) {
    ui.horizontal(|ui| {
        ui.checkbox(value, label);
        info_badge(ui, help);
    });
}

fn info_badge(ui: &mut egui::Ui, help: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(" i ")
                .monospace()
                .strong()
                .color(TEAL)
                .background_color(Color32::from_rgb(226, 239, 237)),
        )
        .sense(Sense::hover()),
    )
    .on_hover_ui(|ui| {
        ui.set_max_width(340.0);
        ui.label(help);
    });
}

fn parameter_help(label: &str) -> &'static str {
    match label {
        "Min block" => {
            "Smallest logical-cell dimension tested in source pixels. Lower values find very fine grids but increase search time. Normal: 2–4; default: 2."
        }
        "Max block" => {
            "Largest logical-cell dimension tested in source pixels. This is a hard search ceiling in both Auto and Custom modes. Raise it for heavily enlarged art. Normal: 16–32; default: 16."
        }
        "Cell width" => {
            "Forces the source cell width and disables automatic width selection in Custom mode. Use 0 to search. Normal: 0, or approximately 2–32 pixels."
        }
        "Cell height" => {
            "Forces the source cell height independently, allowing squeezed grids. Use 0 to search. Normal: 0, or approximately 2–32 pixels."
        }
        "Inset" => {
            "Fraction removed from every edge before measuring a cell color. More inset rejects anti-aliased boundaries but samples less interior area. Normal: 0.10–0.25; default: 0.14 (Auto uses 0.18)."
        }
        "Phase step" => {
            "Subpixel increment used while refining grid position. Smaller values improve alignment at the cost of more work. Normal: 0.10–0.50; default: 0.25."
        }
        "Dimension step" => {
            "Fractional increment used when fitting squeezed or stretched source cells. Smaller values search squeeze more precisely. Normal: 0.05–0.25; default: 0.10."
        }
        "Dimension radius" => {
            "Maximum fractional width/height change searched around an integer cell size. Normal: 0.5–0.9 source pixels; default: 0.75."
        }
        "Complexity" => {
            "Small preference for larger fundamental cells when divisor grids fit similarly. Too high can skip a real fine grid. Normal: 0.001–0.005; default: 0.002."
        }
        "Output block" => {
            "Square pixel size painted into the exported PNG. Use 0 for an area-preserving automatic value. Normal: 0 or 1–16; default: 0."
        }
        "Lattice radius" => {
            "Maximum distance one lattice junction may move from its regular-grid seed. Normal: 1–4 source pixels; default: 2."
        }
        "Lattice step" => {
            "Subpixel increment tested around each seeded lattice junction. Smaller values improve precision but cost more work. Normal: 0.1–0.5; default: 0.25."
        }
        "Lattice regularization" => {
            "Penalty for neighboring junctions receiving different displacements. Corner anchors also guide nearer points more strongly along their row or column. Normal: 0.005–0.03; default: 0.01."
        }
        "Edge weight" => {
            "Importance of distinct local source-color boundaries in the lattice objective. Strong edges receive more influence than flat spans. Normal: 0.04–0.15; default: 0.08."
        }
        "Min edge strength" => {
            "Minimum boundary contrast used for corner anchors and their row/column edge followers. Raise it to ignore soft detail and noise. Normal: 0.02–0.10; default: 0.04."
        }
        "Lattice passes" => {
            "Alternating column and row fitting passes. Later passes settle interactions between the two axes. Normal: 2–6; default: 4."
        }
        "Candidate max" => {
            "Largest fixed palette size evaluated by smart selection. Higher values preserve complex color sets but cost more work. Normal: 8–32; default: 12."
        }
        "Merge radius" => {
            "Perceptual distance below which nearly identical sampled colors merge. Higher values reduce the palette more aggressively. Normal: 0.02–0.06; default: 0.035."
        }
        "Palette penalty" => {
            "Manual cost for every palette color beyond two. Higher values choose fewer colors. Normal: adaptive/off; when overriding, start around 0–0.02."
        }
        "Edge emphasis" => {
            "Extra palette weight for colors touching high-contrast boundaries. Raise it to preserve rare outlines and highlights. Normal: 0.5–2.0; default: 1.0."
        }
        "Ramp penalty" => {
            "Strength of snapping a one-cell intermediate ramp toward a high-contrast endpoint. Set 0 to disable. Normal: 0.1–0.6; default: 0.3."
        }
        "Ramp contrast" => {
            "Minimum perceptual distance between the two sides of a removable ramp. Normal: 0.06–0.15; default: 0.08."
        }
        "Line tolerance" => {
            "How closely the middle color must lie on the color segment between ramp endpoints. Lower is stricter. Normal: 0.02–0.06; default: 0.035."
        }
        "Continuation" => {
            "Maximum color drift allowed as each side continues away from a candidate ramp. Lower requires flatter neighboring regions. Normal: 0.02–0.08; default: 0.04."
        }
        "Ramp passes" => {
            "Maximum cleanup passes over one-cell ramps. More passes can remove chains but may become aggressive. Normal: 0–2; default: 1."
        }
        "Rectified width" => {
            "Width of the perspective-corrected working image. Use 0 to estimate it from the four sides. Normal: 0 unless the true flat width is known."
        }
        "Rectified height" => {
            "Height of the perspective-corrected working image. Use 0 to estimate it from the four sides. Normal: 0 unless the true flat height is known."
        }
        _ => "Adjusts this processing parameter. The displayed value is the current setting.",
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(235, 231, 220))
        .corner_radius(20.0)
        .inner_margin(egui::Margin::symmetric(9, 5))
        .show(ui, |ui| {
            ui.label(format!("{label}: {value}"));
        });
}

fn show_texture_unscaled(ui: &mut egui::Ui, texture: &TextureHandle) {
    let texture_size = texture.size_vec2();
    ui.add(
        egui::Image::new(texture)
            .fit_to_exact_size(texture_size)
            .texture_options(TextureOptions::NEAREST),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_summary_reads_native_app_metrics() {
        let report = serde_json::json!({
            "selected": {"cell_width": 4.2, "cell_height": 3.8, "score": 0.1},
            "output_width": 80, "output_height": 60, "output_bit_depth": 4,
            "palette": [[0, 0, 0], [255, 255, 255]], "perspective": null,
            "color_picker_overrides": 7
        });
        let summary = report_summary(&report);
        assert_eq!(summary.source_grid, "4.20 × 3.80 px");
        assert_eq!(summary.palette_colors, 2);
        assert_eq!(summary.color_picker_overrides, 7);
        assert!(!summary.perspective);
    }

    #[test]
    fn untouched_custom_mode_uses_the_automatic_pipeline() {
        let mut controls = Controls {
            automatic: false,
            ..Controls::default()
        };
        assert!(uses_automatic_pipeline(&controls));

        controls.custom_modified = true;
        assert!(!uses_automatic_pipeline(&controls));

        controls.automatic = true;
        assert!(uses_automatic_pipeline(&controls));
    }

    #[test]
    fn every_numeric_control_has_specific_hover_help() {
        let labels = [
            "Min block",
            "Max block",
            "Cell width",
            "Cell height",
            "Inset",
            "Phase step",
            "Dimension step",
            "Dimension radius",
            "Complexity",
            "Output block",
            "Lattice radius",
            "Lattice step",
            "Lattice regularization",
            "Edge weight",
            "Min edge strength",
            "Lattice passes",
            "Candidate max",
            "Merge radius",
            "Palette penalty",
            "Edge emphasis",
            "Ramp penalty",
            "Ramp contrast",
            "Line tolerance",
            "Continuation",
            "Ramp passes",
            "Rectified width",
            "Rectified height",
        ];
        for label in labels {
            assert!(
                parameter_help(label).contains("Normal:"),
                "missing normal-range guidance for {label}"
            );
        }
    }
}
