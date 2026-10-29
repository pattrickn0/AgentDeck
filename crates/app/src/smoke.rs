//! Verification mode (port of `HOLT_SMOKE`): with `HOLT_SMOKE=<folder>`, after
//! `HOLT_SMOKE_DELAY` ms (default 4000) reads the framebuffer, saves `<folder>/screen.png` and exits.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glow::HasContext;

pub struct Smoke {
    pub dir: PathBuf,
    pub at: Instant,
    pub armed: bool,
}

impl Smoke {
    pub fn from_env() -> Option<Self> {
        let dir = holt_core::env_var_os("SMOKE")?;
        let delay = holt_core::env_var("SMOKE_DELAY").and_then(|v| v.parse::<u64>().ok()).unwrap_or(4000);
        Some(Self { dir: PathBuf::from(dir), at: Instant::now() + Duration::from_millis(delay), armed: false })
    }
}

/// Reads the current framebuffer (RGBA, origin at the bottom) and returns the rows top to bottom.
pub fn read_framebuffer(gl: &glow::Context, width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    // SAFETY: buffer with the exact width×height×RGBA8 size.
    unsafe {
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(0, 0, width as i32, height as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
    }
    flip_rows(&pixels, (width * 4) as usize)
}

fn flip_rows(pixels: &[u8], stride: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    for row in pixels.chunks(stride).rev() {
        out.extend_from_slice(row);
    }
    out
}

pub fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    // Forces an opaque alpha (the framebuffer may have alpha < 1 in some configurations).
    let opaque: Vec<u8> = rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    w.write_image_data(&opaque)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flips_rows() {
        let px = [1, 1, 2, 2, 3, 3];
        assert_eq!(flip_rows(&px, 2), vec![3, 3, 2, 2, 1, 1]);
    }
}
