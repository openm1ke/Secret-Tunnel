//! Icons of Windows programs for the application picker: the 48 px icon
//! Explorer shows for the file, read as pixels and sent as a PNG data URL.
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::UI::Controls::{ImageList_GetIcon, HIMAGELIST, ILD_TRANSPARENT};
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHGetImageList, SHFILEINFOW, SHGFI_SYSICONINDEX, SHIL_EXTRALARGE};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

/// IID_IImageList: SHGetImageList hands back the system image list.
const IID_IMAGE_LIST: windows_sys::core::GUID = windows_sys::core::GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);

fn cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Icons for absolute paths of programs or shortcuts; other entries (a bare
/// process name added by hand) keep their letter badge.
pub fn icons(paths: &[String]) -> HashMap<String, String> {
    // The shell needs COM on this thread; a second call is harmless.
    unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    let mut list: *mut core::ffi::c_void = std::ptr::null_mut();
    let listed = unsafe { SHGetImageList(SHIL_EXTRALARGE as i32, &IID_IMAGE_LIST, &mut list) };
    let list = if listed >= 0 && !list.is_null() { Some(list as HIMAGELIST) } else { None };
    let mut out = HashMap::new();
    for path in paths {
        let absolute = path.get(1..3) == Some(":\\") || path.starts_with("\\\\");
        if !absolute {
            continue;
        }
        let known = cache().lock().ok().and_then(|cache| cache.get(path).cloned());
        let icon = match known {
            Some(icon) => icon,
            None => {
                let icon = list.and_then(|list| icon_of(path, list));
                if let Ok(mut cache) = cache().lock() {
                    cache.insert(path.clone(), icon.clone());
                }
                icon
            }
        };
        if let Some(icon) = icon {
            out.insert(path.clone(), icon);
        }
    }
    out
}

fn icon_of(path: &str, list: HIMAGELIST) -> Option<String> {
    let wide: Vec<u16> = OsStr::new(path).encode_wide().chain(std::iter::once(0)).collect();
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let found = unsafe {
        SHGetFileInfoW(wide.as_ptr(), 0, &mut info, std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX)
    };
    if found == 0 {
        return None;
    }
    let icon = unsafe { ImageList_GetIcon(list, info.iIcon, ILD_TRANSPARENT) };
    if icon.is_null() {
        return None;
    }
    let png = pixels(icon);
    unsafe { DestroyIcon(icon) };
    png.map(|png| crate::icon_png::data_url(&png))
}

/// The icon's pixels as a PNG. Old icons without alpha take it from their mask.
fn pixels(icon: HICON) -> Option<Vec<u8>> {
    let mut info: ICONINFO = unsafe { std::mem::zeroed() };
    if unsafe { GetIconInfo(icon, &mut info) } == 0 {
        return None;
    }
    let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
    let result = (|| {
        if info.hbmColor.is_null() || dc.is_null() {
            return None;
        }
        let read = |bitmap, header: &mut BITMAPINFO| -> Option<(u32, u32, Vec<u8>)> {
            header.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            if unsafe { GetDIBits(dc, bitmap, 0, 0, std::ptr::null_mut(), header, DIB_RGB_COLORS) } == 0 {
                return None;
            }
            let (width, height) = (header.bmiHeader.biWidth.unsigned_abs(), header.bmiHeader.biHeight.unsigned_abs());
            if width == 0 || height == 0 || width > 512 || height > 512 {
                return None;
            }
            header.bmiHeader.biBitCount = 32;
            header.bmiHeader.biCompression = BI_RGB;
            header.bmiHeader.biPlanes = 1;
            header.bmiHeader.biHeight = -(height as i32); // top-down rows
            let mut bits = vec![0_u8; (width * height * 4) as usize];
            let lines = unsafe { GetDIBits(dc, bitmap, 0, height, bits.as_mut_ptr().cast(), header, DIB_RGB_COLORS) };
            (lines > 0).then_some((width, height, bits))
        };
        let mut header: BITMAPINFO = unsafe { std::mem::zeroed() };
        let color = read(info.hbmColor, &mut header);
        let (width, height, mut bgra) = color?;
        if bgra.chunks(4).all(|pixel| pixel[3] == 0) {
            let mut mask_header: BITMAPINFO = unsafe { std::mem::zeroed() };
            if let Some((_, _, mask)) = read(info.hbmMask, &mut mask_header).filter(|(w, h, _)| (*w, *h) == (width, height)) {
                for (pixel, bit) in bgra.chunks_mut(4).zip(mask.chunks(4)) {
                    pixel[3] = if bit[0] == 0 { 255 } else { 0 };
                }
            } else {
                bgra.chunks_mut(4).for_each(|pixel| pixel[3] = 255);
            }
        }
        let rgba: Vec<u8> = bgra.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        Some(crate::icon_png::encode_rgba(width, height, &rgba))
    })();
    unsafe {
        if !dc.is_null() {
            DeleteDC(dc);
        }
        DeleteObject(info.hbmColor);
        DeleteObject(info.hbmMask);
    }
    result
}
