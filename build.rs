use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    // Common Controls 6 (themed tree view, tooltips, dark scrollbars) needs a manifest;
    // the linker writes one into the exe.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' \
         version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );

    // the icon: rc.exe from the Windows SDK compiles it, the linker puts it into the exe
    println!("cargo:rerun-if-changed=res");
    let out = env::var("OUT_DIR").unwrap();
    match rc_exe() {
        Some(rc) => {
            let res = Path::new(&out).join("app.res");
            let ok = Command::new(rc).args(["/nologo", "/fo"]).arg(&res).arg("res/app.rc").status().is_ok_and(|s| s.success());
            assert!(ok, "rc.exe could not compile res/app.rc");
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
        None => println!("cargo:warning=rc.exe not found (Windows SDK), the exe gets no icon"),
    }

    // the language files go next to the exe, where they can be edited
    println!("cargo:rerun-if-changed=lang");
    let target = Path::new(&out).ancestors().nth(3).unwrap().join("lang");
    fs::create_dir_all(&target).unwrap();
    for f in fs::read_dir("lang").unwrap().flatten() {
        fs::copy(f.path(), target.join(f.file_name())).unwrap();
    }
}

// rc.exe of the newest Windows SDK, unless it is on the PATH already
fn rc_exe() -> Option<PathBuf> {
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some("rc.exe".into());
    }
    let bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = fs::read_dir(bin).ok()?.flatten().map(|e| e.path().join(r"x64\rc.exe")).filter(|p| p.exists()).collect();
    versions.sort();
    versions.pop()
}
