//! Approved application artwork shared by the native window and OS tray.
//! Embedded PNGs retain straight RGBA alpha; no runtime files are required.
use eframe::egui::{self, IconData};
use std::sync::OnceLock;

const WINDOW_PNG: &[u8] = include_bytes!("../assets/branding/artex-app-icon-256.png");
const TRAY_PNG: &[u8] = include_bytes!("../assets/branding/artex-app-icon-32.png");

fn decode(bytes: &[u8], size: u32) -> Result<IconData, String> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| format!("Cannot decode embedded ARTEX icon: {error}"))?
        .into_rgba8();
    if image.dimensions() != (size, size) {
        return Err(format!(
            "Embedded ARTEX icon must be {size} by {size} pixels"
        ));
    }
    Ok(IconData {
        width: size,
        height: size,
        rgba: image.into_raw(),
    })
}

pub(crate) fn window() -> Result<IconData, String> {
    static ICON: OnceLock<Result<IconData, String>> = OnceLock::new();
    ICON.get_or_init(|| decode(WINDOW_PNG, 256)).clone()
}

pub(crate) fn tray() -> Result<IconData, String> {
    static ICON: OnceLock<Result<IconData, String>> = OnceLock::new();
    ICON.get_or_init(|| decode(TRAY_PNG, 32)).clone()
}

/// Retain the GPU texture in this context so each titlebar frame reuses it.
pub(crate) fn title_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("artex-approved-title-icon");
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let icon = window().expect("Approved embedded ARTEX title icon");
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    let texture = ctx.load_texture(
        "ARTEX approved app icon",
        image,
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_embedded_pngs_preserve_dimensions_and_transparent_artwork() {
        for (icon, size) in [(window().unwrap(), 256), (tray().unwrap(), 32)] {
            assert_eq!((icon.width, icon.height), (size, size));
            assert_eq!(icon.rgba.len(), (size * size * 4) as usize);
            assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] == 0));
            assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));
        }
    }

    #[test]
    fn title_texture_is_retained_without_uploading_again_each_frame() {
        let ctx = egui::Context::default();
        let mut first_id = None;
        let first = ctx.run(egui::RawInput::default(), |ctx| {
            first_id = Some(title_texture(ctx).id());
        });
        assert!(
            first
                .textures_delta
                .set
                .iter()
                .any(|(id, _)| Some(*id) == first_id)
        );
        let second = ctx.run(egui::RawInput::default(), |ctx| {
            assert_eq!(Some(title_texture(ctx).id()), first_id);
        });
        assert!(
            !second
                .textures_delta
                .set
                .iter()
                .any(|(id, _)| Some(*id) == first_id)
        );
    }
}
