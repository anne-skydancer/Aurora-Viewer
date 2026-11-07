//! Build script: tells the viewer which cargo profile built it, for the
//! window title ("Aurora Viewer <version> (Dev)"): cargo only exposes `debug` /
//! `release` (PROFILE), while the profile's target directory names the
//! custom ones (`debugging`, `ci`). On Windows it also embeds the resources
//! of aurora-viewer.exe: its icon and its version block.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // OUT_DIR = <target>/<profile dir>/build/<crate>-<hash>/out
    let out = std::env::var("OUT_DIR").unwrap_or_default();
    let parts: Vec<String> = Path::new(&out)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let dir = parts
        .iter()
        .rposition(|p| p == "build")
        .and_then(|i| i.checked_sub(1))
        .map(|i| parts[i].clone())
        .unwrap_or_default();
    println!("cargo:rustc-env=AURORA_BUILD_PROFILE={dir}");
    dictionaries();
    settings_docs();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
        // The hybrid-GPU hints of main.rs (gpu_hints) in the exe's export
        // table, where the NVIDIA and AMD drivers look for them: a Rust
        // executable exports nothing on its own
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            for symbol in ["NvOptimusEnablement", "AmdPowerXpressRequestHighPerformance"] {
                println!("cargo:rustc-link-arg-bin=aurora-viewer=/EXPORT:{symbol},DATA");
            }
        }
    }
}

/// The spelling dictionaries (assets/dictionaries/<name>/<name>.aff and
/// .dic, kept as text in the repository) gzipped into OUT_DIR, where
/// spelling/dictionaries.rs embeds them: 22 MB of words take about 5 MB of
/// the exe, inflated on the spelling thread only for the languages in use.
fn dictionaries() {
    use std::io::Write as _;
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR");
    let source = Path::new(&manifest).join("assets/dictionaries");
    println!("cargo:rerun-if-changed={}", source.display());
    let out = Path::new(&std::env::var("OUT_DIR").expect("Cargo supplies OUT_DIR")).join("dictionaries");
    std::fs::create_dir_all(&out).expect("the build folder is writable");
    let folders = std::fs::read_dir(&source).expect("assets/dictionaries is in the repository");
    for folder in folders.flatten().filter(|entry| entry.path().is_dir()) {
        for file in std::fs::read_dir(folder.path()).expect("a dictionary folder is readable").flatten() {
            let path = file.path();
            if !matches!(path.extension().and_then(|e| e.to_str()), Some("aff" | "dic")) {
                continue;
            }
            let bytes = std::fs::read(&path).expect("a dictionary file is readable");
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            gz.write_all(&bytes).expect("gzip in memory");
            let name = format!("{}.gz", path.file_name().and_then(|n| n.to_str()).expect("an ASCII file name"));
            std::fs::write(out.join(name), gz.finish().expect("gzip in memory")).expect("the build folder is writable");
        }
    }
}

/// The struct fields of `src/**/*.rs` with their `///` lines, their serde
/// attributes and their type, as a table in OUT_DIR/settings_docs.rs for
/// `settings/registry.rs`: the Debug settings window shows each setting's
/// doc comment as its description, the way the Legacy Viewer shows a
/// setting's comment, with nothing to write twice. A plain line scan, not
/// a Rust parser: a struct is `struct Name {` on its own line, a field
/// `[pub[(crate)]] name: Type,` (one line, or until the line ending with a
/// comma), under its `///` lines and `#[serde(...)]` attributes.
fn settings_docs() {
    use std::fmt::Write as _;
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR");
    let source = Path::new(&manifest).join("src");
    println!("cargo:rerun-if-changed={}", source.display());
    let mut files = Vec::new();
    rust_files(&source, &mut files);
    files.sort();
    let mut out = String::from("&[\n");
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (name, fields) in scan_structs(&text) {
            let _ = writeln!(out, "    Struct {{ name: {name:?}, fields: &[");
            for f in fields {
                let _ = writeln!(
                    out,
                    "        Field {{ json: {:?}, rust: {:?}, ty: {:?}, doc: {:?}, skipped: {}, runtime: {} }},",
                    f.json, f.rust, f.ty, f.doc, f.skipped, f.runtime
                );
            }
            out.push_str("    ] },\n");
        }
    }
    out.push_str("]\n");
    let dest = Path::new(&std::env::var("OUT_DIR").expect("Cargo supplies OUT_DIR")).join("settings_docs.rs");
    std::fs::write(dest, out).expect("the build folder is writable");
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

struct ScannedField {
    json: String,
    rust: String,
    ty: String,
    doc: String,
    /// Not in the saved file (`skip`, `skip_serializing`).
    skipped: bool,
    /// `#[serde(skip)]`: a value of the running viewer the file never holds.
    runtime: bool,
}

/// The structs of one source file with their fields (see `settings_docs`).
fn scan_structs(text: &str) -> Vec<(String, Vec<ScannedField>)> {
    let mut structs = Vec::new();
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let Some(name) = struct_name(line) else {
            continue;
        };
        let mut fields = Vec::new();
        let mut doc = Vec::new();
        let mut serde = Vec::new();
        let mut partial: Option<String> = None;
        for line in lines.by_ref() {
            if line == "}" {
                break;
            }
            if let Some(rest) = partial.take() {
                let joined = format!("{rest} {line}");
                if line.ends_with(',') {
                    if let Some(field) = field_of(&joined, &doc, &serde) {
                        fields.push(field);
                    }
                    doc.clear();
                    serde.clear();
                } else {
                    partial = Some(joined);
                }
                continue;
            }
            if let Some(text) = line.strip_prefix("///") {
                doc.push(text.trim().to_owned());
            } else if let Some(attr) = line.strip_prefix("#[serde(").and_then(|a| a.strip_suffix(")]")) {
                serde.push(attr.to_owned());
            } else if line.starts_with("#[") || line.starts_with("//") || line.is_empty() {
                // another attribute, a plain comment, a blank line: the
                // doc lines above still belong to the field under them
            } else if line.ends_with(',') {
                if let Some(field) = field_of(line, &doc, &serde) {
                    fields.push(field);
                }
                doc.clear();
                serde.clear();
            } else if line.contains(':') {
                partial = Some(line.to_owned());
            } else {
                doc.clear();
                serde.clear();
            }
        }
        structs.push((name, fields));
    }
    structs
}

/// `Name` of a `[pub[(crate)]] struct Name {` line (a block struct only).
fn struct_name(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("pub(crate) ")
        .or_else(|| line.strip_prefix("pub(super) "))
        .or_else(|| line.strip_prefix("pub "))
        .unwrap_or(line);
    let rest = rest.strip_prefix("struct ")?;
    let (name, tail) = rest.split_once(' ')?;
    let name = name.split('<').next()?;
    (tail.trim_end() == "{" && name.chars().all(|c| c.is_alphanumeric() || c == '_')).then(|| name.to_owned())
}

/// The field of a `[pub[(crate)]] name: Type,` line, with its doc and serde
/// attributes.
fn field_of(line: &str, doc: &[String], serde: &[String]) -> Option<ScannedField> {
    let rest = line
        .strip_prefix("pub(crate) ")
        .or_else(|| line.strip_prefix("pub(super) "))
        .or_else(|| line.strip_prefix("pub "))
        .unwrap_or(line);
    let (name, ty) = rest.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') || name.starts_with(char::is_uppercase) {
        return None;
    }
    let ty = ty.trim().trim_end_matches(',').trim().to_owned();
    let attrs = serde.join(",");
    let mut json = name.to_owned();
    for attr in attrs.split(',').map(str::trim) {
        if let Some(renamed) = attr.strip_prefix("rename = ") {
            json = renamed.trim_matches('"').to_owned();
        }
    }
    let has = |wanted: &str| attrs.split(',').map(str::trim).any(|a| a == wanted);
    Some(ScannedField {
        json,
        rust: name.to_owned(),
        ty,
        doc: doc.join(" "),
        skipped: has("skip") || has("skip_serializing"),
        runtime: has("skip"),
    })
}

/// The window sets its icon at run time (app.rs), but the file itself needs
/// an icon resource: without one, Explorer, shortcuts and the taskbar pin
/// show the generic executable icon. The version block comes with it:
/// Properties > Details, and the name Windows gives the program (Task
/// Manager, volume mixer, firewall prompts) is its FileDescription.
///
/// The version is the one aurora-version resolved from the Git tag (its
/// `links` metadata), never CARGO_PKG_VERSION. A failure stops the build: a
/// release must not ship without its icon again.
fn windows_resources() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR");
    let icon = Path::new(&manifest).join("../../assets/branding/aurora.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let display = std::env::var("DEP_AURORA_VERSION_DISPLAY").expect("aurora-version's build script supplies the version");
    let numeric = std::env::var("DEP_AURORA_VERSION_NUMERIC").expect("aurora-version's build script supplies the version");
    // "major.minor.patch.distance" packed as four 16-bit fields
    let packed = numeric
        .split('.')
        .chain(std::iter::repeat("0"))
        .take(4)
        .fold(0u64, |acc, part| (acc << 16) | part.parse::<u64>().unwrap_or(0).min(0xFFFF));

    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.to_string_lossy())
        .set("ProductName", "Aurora Viewer")
        .set("FileDescription", "Aurora Viewer")
        .set("OriginalFilename", "aurora-viewer.exe")
        .set("ProductVersion", &display)
        .set("FileVersion", &display)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed)
        .set_version_info(winresource::VersionInfo::FILEVERSION, packed);
    // the viewer only: the test binaries of this crate need no resources
    if let Err(error) = res.compile_for("aurora-viewer") {
        panic!("Cannot embed the icon and version of aurora-viewer.exe (rc.exe of the Windows SDK): {error}");
    }
}
