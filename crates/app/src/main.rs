//! Codebase Intel — a desktop map of a repository.
//!
//! Double-clicking the binary opens a window; there is no console mode.

// In release builds this hides the console window that would otherwise sit
// behind the app. Debug builds keep it so panics and logs are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod flow;
mod graph;
mod repo;
mod scan;
mod settings;

fn main() -> eframe::Result {
    install_panic_dialog();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Codebase Intel")
            .with_inner_size([1380.0, 880.0])
            .with_min_inner_size([960.0, 640.0])
            .with_icon(app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Codebase Intel",
        options,
        Box::new(|context| Ok(Box::new(app::App::new(context)))),
    )
}

/// In release builds stderr is not visible, so a panic would otherwise close
/// the window with no explanation. Show it in a dialog instead.
fn install_panic_dialog() {
    let previous = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        previous(info);

        let message = format!("Codebase Intel hit an unexpected error:\n\n{info}");
        let _ = rfd::MessageDialog::new()
            .set_title("Codebase Intel")
            .set_description(message)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }));
}

/// A small generated icon: three nodes connected by lines, on a dark rounded
/// tile. Generated in code so the binary stays a single file.
fn app_icon() -> egui::IconData {
    const SIZE: usize = 64;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];

    let background = (24u8, 27u8, 33u8);
    let accent = (94u8, 168u8, 255u8);
    let node = (232u8, 237u8, 244u8);

    for y in 0..SIZE {
        for x in 0..SIZE {
            let index = (y * SIZE + x) * 4;
            let (r, g, b) = if corner_alpha(x, y, SIZE) > 0 {
                background
            } else {
                (0, 0, 0)
            };
            rgba[index] = r;
            rgba[index + 1] = g;
            rgba[index + 2] = b;
            rgba[index + 3] = if corner_alpha(x, y, SIZE) > 0 { 255 } else { 0 };
        }
    }

    let nodes = [(18.0f32, 20.0f32), (46.0, 30.0), (26.0, 47.0)];
    for (index, (x, y)) in nodes.iter().enumerate() {
        let color = if index == 1 { node } else { accent };
        draw_disc(&mut rgba, *x, *y, 6.5, color);
    }
    draw_line(&mut rgba, nodes[0], nodes[1], accent);
    draw_line(&mut rgba, nodes[0], nodes[2], accent);
    draw_line(&mut rgba, nodes[1], nodes[2], accent);

    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}

fn corner_alpha(x: usize, y: usize, size: usize) -> u8 {
    let radius = 12.0;
    let fx = x as f32;
    let fy = y as f32;
    let max = size as f32 - 1.0;

    let corner_x = if fx < radius {
        radius - fx
    } else if fx > max - radius {
        fx - (max - radius)
    } else {
        0.0
    };
    let corner_y = if fy < radius {
        radius - fy
    } else if fy > max - radius {
        fy - (max - radius)
    } else {
        0.0
    };

    if corner_x * corner_x + corner_y * corner_y > radius * radius {
        0
    } else {
        255
    }
}

fn draw_disc(rgba: &mut [u8], cx: f32, cy: f32, radius: f32, color: (u8, u8, u8)) {
    let size = 64usize;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= radius * radius {
                let index = (y * size + x) * 4;
                rgba[index] = color.0;
                rgba[index + 1] = color.1;
                rgba[index + 2] = color.2;
                rgba[index + 3] = 255;
            }
        }
    }
}

fn draw_line(rgba: &mut [u8], from: (f32, f32), to: (f32, f32), color: (u8, u8, u8)) {
    let steps = 80;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let x = from.0 + (to.0 - from.0) * t;
        let y = from.1 + (to.1 - from.1) * t;
        draw_disc(rgba, x, y, 1.6, color);
    }
}


