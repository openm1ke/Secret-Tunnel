//! Read bundle metadata without executing applications or requiring Spotlight.
use crate::installed_apps::InstalledApp;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

fn metadata(path: &Path) -> Option<(String, String)> {
    let value = plist::Value::from_file(path.join("Contents/Info.plist")).ok()?;
    let info = value.as_dictionary()?;
    let id = info.get("CFBundleIdentifier")?.as_string()?.to_owned();
    let name = info
        .get("CFBundleDisplayName")
        .or_else(|| info.get("CFBundleName"))
        .and_then(plist::Value::as_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Application")
        })
        .to_owned();
    Some((id, name))
}

// Helpers have their own signing identifiers. Include exact nested bundle IDs,
// rather than a prefix wildcard that could also select unrelated applications.
fn helpers(path: &Path, depth: usize, ids: &mut Vec<String>, budget: &mut usize) {
    if depth == 0 || *budget == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        } // do not follow framework symlinks
        let child = entry.path();
        if matches!(
            child.extension().and_then(|s| s.to_str()),
            Some("app" | "xpc")
        ) {
            if let Some((id, _)) = metadata(&child) {
                if super::macos_redirect::validate_ids(std::slice::from_ref(&id)).is_ok() {
                    ids.push(id);
                }
            }
        }
        helpers(&child, depth - 1, ids, budget);
    }
}

pub fn describe(path: &Path) -> Result<InstalledApp, String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("Не удалось открыть приложение: {e}"))?;
    if path.extension().and_then(|s| s.to_str()) != Some("app") || !path.is_dir() {
        return Err("Выберите приложение macOS с расширением .app".into());
    }
    let (id, name) = metadata(&path).ok_or("В приложении отсутствует CFBundleIdentifier")?;
    super::macos_redirect::validate_ids(std::slice::from_ref(&id))?;
    let primary = id.clone();
    let mut patterns = vec![id];
    helpers(&path.join("Contents"), 10, &mut patterns, &mut 20_000);
    patterns.sort();
    patterns.dedup();
    Ok(InstalledApp {
        id: path.to_string_lossy().into_owned(),
        name,
        executable: path.to_string_lossy().into_owned(),
        patterns,
        source: "macos".into(),
        primary,
    })
}

fn discover(path: &Path, depth: usize, paths: &mut Vec<PathBuf>, budget: &mut usize) {
    if depth == 0 || *budget == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let child = entry.path();
        if child.extension().and_then(|s| s.to_str()) == Some("app") {
            paths.push(child);
        } else if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            discover(&child, depth - 1, paths, budget);
        }
    }
}

/// Icon file of a bundle: `CFBundleIconFile` or `CFBundleIconName` as an
/// `.icns` in Resources, else `AppIcon.icns`. Icons kept only in an asset
/// catalog have no file and get a letter badge in the UI.
fn icon_file(app: &Path) -> Option<PathBuf> {
    let value = plist::Value::from_file(app.join("Contents/Info.plist")).ok()?;
    let info = value.as_dictionary()?;
    let resources = app.join("Contents/Resources");
    for key in ["CFBundleIconFile", "CFBundleIconName"] {
        if let Some(name) = info.get(key).and_then(plist::Value::as_string) {
            if name.contains('/') {
                continue;
            }
            let mut file = resources.join(name);
            if file.extension().is_none() {
                file.set_extension("icns");
            }
            if file.is_file() {
                return Some(file);
            }
        }
    }
    let fallback = resources.join("AppIcon.icns");
    fallback.is_file().then_some(fallback)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if index <= chunk.len() {
                TABLE[(n >> shift) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}

/// A 64-pixel PNG of the bundle's icon as a data URL, converted by the system
/// `sips` tool; nothing in the bundle is executed.
fn icon_data_url(app: &Path) -> Option<String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let icns = icon_file(app)?;
    let out = std::env::temp_dir().join(format!(
        "secret-tunnel-icon-{}-{}.png",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let status = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "png", "-Z", "64"])
        .arg(&icns)
        .arg("--out")
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok();
    let bytes = std::fs::read(&out).ok();
    let _ = std::fs::remove_file(&out);
    let bytes = bytes.filter(|_| status.is_some_and(|s| s.success()))?;
    Some(format!("data:image/png;base64,{}", base64(&bytes)))
}

/// Icons of `.app` bundles by path; bundles without an icon file are left out.
/// Results are cached for the life of the process.
pub fn icons(paths: &[String]) -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let wanted: Vec<String> = {
        let known = cache.lock().unwrap();
        paths.iter().filter(|path| !known.contains_key(*path)).cloned().collect()
    };
    let next = Mutex::new(wanted.into_iter());
    std::thread::scope(|scope| {
        for _ in 0..6 {
            scope.spawn(|| loop {
                let Some(path) = next.lock().unwrap().next() else { break };
                let app = Path::new(&path);
                let icon = (app.extension().and_then(|s| s.to_str()) == Some("app") && app.is_dir())
                    .then(|| icon_data_url(app))
                    .flatten();
                cache.lock().unwrap().insert(path, icon);
            });
        }
    });
    let known = cache.lock().unwrap();
    paths
        .iter()
        .filter_map(|path| Some((path.clone(), known.get(path)?.clone()?)))
        .collect()
}

pub fn scan() -> Result<Vec<InstalledApp>, String> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Library/CoreServices/Applications"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    let mut paths = Vec::new();
    let mut budget = 20_000;
    for root in roots {
        discover(&root, 5, &mut paths, &mut budget);
    }
    let mut seen = HashSet::new();
    let mut apps: Vec<_> = paths
        .iter()
        .filter_map(|p| describe(p).ok())
        .filter(|app| seen.insert(app.id.clone()))
        .collect();
    apps.sort_by_cached_key(|app| app.name.to_lowercase());
    Ok(apps)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
    #[test]
    fn a_system_app_icon_becomes_a_png_data_url() {
        let icons = icons(&["/System/Applications/Calculator.app".into(), "/nonexistent.app".into()]);
        if Path::new("/System/Applications/Calculator.app/Contents/Resources/AppIcon.icns").is_file() {
            assert!(icons["/System/Applications/Calculator.app"].starts_with("data:image/png;base64,iVBOR"));
        }
        assert!(!icons.contains_key("/nonexistent.app"));
    }
    #[test]
    fn reads_binary_plists_and_exact_helper_ids() {
        let root =
            std::env::temp_dir().join(format!("secret-tunnel-bundles-{}", std::process::id()));
        let app = root.join("Test App.app");
        let helper = app.join("Contents/Frameworks/Helper.app");
        for (path, id) in [
            (&app, "org.example.Test"),
            (&helper, "org.example.Test.helper"),
        ] {
            std::fs::create_dir_all(path.join("Contents")).unwrap();
            let value = plist::Value::Dictionary(
                [
                    ("CFBundleIdentifier", plist::Value::String(id.into())),
                    ("CFBundleName", plist::Value::String("Test App".into())),
                ]
                .into_iter()
                .collect(),
            );
            value
                .to_file_binary(path.join("Contents/Info.plist"))
                .unwrap();
        }
        let result = describe(&app).unwrap();
        assert_eq!(result.name, "Test App");
        // Helpers are sorted in with the bundle ID; `primary` keeps the app's own.
        assert_eq!(result.primary, "org.example.Test");
        assert_eq!(
            result.patterns,
            ["org.example.Test", "org.example.Test.helper"]
        );
        assert!(describe(&app.join("Contents")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
