//! The app icon for the tray and notifications, embedded so they work before
//! (or without) installing the icon theme files.

use patch_linux::paths::APP_ID;
use std::path::{Path, PathBuf};

const TRAY_PNGS: [&[u8]; 4] = [
    include_bytes!("../../../packaging/linux/icons/hicolor/22x22/apps/com.mdhaarith.Patch.png"),
    include_bytes!("../../../packaging/linux/icons/hicolor/32x32/apps/com.mdhaarith.Patch.png"),
    include_bytes!("../../../packaging/linux/icons/hicolor/48x48/apps/com.mdhaarith.Patch.png"),
    include_bytes!("../../../packaging/linux/icons/hicolor/64x64/apps/com.mdhaarith.Patch.png"),
];
const LARGE_PNG: &[u8] = include_bytes!("../../../packaging/linux/icons/hicolor/256x256/apps/com.mdhaarith.Patch.png");

/// Decodes a PNG to straight (non-premultiplied) RGBA8.
pub fn decode_rgba(png_bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut data = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut data).ok()?;
    data.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => data,
        png::ColorType::Rgb => data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        _ => return None,
    };
    Some((info.width, info.height, rgba))
}

/// ARGB32 pixmaps in network byte order, as StatusNotifierItem wants them.
pub fn tray_pixmaps() -> Vec<ksni::Icon> {
    TRAY_PNGS
        .iter()
        .filter_map(|png| decode_rgba(png))
        .map(|(w, h, mut data)| {
            for px in data.chunks_exact_mut(4) {
                px.rotate_right(1);
            }
            ksni::Icon { width: w as i32, height: h as i32, data }
        })
        .collect()
}

fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        Some(d) => dirs.push(d),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty());
    dirs.extend(
        system
            .as_deref()
            .unwrap_or("/usr/local/share:/usr/share")
            .split(':')
            .map(PathBuf::from)
            .filter(|p| p.is_absolute()),
    );
    dirs
}

/// Whether the icon theme has our icon, i.e. Patch was installed rather than
/// run from a build directory. Desktop shells then look it up by name.
pub fn themed_icon_installed() -> bool {
    data_dirs().iter().any(|d| {
        ["48x48", "256x256", "scalable"].iter().any(|size| {
            let dir = d.join("icons/hicolor").join(size).join("apps");
            dir.join(format!("{APP_ID}.png")).exists() || dir.join(format!("{APP_ID}.svg")).exists()
        })
    })
}

/// Icon for notifications: the theme name when installed, otherwise a copy
/// of the embedded PNG in the cache directory (notification daemons take a
/// file path, not pixels, through the `app_icon` argument).
pub fn notification_icon(cache: &Path) -> String {
    if themed_icon_installed() {
        return APP_ID.into();
    }
    let path = cache.join("icon-256.png");
    let current = std::fs::read(&path).map(|b| b == LARGE_PNG).unwrap_or(false);
    if !current {
        let _ = std::fs::create_dir_all(cache);
        if patch_core::doc::write_atomic(&path, LARGE_PNG).is_err() {
            return APP_ID.into();
        }
    }
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icons_decode() {
        let px = tray_pixmaps();
        assert_eq!(px.len(), 4);
        assert_eq!((px[0].width, px[0].height), (22, 22));
        assert_eq!(px[0].data.len(), 22 * 22 * 4);
        // Corners of the rounded-square icon are transparent: alpha comes first in ARGB.
        assert_eq!(px[3].data[0], 0);
    }
}
