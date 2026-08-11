#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() -> eframe::Result {
    pixel_pusher::gui::run_with_image(std::env::args_os().nth(1).map(Into::into))
}
