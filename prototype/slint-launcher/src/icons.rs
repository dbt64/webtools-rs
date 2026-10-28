//! Shell icon extraction for launcher rows.

use crate::index::Launch;
use serde_json::Value;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::collections::HashMap;

/// Only icons already saved by Manager are read; rendering never fetches a favicon.
pub fn saved_website_icons(root: &std::path::Path, websites: &Value) -> HashMap<String, Image> {
    let ids = websites
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|site| site["id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut icons = HashMap::new();
    if ids.is_empty() {
        return icons;
    }
    for path in [
        root.join("nook-data.json"),
        root.join("runtime/wt-manager-profile/nook-data.json"),
    ] {
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        if file
            .metadata()
            .map(|meta| meta.len() > 32 * 1024 * 1024)
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(data) = read_saved_website_document(file) else {
            continue;
        };
        for site in data["webEntries"].as_array().into_iter().flatten() {
            let Some(id) = site["id"].as_str().filter(|id| ids.contains(id)) else {
                continue;
            };
            if let Some(icon) = site["favicon"].as_str().and_then(from_data_url) {
                icons.insert(id.to_string(), icon);
            }
        }
    }
    for site in websites.as_array().into_iter().flatten() {
        if let (Some(id), Some(icon)) = (
            site["id"].as_str(),
            site["favicon"].as_str().and_then(from_data_url),
        ) {
            icons.insert(id.to_string(), icon);
        }
    }
    icons
}

fn read_saved_website_document(reader: impl std::io::Read) -> serde_json::Result<Value> {
    serde_json::from_reader(std::io::BufReader::new(reader))
}

pub fn from_data_url(reference: &str) -> Option<Image> {
    use windows_sys::Win32::Security::Cryptography::{CryptStringToBinaryW, CRYPT_STRING_BASE64};
    if reference.len() > 500_000 {
        return None;
    }
    let (header, encoded) = reference.split_once(',')?;
    let header = header.to_ascii_lowercase();
    if !header.starts_with("data:image/") || !header.ends_with(";base64") {
        return None;
    }
    let wide = encoded.encode_utf16().collect::<Vec<_>>();
    let mut length = 0u32;
    unsafe {
        if CryptStringToBinaryW(
            wide.as_ptr(),
            wide.len() as u32,
            CRYPT_STRING_BASE64,
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == 0
        {
            return None;
        }
        let mut bytes = vec![0u8; length as usize];
        if CryptStringToBinaryW(
            wide.as_ptr(),
            wide.len() as u32,
            CRYPT_STRING_BASE64,
            bytes.as_mut_ptr(),
            &mut length,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == 0
        {
            return None;
        }
        bytes.truncate(length as usize);
        decode_system_image(&bytes).or_else(|| {
            if header.contains("svg+xml") {
                Image::load_from_data(&bytes, Some("svg")).ok()
            } else {
                None
            }
        })
    }
}

/// WIC is also WPF's image decoder. It supports saved ICO/GIF images without
/// enabling new Slint decoder crates, and scales before retaining bitmap pixels.
fn decode_system_image(bytes: &[u8]) -> Option<Image> {
    use windows::Win32::{Graphics::Imaging::*, System::Com::*, UI::Shell::SHCreateMemStream};
    struct Apartment(bool);
    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    unsafe {
        let _apartment = Apartment(CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok());
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = SHCreateMemStream(Some(bytes))?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut width, mut height) = (0, 0);
        frame.GetSize(&mut width, &mut height).ok()?;
        if width == 0 || height == 0 {
            return None;
        }
        let scale = (64.0 / width.max(height) as f32).min(1.0);
        let width = (width as f32 * scale).round().max(1.0) as u32;
        let height = (height as f32 * scale).round().max(1.0) as u32;
        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler
            .Initialize(&frame, width, height, WICBitmapInterpolationModeFant)
            .ok()?;
        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &scaler,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None::<&IWICPalette>,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
        converter
            .CopyPixels(std::ptr::null(), width * 4, pixels.make_mut_bytes())
            .ok()?;
        Some(Image::from_rgba8(pixels))
    }
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<Image>>> = RefCell::new(HashMap::new());
}

pub fn for_entry(launch: &Launch) -> Option<Image> {
    let path = match launch {
        Launch::Shortcut { path, .. } | Launch::Executable(path) => path.clone(),
        Launch::Packaged(aumid) => format!("shell:AppsFolder\\{aumid}"),
        Launch::System(app) if app == "recycle-bin" => "shell:RecycleBinFolder".to_string(),
        Launch::System(app) => crate::index::system_icon_source(app)?.to_string(),
        Launch::None => return None,
    };
    if let Some(hit) = CACHE.with(|cache| cache.borrow().get(&path).cloned()) {
        return hit;
    }
    let image = extract(&path);
    CACHE.with(|cache| cache.borrow_mut().insert(path, image.clone()));
    image
}

/// Extracts shell icon pixels for a file-search result. Call from a worker thread;
/// the caller can create the Slint image on the UI thread after the query returns.
pub fn file_path_pixels(path: &str) -> Option<(Vec<u8>, u32)> {
    extract_pixels(path)
}

pub fn image_from_pixels(bytes: &[u8], edge: u32) -> Image {
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(bytes, edge, edge);
    Image::from_rgba8_premultiplied(buffer)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn extract(path: &str) -> Option<Image> {
    let (bytes, edge) = extract_pixels(path)?;
    Some(image_from_pixels(&bytes, edge))
}

fn extract_pixels(path: &str) -> Option<(Vec<u8>, u32)> {
    if path.starts_with("shell:") {
        return extract_shell_pixels(path);
    }
    let wide_path = wide(path);
    unsafe {
        use windows_sys::Win32::UI::Shell::{
            SHDefExtractIconW, SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON,
        };
        let mut icon = std::ptr::null_mut();
        let size: i32;
        let status = SHDefExtractIconW(
            wide_path.as_ptr(),
            0,
            0,
            &mut icon,
            std::ptr::null_mut(),
            48,
        );
        if status >= 0 && !icon.is_null() {
            size = 48;
        } else {
            let mut info: SHFILEINFOW = std::mem::zeroed();
            let flags = SHGFI_ICON | SHGFI_LARGEICON;
            let ok = SHGetFileInfoW(
                wide_path.as_ptr(),
                0,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                flags,
            );
            if ok == 0 || info.hIcon.is_null() {
                return None;
            }
            icon = info.hIcon;
            size = 32;
        }
        let pixels = icon_to_rgba(icon, size);
        windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(icon);
        pixels
    }
}

unsafe fn icon_to_rgba(icon: *mut std::ffi::c_void, size: i32) -> Option<(Vec<u8>, u32)> {
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DrawIconEx, DI_NORMAL};
    let screen = GetDC(std::ptr::null_mut());
    if screen.is_null() {
        return None;
    }
    let memory = CreateCompatibleDC(screen);
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = size;
    info.bmiHeader.biHeight = -size;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB as u32;
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(
        memory,
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    if dib.is_null() || bits.is_null() {
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        return None;
    }
    let previous = SelectObject(memory, dib);
    DrawIconEx(
        memory,
        0,
        0,
        icon,
        size,
        size,
        0,
        std::ptr::null_mut(),
        DI_NORMAL,
    );
    let count = (size * size) as usize;
    let source = std::slice::from_raw_parts(bits as *const u8, count * 4);
    let mut rgba = Vec::with_capacity(count * 4);
    for pixel in source.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    SelectObject(memory, previous);
    DeleteObject(dib);
    DeleteDC(memory);
    ReleaseDC(std::ptr::null_mut(), screen);
    Some((rgba, size as u32))
}
/// Shell namespace items (UWP/packaged apps) resolve through a PIDL, not a path.
fn extract_shell_pixels(address: &str) -> Option<(Vec<u8>, u32)> {
    unsafe {
        use windows_sys::Win32::System::Com::CoTaskMemFree;
        use windows_sys::Win32::UI::Shell::{
            SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON,
            SHGFI_PIDL,
        };
        {
            use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED as u32);
        }
        let name = wide(address);
        let mut pidl: *mut windows_sys::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        let status = SHParseDisplayName(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut pidl,
            0,
            std::ptr::null_mut(),
        );
        if status < 0 || pidl.is_null() {
            if std::env::var("WT_ICON_DEBUG").is_ok() {
                eprintln!("icon parse failed for {address}: {status:#x}");
            }
            return None;
        }
        let mut info: SHFILEINFOW = std::mem::zeroed();
        let flags = SHGFI_PIDL | SHGFI_ICON | SHGFI_LARGEICON;
        let ok = SHGetFileInfoW(
            pidl as *const u16,
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        );
        CoTaskMemFree(pidl as *const std::ffi::c_void);
        if ok == 0 || info.hIcon.is_null() {
            return None;
        }
        let pixels = icon_to_rgba(info.hIcon, 32);
        windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(info.hIcon);
        pixels
    }
}

#[cfg(test)]
mod data_url_tests {
    use super::*;
    const ICON: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwAAHYcAAB2HAY/l8WUAAAAQSURBVBhXY5Cb8P8/AzIAACfzAq1LSISMAAAAAElFTkSuQmCC";

    #[test]
    fn saved_favicon_json_does_not_issue_a_file_read_for_every_byte() {
        struct Reads<'a> {
            input: std::io::Cursor<Vec<u8>>,
            calls: &'a std::cell::Cell<usize>,
        }
        impl std::io::Read for Reads<'_> {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                self.calls.set(self.calls.get() + 1);
                std::io::Read::read(&mut self.input, bytes)
            }
        }
        let document = serde_json::to_vec(
            &serde_json::json!({"webEntries":[{"id":"site", "favicon":"a".repeat(280_000)}]}),
        )
        .unwrap();
        let calls = std::cell::Cell::new(0);
        let data = read_saved_website_document(Reads {
            input: std::io::Cursor::new(document),
            calls: &calls,
        })
        .unwrap();
        assert_eq!(data["webEntries"][0]["id"], "site");
        assert!(
            calls.get() < 128,
            "280 KB must be buffered, actual file reads={}",
            calls.get()
        );
    }

    #[test]
    fn saved_png_data_is_decoded_without_a_window_or_network() {
        let icon = from_data_url(ICON).expect("saved PNG must decode");
        assert_eq!((icon.size().width, icon.size().height), (2, 2));
        assert_eq!(icon.to_rgba8().unwrap().as_slice()[0].b, 255);
    }

    #[test]
    #[ignore = "read-only probe of the reported saved favicon; requires WT_ICON_PROBE_PROFILE"]
    fn reported_favicon_decode_probe() {
        let root = std::path::PathBuf::from(std::env::var_os("WT_ICON_PROBE_PROFILE").unwrap());
        let data: Value = serde_json::from_reader(
            std::fs::File::open(root.join("runtime/wt-manager-profile/nook-data.json")).unwrap(),
        )
        .unwrap();
        let reference = data["webEntries"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|site| site["favicon"].as_str())
            .expect("saved icon");
        eprintln!("icon-probe: encoded-bytes={}", reference.len());
        for attempt in 0..3 {
            let at = std::time::Instant::now();
            eprintln!("icon-probe: begin={attempt}");
            let image = from_data_url(reference);
            eprintln!(
                "icon-probe: finish={attempt} ms={:.2} decoded={}",
                at.elapsed().as_secs_f64() * 1000.0,
                image.is_some()
            );
        }
    }

    #[test]
    fn unavailable_or_invalid_icons_use_the_existing_fallback() {
        for reference in [
            "",
            "https://site.example/icon.png",
            "data:text/plain;base64,YQ==",
            "data:image/png;base64,invalid!",
        ] {
            assert!(from_data_url(reference).is_none());
        }
    }

    #[test]
    fn saved_gif_favicon_uses_windows_image_decoding() {
        let image = from_data_url(
            "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
        )
        .expect("WPF-compatible saved GIF favicon must be decoded");
        assert_eq!((image.size().width, image.size().height), (1, 1));
    }

    #[test]
    fn saved_ico_favicon_is_decoded() {
        let image = from_data_url("data:image/x-icon;base64,AAABAAEAAgIAAAEAIAB7AAAAFgAAAIlQTkcNChoKAAAADUlIRFIAAAACAAAAAggGAAAAcrYNJAAAAAFzUkdCAK7OHOkAAAAEZ0FNQQAAsY8L/GEFAAAACXBIWXMAAB2HAAAdhwGP5fFlAAAAEElEQVQYV2OQm/D/PwMyAAAn8wKtS0iEjAAAAABJRU5ErkJggg==")
            .expect("saved ICO favicon must be decoded");
        assert_eq!((image.size().width, image.size().height), (2, 2));
    }

    #[test]
    fn large_saved_images_keep_only_a_thumbnail_with_original_aspect_ratio() {
        let mut bitmap = vec![0u8; 54 + 128 * 256 * 4];
        bitmap[..2].copy_from_slice(b"BM");
        let length = bitmap.len() as u32;
        bitmap[2..6].copy_from_slice(&length.to_le_bytes());
        bitmap[10..14].copy_from_slice(&54u32.to_le_bytes());
        bitmap[14..18].copy_from_slice(&40u32.to_le_bytes());
        bitmap[18..22].copy_from_slice(&128i32.to_le_bytes());
        bitmap[22..26].copy_from_slice(&256i32.to_le_bytes());
        bitmap[26..28].copy_from_slice(&1u16.to_le_bytes());
        bitmap[28..30].copy_from_slice(&32u16.to_le_bytes());
        let image = decode_system_image(&bitmap).unwrap();
        assert_eq!((image.size().width, image.size().height), (32, 64));
    }

    #[test]
    fn favicon_reader_uses_the_isolated_manager_profile_and_requested_ids() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/shortcut-icons")
            .join(webtools_ipc::new_request_id());
        let manager = root.join("runtime/wt-manager-profile");
        std::fs::create_dir_all(&manager).unwrap();
        std::fs::write(
            manager.join("nook-data.json"),
            serde_json::to_vec(&serde_json::json!({
                "webEntries":[{"id":"keep", "favicon":ICON}, {"id":"other", "favicon":ICON}]
            }))
            .unwrap(),
        )
        .unwrap();
        let icons = saved_website_icons(&root, &serde_json::json!([{"id":"keep"}]));
        assert_eq!(icons.len(), 1);
        assert!(icons.contains_key("keep"));
    }
}
