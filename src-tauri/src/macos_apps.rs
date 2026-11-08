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
        assert_eq!(
            result.patterns,
            ["org.example.Test", "org.example.Test.helper"]
        );
        assert!(describe(&app.join("Contents")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
