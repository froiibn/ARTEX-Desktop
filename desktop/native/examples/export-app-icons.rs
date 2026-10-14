//! Rebuild platform icon assets from the approved, unmodified imagegen PNG.
//! Run: cargo run --locked --manifest-path desktop/native/Cargo.toml --example export-app-icons
use image::{ImageEncoder, codecs::png::PngEncoder, imageops::FilterType};
use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/branding");
    let source = image::load_from_memory_with_format(
        &fs::read(directory.join("artex-app-icon-v1.png"))?,
        image::ImageFormat::Png,
    )?;
    if source.width() != source.height() || source.width() < 256 {
        return Err("The approved icon must be square and at least 256 pixels".into());
    }
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut frames = Vec::new();
    for size in sizes {
        let rgba = source
            .resize_exact(size, size, FilterType::Lanczos3)
            .into_rgba8();
        let mut png = Vec::new();
        PngEncoder::new(&mut png).write_image(
            rgba.as_raw(),
            size,
            size,
            image::ExtendedColorType::Rgba8,
        )?;
        if size == 32 || size == 256 {
            fs::write(directory.join(format!("artex-app-icon-{size}.png")), &png)?;
        }
        frames.push((size, png));
    }
    // Windows Vista and newer accept PNG frames in an ICO container. No extra codec dependency.
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * frames.len() as u32;
    for (size, png) in &frames {
        let dimension = if *size == 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[dimension, dimension, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in frames {
        ico.extend_from_slice(&png);
    }
    fs::write(directory.join("artex-app-icon.ico"), ico)?;
    println!("Exported 32/256 PNGs and seven Windows ICO sizes from the approved source.");
    Ok(())
}
