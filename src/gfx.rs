// Drawing helpers: GDI+ for the anti-aliased shapes, GDI for ClearType text.
use std::ptr::null_mut;
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::{Gdi::*, GdiPlus::*},
};

use crate::util::wide;

pub fn startup() {
    let mut token = 0usize;
    unsafe {
        let mut input: GdiplusStartupInput = std::mem::zeroed();
        input.GdiplusVersion = 1;
        GdiplusStartup(&mut token, &input, null_mut());
    }
}

// 0xRRGGBB to what GDI wants (0x00BBGGRR)
pub fn cref(c: u32) -> u32 {
    (c >> 16 & 0xff) | (c & 0xff00) | (c & 0xff) << 16
}

fn argb(c: u32) -> u32 {
    0xFF00_0000 | c
}

pub fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn same(a: &RECT, b: &RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

pub fn contains(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

pub struct Path(pub *mut GpPath);

impl Path {
    pub fn new() -> Path {
        let mut p = null_mut();
        unsafe { GdipCreatePath(FillModeAlternate, &mut p) };
        Path(p)
    }

    // a ring segment between two radii, angles clockwise from 3 o'clock
    pub fn ring(cx: f32, cy: f32, inner: f32, outer: f32, start: f32, sweep: f32) -> Path {
        let p = Path::new();
        unsafe {
            GdipAddPathArc(p.0, cx - outer, cy - outer, 2.0 * outer, 2.0 * outer, start, sweep);
            GdipAddPathArc(p.0, cx - inner, cy - inner, 2.0 * inner, 2.0 * inner, start + sweep, -sweep);
            GdipClosePathFigure(p.0);
        }
        p
    }

    pub fn pie(cx: f32, cy: f32, r: f32, start: f32, sweep: f32) -> Path {
        let p = Path::new();
        unsafe { GdipAddPathPie(p.0, cx - r, cy - r, 2.0 * r, 2.0 * r, start, sweep) };
        p
    }

    pub fn round(r: &RECT, radius: f32) -> Path {
        let p = Path::new();
        let (x, y, w, h, d) = (r.left as f32 + 0.5, r.top as f32 + 0.5, (r.right - r.left) as f32 - 1.0, (r.bottom - r.top) as f32 - 1.0, radius * 2.0);
        unsafe {
            GdipAddPathArc(p.0, x, y, d, d, 180.0, 90.0);
            GdipAddPathArc(p.0, x + w - d, y, d, d, 270.0, 90.0);
            GdipAddPathArc(p.0, x + w - d, y + h - d, d, d, 0.0, 90.0);
            GdipAddPathArc(p.0, x, y + h - d, d, d, 90.0, 90.0);
            GdipClosePathFigure(p.0);
        }
        p
    }
}

impl Drop for Path {
    fn drop(&mut self) {
        unsafe { GdipDeletePath(self.0) };
    }
}

pub struct Brush(*mut GpBrush);

impl Brush {
    pub fn solid(c: u32) -> Brush {
        let mut b = null_mut();
        unsafe { GdipCreateSolidFill(argb(c), &mut b) };
        Brush(b.cast())
    }

    // the diagonal hatch of free space
    pub fn hatch(line: u32, back: u32) -> Brush {
        let mut b = null_mut();
        unsafe { GdipCreateHatchBrush(HatchStyleLightUpwardDiagonal, argb(line), argb(back), &mut b) };
        Brush(b.cast())
    }
}

impl Drop for Brush {
    fn drop(&mut self) {
        unsafe { GdipDeleteBrush(self.0) };
    }
}

// a GDI+ surface over a device context, anti-aliased
pub struct G(*mut GpGraphics);

impl G {
    pub fn new(hdc: HDC) -> G {
        let mut g = null_mut();
        unsafe {
            GdipCreateFromHDC(hdc, &mut g);
            GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        }
        G(g)
    }

    pub fn fill(&self, path: &Path, brush: &Brush) {
        unsafe { GdipFillPath(self.0, brush.0, path.0) };
    }

    pub fn outline(&self, path: &Path, c: u32) {
        let mut pen = null_mut();
        unsafe {
            GdipCreatePen1(argb(c), 1.0, UnitPixel, &mut pen);
            GdipDrawPath(self.0, pen, path.0);
            GdipDeletePen(pen);
        }
    }

    pub fn round(&self, r: &RECT, radius: f32, c: u32) {
        self.fill(&Path::round(r, radius), &Brush::solid(c));
    }

    pub fn disc(&self, cx: f32, cy: f32, r: f32, c: u32) {
        unsafe { GdipFillEllipse(self.0, Brush::solid(c).0, cx - r, cy - r, 2.0 * r, 2.0 * r) };
    }
}

impl Drop for G {
    fn drop(&mut self) {
        unsafe { GdipDeleteGraphics(self.0) };
    }
}

pub fn font(face: &str, px: i32, bold: bool) -> HFONT {
    let face = wide(face);
    unsafe {
        CreateFontW(-px, 0, 0, 0, if bold { 600 } else { 400 }, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32, CLEARTYPE_QUALITY as u32, 0, face.as_ptr())
    }
}

pub fn text(hdc: HDC, s: &str, r: &RECT, c: u32, font: HFONT, flags: DRAW_TEXT_FORMAT) {
    // an empty Vec points nowhere, and DrawTextW reads it even for no characters
    if s.is_empty() {
        return;
    }
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut r = *r;
    unsafe {
        SelectObject(hdc, font);
        SetTextColor(hdc, cref(c));
        SetBkMode(hdc, TRANSPARENT as _);
        DrawTextW(hdc, w.as_ptr(), w.len() as i32, &mut r, flags | DT_NOPREFIX);
    }
}

pub fn text_width(hdc: HDC, s: &str, font: HFONT) -> i32 {
    if s.is_empty() {
        return 0;
    }
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe {
        SelectObject(hdc, font);
        DrawTextW(hdc, w.as_ptr(), w.len() as i32, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
    }
    r.right
}

pub fn line_height(hdc: HDC, font: HFONT) -> i32 {
    unsafe {
        SelectObject(hdc, font);
        let mut tm: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(hdc, &mut tm);
        tm.tmHeight
    }
}

pub fn fill_rect(hdc: HDC, r: &RECT, c: u32) {
    unsafe {
        let b = CreateSolidBrush(cref(c));
        FillRect(hdc, r, b);
        DeleteObject(b);
    }
}

// one line, vertically centred
pub const LINE: DRAW_TEXT_FORMAT = DT_SINGLELINE | DT_VCENTER;
