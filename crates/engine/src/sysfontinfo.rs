//! The font source PDFium is given, in PDFium's own interface (`FPDF_SYSFONTINFO`).
//!
//! `pdfium-render` has a wrapper for this interface, but it turns the face name into a Rust string
//! before the engine sees the request, and a name that is not UTF-8 ends the request with "no font".
//! Names in GBK, Shift-JIS, EUC-KR and Big5 are what many Asian PDFs carry in `/BaseFont`, and for
//! them the character set that PDFium passes is the only thing that tells the script. So the engine
//! speaks the interface itself: small, no file or network access, and every request answered from
//! the fonts in memory (`fonts::MemoryFonts`).
//!
//! Each font handed to PDFium is a handle (a number) that stands for bytes held by the engine in
//! one shared copy; PDFium copies them into its own buffer when it loads the face. A handle lives
//! until PDFium deletes it.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uchar, c_uint, c_ulong, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use pdfium_render::prelude::{FPDF_BOOL, FPDF_SYSFONTINFO};

use crate::fonts::MemoryFonts;

/// What a handle stands for.
struct Handle {
    data: Arc<[u8]>,
    /// The face name as PDFium gave it, with a closing zero.
    face: Vec<u8>,
    charset: c_int,
}

/// The structure PDFium sees. `base` comes first, so a pointer to this is a pointer to the
/// interface structure, and back.
#[repr(C)]
pub struct SysFontInfo {
    base: FPDF_SYSFONTINFO,
    fonts: MemoryFonts,
    handles: HashMap<usize, Handle>,
    next: usize,
}

impl SysFontInfo {
    pub fn new(fonts: MemoryFonts) -> Box<SysFontInfo> {
        Box::new(SysFontInfo {
            base: FPDF_SYSFONTINFO {
                // Version 2: PDFium asks per request (`MapFont`) and does not list the fonts.
                version: 2,
                Release: Some(release),
                EnumFonts: None,
                MapFont: Some(map_font),
                GetFont: None,
                GetFontData: Some(get_font_data),
                GetFaceName: Some(get_face_name),
                GetFontCharset: Some(get_font_charset),
                DeleteFont: Some(delete_font),
            },
            fonts,
            handles: HashMap::new(),
            next: 1,
        })
    }

    /// The pointer to give to `FPDF_SetSystemFontInfo`. The structure must stay where it is for as
    /// long as PDFium lives.
    pub fn as_ptr(&mut self) -> *mut FPDF_SYSFONTINFO {
        &mut self.base
    }
}

/// # Safety
/// `this` is the pointer PDFium was given by [`SysFontInfo::as_ptr`].
unsafe fn info<'a>(this: *mut FPDF_SYSFONTINFO) -> Option<&'a mut SysFontInfo> {
    // SAFETY: `base` is the first field of a `repr(C)` structure, and PDFium passes back the
    // pointer it was given.
    unsafe { (this as *mut SysFontInfo).as_mut() }
}

unsafe extern "C" fn release(this: *mut FPDF_SYSFONTINFO) {
    // SAFETY: see `info`.
    if let Some(info) = unsafe { info(this) } {
        info.handles.clear();
    }
}

unsafe extern "C" fn map_font(
    this: *mut FPDF_SYSFONTINFO,
    _weight: c_int,
    italic: FPDF_BOOL,
    charset: c_int,
    _pitch_family: c_int,
    face: *const c_char,
    _exact: *mut FPDF_BOOL,
) -> *mut c_void {
    // A panic must not unwind into PDFium: it means "no font".
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: see `info`.
        let info = unsafe { info(this) }?;
        let raw: &[u8] = if face.is_null() {
            b""
        } else {
            // SAFETY: PDFium passes a string with a closing zero.
            unsafe { CStr::from_ptr(face) }.to_bytes()
        };
        // Bytes that are not UTF-8 (a name in GBK or Shift-JIS) become replacement characters:
        // such a name tells nothing, the character set does.
        let name = String::from_utf8_lossy(raw);
        let served = info.fonts.serve(&name, italic != 0, charset)?;
        let key = info.next;
        info.next += 1;
        let mut face = raw.to_vec();
        face.push(0);
        info.handles.insert(
            key,
            Handle {
                data: served.data,
                face,
                charset: served.charset,
            },
        );
        Some(key as *mut c_void)
    }))
    .ok()
    .flatten()
    .unwrap_or(std::ptr::null_mut())
}

unsafe extern "C" fn get_font_data(
    this: *mut FPDF_SYSFONTINFO,
    font: *mut c_void,
    table: c_uint,
    buffer: *mut c_uchar,
    buf_size: c_ulong,
) -> c_ulong {
    // Only the whole file (table 0) is offered, not single tables. A collection is therefore used
    // from its first font.
    if table != 0 {
        return 0;
    }
    // SAFETY: see `info`.
    let Some(info) = (unsafe { info(this) }) else {
        return 0;
    };
    let Some(handle) = info.handles.get(&(font as usize)) else {
        return 0;
    };
    let length = handle.data.len();
    if !buffer.is_null() && (buf_size as usize) >= length {
        // SAFETY: PDFium says the buffer holds `buf_size` bytes, and the data is `length` of them.
        unsafe { std::ptr::copy_nonoverlapping(handle.data.as_ptr(), buffer, length) };
    }
    length as c_ulong
}

unsafe extern "C" fn get_face_name(
    this: *mut FPDF_SYSFONTINFO,
    font: *mut c_void,
    buffer: *mut c_char,
    buf_size: c_ulong,
) -> c_ulong {
    // SAFETY: see `info`.
    let Some(info) = (unsafe { info(this) }) else {
        return 0;
    };
    let Some(handle) = info.handles.get(&(font as usize)) else {
        return 0;
    };
    let length = handle.face.len();
    if !buffer.is_null() && (buf_size as usize) >= length {
        // SAFETY: as in `get_font_data`.
        unsafe { std::ptr::copy_nonoverlapping(handle.face.as_ptr(), buffer as *mut u8, length) };
    }
    length as c_ulong
}

unsafe extern "C" fn get_font_charset(this: *mut FPDF_SYSFONTINFO, font: *mut c_void) -> c_int {
    // SAFETY: see `info`.
    unsafe { info(this) }
        .and_then(|info| info.handles.get(&(font as usize)))
        .map_or(1, |handle| handle.charset)
}

unsafe extern "C" fn delete_font(this: *mut FPDF_SYSFONTINFO, font: *mut c_void) {
    // SAFETY: see `info`.
    if let Some(info) = unsafe { info(this) } {
        info.handles.remove(&(font as usize));
    }
}
