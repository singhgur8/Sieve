use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Staging area for bundle inputs (under the manifest's `target/`, gitignored). Paths are
/// referenced statically from `tauri.conf.json` (`bundle.macOS.frameworks`, `bundle.resources`).
const STAGE_DIR: &str = "target/sieve-stage";
/// ONNX models shipped inside the app (culling engine ~27 MB + face identity 174 MB). The
/// segmentation models (~560 MB) are downloaded on first use into the app data dir (see
/// `model_fetch.rs`).
const BUNDLED_MODELS: &[&str] = &[
    "det_10g.onnx",
    "2d106det.onnx",
    "open_closed_eye.onnx",
    "face_landmarks_detector_1x3x256x256.onnx",
    "w600k_r50.onnx",
];

fn main() {
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let stage = manifest_dir.join(STAGE_DIR);

    // LibRaw (thread-safe build) for RAW decoding / thumbnail fallback.
    let libraw =
        find_system_lib("raw_r", "LIBRAW_DIR", "libraw_r", &["/opt/homebrew/opt/libraw", "/usr/local/opt/libraw"]);
    // TurboJPEG 3 API from libjpeg-turbo (a dependency of Homebrew's libraw).
    let turbojpeg = find_system_lib(
        "turbojpeg",
        "TURBOJPEG_DIR",
        "libturbojpeg",
        &["/opt/homebrew/opt/jpeg-turbo", "/usr/local/opt/jpeg-turbo"],
    );

    // Tiny C shim for LibRaw fields without C-API accessors (develop source settings,
    // colour matrices), compiled against the same install's headers.
    println!("cargo:rerun-if-changed=native/libraw_shim.c");
    let mut shim = cc::Build::new();
    shim.file("native/libraw_shim.c").warnings(false);
    if let Some(include) = libraw.as_ref().and_then(|lib| lib.parent()).map(|p| p.join("include")) {
        shim.include(include);
    }
    shim.compile("sieve_libraw_shim");

    if macos {
        // Self-contained bundle: copy the Homebrew dylibs (and their non-system transitive
        // deps) into the stage, rewrite install names to `@rpath/<name>`, ad-hoc re-sign, and
        // link against the staged copies. tauri-build copies them into `target/Frameworks` and
        // adds `-rpath @executable_path/../Frameworks`; the bundler ships them in
        // `Contents/Frameworks`.
        let libs: Vec<(&str, PathBuf)> = [("raw_r", libraw), ("turbojpeg", turbojpeg)]
            .into_iter()
            .filter_map(|(name, dir)| dir.map(|d| (name, d.join(format!("lib{name}.dylib")))))
            .collect();
        if libs.len() == 2 {
            let frameworks = stage.join("Frameworks");
            let staged = stage_dylibs(&libs, &frameworks);
            check_frameworks_config(&manifest_dir, &staged);
            println!("cargo:rustc-link-search=native={}", frameworks.display());
            // Test / example binaries live one level deeper (`target/<profile>/{deps,examples}`),
            // so `@executable_path/../Frameworks` resolves to `target/<profile>/Frameworks`.
            if let Some(profile_dir) = profile_dir() {
                copy_all(&frameworks, &profile_dir.join("Frameworks"), &staged);
            }
        } else {
            println!("cargo:warning=LibRaw / libjpeg-turbo not found: `brew install libraw`");
        }
    } else {
        for dir in [libraw, turbojpeg].into_iter().flatten() {
            println!("cargo:rustc-link-search=native={}", dir.display());
        }
    }
    println!("cargo:rustc-link-lib=dylib=raw_r");
    println!("cargo:rustc-link-lib=dylib=turbojpeg");

    stage_models(&manifest_dir, &stage.join("models"));
    tauri_build::build()
}

/// Finds `lib<name>` in `$<env_dir>/lib`, `pkg-config <pc>`, then the given Homebrew prefixes.
/// Install with `brew install libraw` (brings jpeg-turbo).
fn find_system_lib(name: &str, env_dir: &str, pc: &str, prefixes: &[&str]) -> Option<PathBuf> {
    println!("cargo:rerun-if-env-changed={env_dir}");
    let has_lib =
        |lib: &Path| lib.join(format!("lib{name}.dylib")).exists() || lib.join(format!("lib{name}.so")).exists();
    let from_env = std::env::var_os(env_dir).map(|p| PathBuf::from(p).join("lib"));
    let from_brew = || prefixes.iter().map(|p| Path::new(p).join("lib")).find(|lib| has_lib(lib));
    let dir = from_env.or_else(|| pkg_config_libdir(pc)).or_else(from_brew);
    if dir.is_none() {
        println!("cargo:warning=lib{name} not found (set {env_dir} or `brew install libraw`)");
    }
    dir
}

fn pkg_config_libdir(name: &str) -> Option<PathBuf> {
    let out = Command::new("pkg-config").args(["--variable=libdir", name]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!dir.is_empty()).then(|| PathBuf::from(dir))
}

/// `<target>/<profile>` from `OUT_DIR` (`<target>/<profile>/build/<pkg>/out`).
fn profile_dir() -> Option<PathBuf> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR")?);
    out.ancestors().nth(3).map(Path::to_path_buf)
}

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap_or_else(|e| panic!("failed to run {cmd:?}: {e}"));
    if !out.status.success() {
        panic!("{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Non-system load commands of a Mach-O file (`otool -L`), excluding its own install id.
fn dylib_deps(file: &Path) -> Vec<String> {
    let id = run(Command::new("otool").arg("-D").arg(file)).lines().nth(1).map(str::trim).map(str::to_owned);
    run(Command::new("otool").arg("-L").arg(file))
        .lines()
        .skip(1)
        .filter_map(|l| l.trim().split(" (").next().map(str::to_owned))
        .filter(|p| Some(p) != id.as_ref())
        .filter(|p| !p.starts_with("/usr/lib/") && !p.starts_with("/System/") && !p.starts_with('@'))
        .collect()
}

/// Stages `libs` (link name, path) and their transitive non-system deps into `dest` with
/// `@rpath/<name>` install names. Returns the staged file names.
fn stage_dylibs(libs: &[(&str, PathBuf)], dest: &Path) -> Vec<String> {
    fs::create_dir_all(dest).expect("create frameworks stage dir");
    // Referenced path (or root path) -> staged file name.
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for (name, path) in libs {
        println!("cargo:rerun-if-changed={}", path.display());
        names.insert(path.display().to_string(), format!("lib{name}.dylib"));
        queue.push_back(path.display().to_string());
    }
    let mut deps_of: BTreeMap<String, Vec<String>> = BTreeMap::new();
    while let Some(src) = queue.pop_front() {
        if deps_of.contains_key(&src) {
            continue;
        }
        let deps = dylib_deps(Path::new(&src));
        for dep in &deps {
            if !names.contains_key(dep) {
                let base = Path::new(dep).file_name().expect("dylib name").to_string_lossy().into_owned();
                names.insert(dep.clone(), base);
                queue.push_back(dep.clone());
            }
        }
        deps_of.insert(src, deps);
    }
    let mut staged = Vec::new();
    for (src, deps) in &deps_of {
        let name = &names[src];
        let real = fs::canonicalize(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        let out = dest.join(name);
        let _ = fs::remove_file(&out);
        fs::copy(&real, &out).unwrap_or_else(|e| panic!("copy {} -> {}: {e}", real.display(), out.display()));
        let mut perms = fs::metadata(&out).expect("staged dylib").permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&out, perms).expect("chmod staged dylib");
        let mut cmd = Command::new("install_name_tool");
        cmd.arg("-id").arg(format!("@rpath/{name}"));
        for dep in deps {
            cmd.arg("-change").arg(dep).arg(format!("@rpath/{}", names[dep]));
        }
        run(cmd.arg(&out));
        // install_name_tool invalidates the signature; arm64 refuses to load unsigned code.
        run(Command::new("codesign").args(["--force", "--sign", "-"]).arg(&out));
        staged.push(name.clone());
    }
    staged.sort();
    staged
}

/// `tauri.conf.json` lists the staged dylibs statically; fail loudly if Homebrew's dependency
/// graph changed (e.g. a new transitive dylib) so the bundle can't silently miss one.
fn check_frameworks_config(manifest_dir: &Path, staged: &[String]) {
    let conf_path = manifest_dir.join("tauri.conf.json");
    println!("cargo:rerun-if-changed={}", conf_path.display());
    let conf: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&conf_path).expect("read tauri.conf.json")).expect("parse conf");
    let prefix = format!("{STAGE_DIR}/Frameworks/");
    let mut listed: Vec<String> = conf["bundle"]["macOS"]["frameworks"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()?.strip_prefix(&prefix).map(str::to_owned)).collect())
        .unwrap_or_default();
    listed.sort();
    if listed != staged {
        panic!(
            "tauri.conf.json bundle.macOS.frameworks {listed:?} does not match the staged dylibs {staged:?}; \
             update the list to \"{prefix}<name>\" for each staged file"
        );
    }
}

fn copy_all(from: &Path, to: &Path, names: &[String]) {
    fs::create_dir_all(to).expect("create profile Frameworks dir");
    for name in names {
        let dst = to.join(name);
        let _ = fs::remove_file(&dst);
        fs::copy(from.join(name), &dst).unwrap_or_else(|e| panic!("copy {name}: {e}"));
    }
}

/// Stages the bundled (culling) models for `bundle.resources`. Always creates the directory so
/// builds without fetched models still work (the app reports missing models at runtime); a
/// `pnpm tauri build` fetches them first (`beforeBuildCommand`).
fn stage_models(manifest_dir: &Path, dest: &Path) {
    let src = manifest_dir.join("models");
    println!("cargo:rerun-if-changed={}", src.display());
    fs::create_dir_all(dest).expect("create models stage dir");
    let mut files: Vec<&str> = BUNDLED_MODELS.to_vec();
    files.push("README.md");
    for name in files {
        let from = src.join(name);
        let to = dest.join(name);
        println!("cargo:rerun-if-changed={}", from.display());
        let _ = fs::remove_file(&to);
        if from.exists() {
            if fs::hard_link(&from, &to).is_err() {
                fs::copy(&from, &to).unwrap_or_else(|e| panic!("stage {name}: {e}"));
            }
        } else if name.ends_with(".onnx") {
            let msg = format!("model {name} missing: run scripts/fetch-models.sh --culling");
            if std::env::var("PROFILE").as_deref() == Ok("release") {
                if std::env::var_os("TAURI_ENV_PLATFORM").is_some() {
                    panic!("{msg}");
                }
                println!("cargo:warning={msg}");
            }
        }
    }
}
