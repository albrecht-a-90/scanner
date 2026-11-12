// User choices kept between runs in the registry, under HKEY_CURRENT_USER\Software\Scanner
// (the same values the C# version used).
use std::ptr::null_mut;
use windows_sys::Win32::System::Registry::*;

use crate::{lang, theme::Mode, util::wide};

const KEY: &str = "Software\\Scanner";

pub struct Settings {
    pub theme: Mode,
    pub rings: u32, // 4 (minimal), or 0 for all levels
    pub lang: String,
    pub free: bool,
    pub panel: bool,   // the folder panel is off until it is switched on
    pub by_size: bool, // false: folders first, then files, by name
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Settings { theme: Mode::System, rings: 4, lang: lang::DEFAULT.into(), free: true, panel: false, by_size: false };
        if let Some(v) = get_str("Theme") {
            s.theme = match v.to_lowercase().as_str() {
                "light" => Mode::Light,
                "dark" => Mode::Dark,
                _ => Mode::System,
            };
        }
        if let Some(v) = get_dword("Rings") {
            s.rings = if v == 0 { 0 } else { 4 };
        }
        if let Some(v) = get_dword("ShowFree") {
            s.free = v != 0;
        }
        if let Some(v) = get_dword("FolderPanel") {
            s.panel = v != 0;
        }
        if let Some(v) = get_str("Order") {
            s.by_size = v == "size";
        }
        if let Some(v) = get_str("Language").filter(|v| !v.is_empty()) {
            s.lang = v;
        }
        s
    }

    pub fn save(&self) {
        let theme = match self.theme {
            Mode::System => "System",
            Mode::Light => "Light",
            Mode::Dark => "Dark",
        };
        unsafe {
            let mut key = null_mut();
            let path = wide(KEY);
            if RegCreateKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, std::ptr::null(), 0, KEY_WRITE, std::ptr::null(), &mut key, null_mut()) != 0 {
                return;
            }
            set_str(key, "Theme", theme);
            set_dword(key, "Rings", self.rings);
            set_dword(key, "ShowFree", self.free as u32);
            set_dword(key, "FolderPanel", self.panel as u32);
            set_str(key, "Order", if self.by_size { "size" } else { "name" });
            set_str(key, "Language", &self.lang);
            RegCloseKey(key);
        }
    }
}

fn get_str(name: &str) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut len = (buf.len() * 2) as u32;
    let (key, name) = (wide(KEY), wide(name));
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut len)
    } == 0;
    ok.then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
}

fn get_dword(name: &str) -> Option<u32> {
    let mut v = 0u32;
    let mut len = 4u32;
    let (key, name) = (wide(KEY), wide(name));
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, null_mut(), (&mut v as *mut u32).cast(), &mut len)
    } == 0;
    ok.then_some(v)
}

unsafe fn set_str(key: HKEY, name: &str, v: &str) {
    let (name, v) = (wide(name), wide(v));
    unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, v.as_ptr().cast(), (v.len() * 2) as u32) };
}

unsafe fn set_dword(key: HKEY, name: &str, v: u32) {
    let name = wide(name);
    unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_DWORD, (&v as *const u32).cast(), 4) };
}
