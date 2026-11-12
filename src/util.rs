use std::sync::OnceLock;
use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SDECIMAL, LOCALE_STHOUSAND};

use crate::lang::t;

// a NUL-terminated UTF-16 copy of a string, for the Win32 calls
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// the separators of the user's number format, read once
struct Seps {
    thousand: String,
    decimal: String,
}

fn seps() -> &'static Seps {
    static S: OnceLock<Seps> = OnceLock::new();
    S.get_or_init(|| {
        let read = |kind, default: &str| {
            let mut buf = [0u16; 8];
            let n = unsafe { GetLocaleInfoEx(std::ptr::null(), kind, buf.as_mut_ptr(), buf.len() as i32) };
            if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { default.into() }
        };
        Seps { thousand: read(LOCALE_STHOUSAND, ","), decimal: read(LOCALE_SDECIMAL, ".") }
    })
}

// a whole number with thousands separators: 1,253,647
pub fn num(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out += &seps().thousand;
        }
        out.push(c);
    }
    out
}

// a fraction with the given number of digits; `trim` drops trailing zeros (12.5, not 12.50)
pub fn dec(v: f64, digits: usize, trim: bool) -> String {
    let mut s = format!("{v:.digits$}");
    if trim && s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    s.replace('.', &seps().decimal)
}

// a size in the largest unit that keeps it at 1 or more: "12.5 GB"; bytes as a whole number
pub fn size(b: u64) -> String {
    const UNITS: [&str; 5] = ["unit.bytes", "unit.kb", "unit.mb", "unit.gb", "unit.tb"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{} {}", num(b), t(UNITS[0])) } else { format!("{} {}", dec(v, 2, true), t(UNITS[i])) }
}

// a share in percent with one digit, without the % sign
pub fn pct(part: u64, whole: u64) -> String {
    if whole == 0 { String::new() } else { dec(100.0 * part as f64 / whole as f64, 1, false) }
}
