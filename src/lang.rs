// Texts of the interface, read from the "lang" folder next to the exe: one "key = text" file
// per language. English is also built into the exe: a file that is missing or is not a
// language file (no "language" line) gives English, and so does a key missing from a file.
use std::{cell::RefCell, collections::HashMap, fs, path::PathBuf};

pub const DEFAULT: &str = "en";
const BUILT_IN: &str = include_str!("../lang/en.txt");

type Texts = HashMap<String, String>;

struct State {
    english: Texts,
    current: Texts,
    code: String,
}

thread_local! {
    static LANG: RefCell<State> = RefCell::new(State { english: parse(BUILT_IN), current: Texts::new(), code: DEFAULT.into() });
}

fn dir() -> PathBuf {
    std::env::current_exe().map(|p| p.with_file_name("lang")).unwrap_or_default()
}

pub fn t(key: &str) -> String {
    LANG.with(|l| {
        let l = l.borrow();
        l.current.get(key).or_else(|| l.english.get(key)).cloned().unwrap_or_else(|| key.to_string())
    })
}

// a text with {0}, {1}, ... filled in; a broken placeholder simply stays as it is
pub fn tf(key: &str, args: &[&str]) -> String {
    let mut s = t(key);
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), a);
    }
    s
}

// the language in use, which is English when the chosen file could not be used
pub fn code() -> String {
    LANG.with(|l| l.borrow().code.clone())
}

// the usable languages in the folder as (file name, name for the menu); English always
pub fn available() -> Vec<(String, String)> {
    let english = LANG.with(|l| l.borrow().english.get("language").cloned()).unwrap_or_else(|| "English".into());
    let mut list = vec![(DEFAULT.to_string(), english)];
    for f in fs::read_dir(dir()).into_iter().flatten().flatten() {
        let path = f.path();
        let code = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("txt")) && !code.eq_ignore_ascii_case(DEFAULT) {
            if let Some(name) = read(&path).remove("language") {
                list.push((code, name));
            }
        }
    }
    list
}

// en.txt next to the exe may be edited too and then wins over the built-in copy
pub fn load(code: &str) {
    let texts = read(&dir().join(format!("{code}.txt")));
    let ok = texts.contains_key("language");
    LANG.with(|l| {
        let mut l = l.borrow_mut();
        l.current = if ok { texts } else { Texts::new() };
        l.code = if ok { code.to_string() } else { DEFAULT.to_string() };
    });
}

fn read(path: &PathBuf) -> Texts {
    fs::read(path).map(|b| parse(&String::from_utf8_lossy(&b))).unwrap_or_default()
}

fn parse(src: &str) -> Texts {
    src.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().replace("\\n", "\n")))
        .collect()
}
