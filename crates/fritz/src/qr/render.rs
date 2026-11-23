//! The modules of a QR code, as text, SVG and PNG.

use std::fmt::Write as _;

use anyhow::Context as _;
use qrcode::{Color, QrCode};

/// Starts black text on a bright white background, so the code is dark on
/// light also in a dark terminal.
pub(super) const LIGHT: &str = "\x1b[30;107m";
/// Resets the colors.
pub(super) const RESET: &str = "\x1b[0m";

/// The modules of a code with its border, row by row.
pub(super) struct Modules {
    /// Modules per side.
    pub(super) size: usize,
    dark: Vec<bool>,
}

impl Modules {
    pub(super) fn new(payload: &str, border: usize) -> anyhow::Result<Self> {
        let code = QrCode::new(payload.as_bytes()).context("the network does not fit a QR code")?;
        let width = code.width();
        let size = width + 2 * border;
        let mut dark = vec![false; size * size];
        for y in 0..width {
            for x in 0..width {
                dark[(y + border) * size + x + border] = code[(x, y)] == Color::Dark;
            }
        }
        Ok(Self { size, dark })
    }

    pub(super) fn dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.dark[y * self.size + x]
    }
}

/// The code in Unicode half blocks, two rows of modules per line. With
/// `color`, dark on light for any terminal.
pub(super) fn text(modules: &Modules, color: bool) -> String {
    let mut text = String::new();
    for y in (0..modules.size).step_by(2) {
        if color {
            text.push_str(LIGHT);
        }
        for x in 0..modules.size {
            text.push(match (modules.dark(x, y), modules.dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        if color {
            text.push_str(RESET);
        }
        text.push('\n');
    }
    text
}

/// The code as SVG: one path of the dark runs of each row, in modules,
/// scaled to `scale` pixels per module.
pub(super) fn svg(modules: &Modules, scale: usize) -> String {
    let size = modules.size;
    let pixels = size * scale;
    let mut path = String::new();
    for y in 0..size {
        let mut x = 0;
        while x < size {
            let start = x;
            while modules.dark(x, y) {
                x += 1;
            }
            let run = x - start;
            if run > 0 {
                let _ = write!(path, "M{start} {y}h{run}v1h-{run}z");
            }
            x += 1;
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pixels}\" height=\"{pixels}\" \
         viewBox=\"0 0 {size} {size}\" shape-rendering=\"crispEdges\">\n\
         <rect width=\"{size}\" height=\"{size}\" fill=\"#fff\"/>\n\
         <path fill=\"#000\" d=\"{path}\"/>\n\
         </svg>\n"
    )
}

/// The code as an 8-bit grayscale PNG, `scale` pixels per module.
pub(super) fn png(modules: &Modules, scale: usize) -> anyhow::Result<Vec<u8>> {
    let side = modules.size * scale;
    let mut pixels = Vec::with_capacity(side * side);
    for y in 0..side {
        for x in 0..side {
            pixels.push(if modules.dark(x / scale, y / scale) {
                0
            } else {
                255
            });
        }
    }
    let side = u32::try_from(side)?;
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, side, side);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&pixels)?;
    writer.finish()?;
    Ok(bytes)
}
