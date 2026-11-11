//! What ⌘V pastes into a pane.
//!
//! gpui's macOS clipboard only knows plain text and raw image bytes, so on its own
//! a copied file (Finder, screenshot tools) or an image with no text pastes
//! nothing. Like WezTerm we prefer copied files and paste their quoted paths. If
//! the clipboard holds only an image, we go further than WezTerm: we save the
//! image to a temp file and paste that path, which Claude Code / pi attach as an
//! image.

#![allow(unexpected_cfgs)] // objc 0.2 macros

use gpui::{App, ClipboardEntry, ImageFormat};
use std::path::PathBuf;

/// The text to paste for the current clipboard, or `None` if it's empty.
pub fn paste_text(cx: &App) -> Option<String> {
    let quoting = config::configuration().quote_dropped_files;
    let quote = |paths: &[PathBuf]| {
        paths
            .iter()
            .map(|p| quoting.escape(&p.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ")
    };

    let files = copied_files();
    if !files.is_empty() {
        return Some(quote(&files));
    }
    let item = cx.read_from_clipboard()?;
    if let Some(text) = item.text() {
        return Some(text);
    }
    item.entries().iter().find_map(|e| match e {
        ClipboardEntry::Image(img) => match save_image(img.format, &img.bytes) {
            Ok(path) => Some(quote(&[path])),
            Err(err) => {
                log::warn!("couldn't save pasted image: {err:#}");
                None
            }
        },
        _ => None,
    })
}

fn save_image(format: ImageFormat, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
    };
    let dir = std::env::temp_dir().join("wezzy-paste");
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let path = dir.join(format!("paste-{stamp}.{ext}"));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Files on the general pasteboard (copied in Finder, or by screenshot tools).
#[cfg(target_os = "macos")]
fn copied_files() -> Vec<PathBuf> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::{CStr, CString};

    let mut out = vec![];
    unsafe {
        let pb: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
        if pb.is_null() {
            return out;
        }
        let ty = CString::new("NSFilenamesPboardType").unwrap();
        let ty: *mut Object = msg_send![class!(NSString), stringWithUTF8String: ty.as_ptr()];
        let list: *mut Object = msg_send![pb, propertyListForType: ty];
        if list.is_null() {
            return out;
        }
        let is_array: bool = msg_send![list, isKindOfClass: class!(NSArray)];
        if !is_array {
            return out;
        }
        let n: usize = msg_send![list, count];
        for i in 0..n {
            let s: *mut Object = msg_send![list, objectAtIndex: i];
            let cs: *const std::os::raw::c_char = msg_send![s, UTF8String];
            if !cs.is_null() {
                out.push(PathBuf::from(CStr::from_ptr(cs).to_string_lossy().into_owned()));
            }
        }
    }
    out
}

#[cfg(not(target_os = "macos"))]
fn copied_files() -> Vec<PathBuf> {
    vec![]
}
