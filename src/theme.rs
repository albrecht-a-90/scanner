// Colours everything is painted with, as 0xRRGGBB. System follows the Windows app mode and
// accent, read from the keys the Settings app writes.
use std::ptr::null_mut;
use windows_sys::Win32::System::Registry::*;

use crate::util::wide;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    System,
    Light,
    Dark,
}

#[derive(Clone, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub back: u32,
    pub text: u32,
    pub muted: u32,
    pub border: u32,
    pub center: u32,
    pub center_hot: u32,
    pub accent: u32,
    // list rows and icon buttons: under the mouse, selected; disabled glyphs
    pub row_hover: u32,
    pub select: u32,
    pub disabled: u32,
    // chart: the palette the first-level branches take in turn, grey for what is too small
    // to draw, "not scanned" and the hatch of free space
    pub slots: [u32; 12],
    pub other: u32,
    pub sys: u32,
    pub hatch_back: u32,
    pub hatch_line: u32,
}

fn light(accent: Option<u32>) -> Theme {
    Theme {
        dark: false,
        back: 0xF0F0F0,
        text: 0x000000,
        muted: 0x6D6D6D,
        border: 0xA0A0A0,
        center: 0xFFFFFF,
        center_hot: 0xE1E1DE,
        accent: accent.unwrap_or(0x0067C0),
        row_hover: 0xE1E1E1,
        select: 0xD2E1F0,
        disabled: 0xAAAAAA,
        slots: [0x2a78d6, 0xeb6834, 0x1baf7a, 0xeda100, 0xe87ba4, 0x8b5cf6, 0x14b8a6, 0xdc2626, 0x84cc16, 0x4f46e5, 0x0891b2, 0xb45309],
        other: 0xc4c3bc,
        sys: 0x8a8983,
        hatch_back: 0xf1f0ec,
        hatch_line: 0xc9c8c1,
    }
}

fn dark(accent: Option<u32>) -> Theme {
    Theme {
        dark: true,
        back: 0x202020,
        text: 0xF0F0F0,
        muted: 0x969696,
        border: 0x464646,
        center: 0x2D2D2D,
        center_hot: 0x3E3E3C,
        accent: accent.unwrap_or(0x4CC2FF),
        row_hover: 0x323232,
        select: 0x3E3E3E,
        disabled: 0x555555,
        slots: [0x3987e5, 0xd95926, 0x199e70, 0xc98500, 0xd55181, 0x8b6cf0, 0x22a99a, 0xd94040, 0x7cb518, 0x6366f1, 0x1ba3c4, 0xb07d2a],
        other: 0x55554f,
        sys: 0x7a7973,
        hatch_back: 0x2a2a27,
        hatch_line: 0x4a4a44,
    }
}

// Windows 11 uses Dark1 of the accent palette in light mode and Light2 in dark mode; an
// accent that white (light) or black (dark) text would be unreadable on is ignored.
pub fn build(mode: Mode) -> Theme {
    let is_light = match mode {
        Mode::Light => true,
        Mode::Dark => false,
        Mode::System => read_dword("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", "AppsUseLightTheme") != Some(0),
    };
    let accent = if mode == Mode::System { read_palette().map(|p| p[if is_light { 4 } else { 1 }]) } else { None };
    let readable = accent.filter(|&a| contrast(a, if is_light { 0xFFFFFF } else { 0 }) >= 4.5);
    if is_light { light(readable) } else { dark(readable) }
}

fn read_dword(key: &str, name: &str) -> Option<u32> {
    let (mut v, mut len) = (0u32, 4u32);
    let (key, name) = (wide(key), wide(name));
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, null_mut(), (&mut v as *mut u32).cast(), &mut len)
    } == 0;
    ok.then_some(v)
}

// AccentPalette holds 8 RGBA entries: Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3, (unused)
fn read_palette() -> Option<[u32; 8]> {
    let mut b = [0u8; 32];
    let mut len = 32u32;
    let (key, name) = (wide("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"), wide("AccentPalette"));
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_BINARY, null_mut(), b.as_mut_ptr().cast(), &mut len)
    } == 0;
    (ok && len >= 32).then(|| std::array::from_fn(|i| (b[i * 4] as u32) << 16 | (b[i * 4 + 1] as u32) << 8 | b[i * 4 + 2] as u32))
}

// WCAG contrast ratio
fn contrast(a: u32, b: u32) -> f64 {
    let ch = |v: u32| {
        let c = (v & 0xff) as f64 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let lum = |c: u32| 0.2126 * ch(c >> 16) + 0.7152 * ch(c >> 8) + 0.0722 * ch(c);
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

// `top` over `bottom` at the given opacity
pub fn blend(top: u32, bottom: u32, alpha: f64) -> u32 {
    let mix = |s: u32| {
        let (t, b) = ((top >> s & 0xff) as f64, (bottom >> s & 0xff) as f64);
        ((t * alpha + b * (1.0 - alpha)).round() as u32) << s
    };
    mix(16) | mix(8) | mix(0)
}
