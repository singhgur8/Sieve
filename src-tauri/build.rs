use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // LibRaw (thread-safe build) for RAW decoding / thumbnail fallback.
    link_system_lib("raw_r", "LIBRAW_DIR", "libraw_r", &["/opt/homebrew/opt/libraw", "/usr/local/opt/libraw"]);
    // TurboJPEG 3 API from libjpeg-turbo (a dependency of Homebrew's libraw).
    link_system_lib(
        "turbojpeg",
        "TURBOJPEG_DIR",
        "libturbojpeg",
        &["/opt/homebrew/opt/jpeg-turbo", "/usr/local/opt/jpeg-turbo"],
    );
    tauri_build::build()
}

/// Links `lib<name>` dynamically, searching `$<env_dir>/lib`, `pkg-config <pc>`, then
/// the given Homebrew prefixes. Install with `brew install libraw` (brings jpeg-turbo).
/// Homebrew dylibs carry absolute install names, so no rpath is needed at runtime.
fn link_system_lib(name: &str, env_dir: &str, pc: &str, prefixes: &[&str]) {
    println!("cargo:rerun-if-env-changed={env_dir}");
    let has_lib =
        |lib: &Path| lib.join(format!("lib{name}.dylib")).exists() || lib.join(format!("lib{name}.so")).exists();
    let from_env = std::env::var_os(env_dir).map(|p| PathBuf::from(p).join("lib"));
    let from_brew = || prefixes.iter().map(|p| Path::new(p).join("lib")).find(|lib| has_lib(lib));
    let dir = from_env.or_else(|| pkg_config_libdir(pc)).or_else(from_brew);
    match dir {
        Some(dir) => println!("cargo:rustc-link-search=native={}", dir.display()),
        None => println!("cargo:warning=lib{name} not found (set {env_dir} or `brew install libraw`)"),
    }
    println!("cargo:rustc-link-lib=dylib={name}");
}

fn pkg_config_libdir(name: &str) -> Option<PathBuf> {
    let out = Command::new("pkg-config").args(["--variable=libdir", name]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!dir.is_empty()).then(|| PathBuf::from(dir))
}
