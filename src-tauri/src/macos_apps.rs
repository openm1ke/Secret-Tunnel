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

/// The code-signing identifier macOS reports for flows of a program: the
/// `Identifier` of its CodeDirectory, which need not match the bundle ID
/// (ChatGPT's `CodexCLI.app` is `com.openai.codex.cli`, its program `codex`).
/// An unsigned program is matched by its file name. `None` for anything
/// that is not a Mach-O executable (libraries, plug-ins, scripts).
pub fn signing_identifier(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut read_at = |offset: u64, length: usize| -> Option<Vec<u8>> {
        let mut buffer = vec![0; length];
        file.seek(SeekFrom::Start(offset)).ok()?;
        file.read_exact(&mut buffer).ok()?;
        Some(buffer)
    };
    let be = |bytes: &[u8], at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let le = |bytes: &[u8], at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let head = read_at(0, 32)?;
    // A universal binary: every slice carries the same identifier.
    let slice = match be(&head, 0)? {
        0xcafe_babe | 0xcafe_babf => {
            let wide = be(&head, 0)? == 0xcafe_babf;
            if !(1..=16).contains(&be(&head, 4)?) {
                return None; // Java class files share the magic number.
            }
            let entry = read_at(8, if wide { 32 } else { 20 })?;
            if wide {
                u64::from_be_bytes(entry[8..16].try_into().ok()?)
            } else {
                u64::from(be(&entry, 8)?)
            }
        }
        _ => 0,
    };
    let header = read_at(slice, 32)?;
    let header_size = match le(&header, 0)? {
        0xfeed_facf => 32,
        0xfeed_face => 28,
        _ => return None,
    };
    const MH_EXECUTE: u32 = 2;
    const LC_CODE_SIGNATURE: u32 = 0x1d;
    if le(&header, 12)? != MH_EXECUTE {
        return None;
    }
    let name = || path.file_name()?.to_str().map(str::to_owned);
    let commands_size = le(&header, 20)? as usize;
    if commands_size > 1 << 20 {
        return None;
    }
    let commands = read_at(slice + header_size, commands_size)?;
    let mut at = 0;
    let mut signature = None;
    for _ in 0..le(&header, 16)? {
        let (command, size) = (le(&commands, at)?, le(&commands, at + 4)? as usize);
        if command == LC_CODE_SIGNATURE {
            signature = Some((le(&commands, at + 8)?, le(&commands, at + 12)?));
            break;
        }
        if size < 8 {
            return None;
        }
        at += size;
    }
    let Some((offset, size)) = signature else { return name() };
    let blob = read_at(slice + u64::from(offset), (size as usize).min(1 << 20))?;
    if be(&blob, 0)? != 0xfade_0cc0 {
        return name();
    }
    const CODE_DIRECTORY_SLOT: u32 = 0;
    let directory = (0..be(&blob, 8)? as usize)
        .find(|index| be(&blob, 12 + index * 8) == Some(CODE_DIRECTORY_SLOT))
        .and_then(|index| be(&blob, 16 + index * 8))? as usize;
    if be(&blob, directory)? != 0xfade_0c02 {
        return name();
    }
    let identifier = &blob.get(directory + be(&blob, directory + 20)? as usize..)?;
    let end = identifier.iter().position(|&byte| byte == 0)?;
    std::str::from_utf8(&identifier[..end]).ok().map(str::to_owned)
}

// Helpers and bundled programs have their own signing identifiers. Include
// the exact IDs of nested bundles and of every executable inside the app,
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
        let child = entry.path();
        if kind.is_file() {
            use std::os::unix::fs::PermissionsExt;
            let executable = entry.metadata().is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0 && meta.len() >= 4096);
            if let Some(id) = executable.then(|| signing_identifier(&child)).flatten() {
                if super::macos_redirect::validate_ids(std::slice::from_ref(&id)).is_ok() {
                    ids.push(id);
                }
            }
            continue;
        }
        if !kind.is_dir() {
            continue;
        } // do not follow framework symlinks
        if matches!(
            child.extension().and_then(|s| s.to_str()),
            Some("app" | "xpc" | "appex")
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
    Some(format!("data:image/png;base64,{}", crate::icon_png::base64(&bytes)))
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

/// Installed applications whose own bundle ID is among `ids`, freshly
/// described: an update can add helpers or programs since the selection was
/// saved, and saved selections predate signing identifiers of programs.
pub fn selected(ids: &[String]) -> Vec<InstalledApp> {
    let wanted: HashSet<String> = ids.iter().map(|id| id.to_ascii_lowercase()).collect();
    let mut paths = Vec::new();
    let mut budget = 20_000;
    for root in roots() {
        discover(&root, 5, &mut paths, &mut budget);
    }
    let mut seen = HashSet::new();
    paths
        .iter()
        .filter(|path| metadata(path).is_some_and(|(id, _)| wanted.contains(&id.to_ascii_lowercase())))
        .filter_map(|path| describe(path).ok())
        .filter(|app| seen.insert(app.primary.to_ascii_lowercase()))
        .collect()
}

fn roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Library/CoreServices/Applications"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots
}

pub fn scan() -> Result<Vec<InstalledApp>, String> {
    let mut paths = Vec::new();
    let mut budget = 20_000;
    for root in roots() {
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
    fn a_system_app_icon_becomes_a_png_data_url() {
        let icons = icons(&["/System/Applications/Calculator.app".into(), "/nonexistent.app".into()]);
        if Path::new("/System/Applications/Calculator.app/Contents/Resources/AppIcon.icns").is_file() {
            assert!(icons["/System/Applications/Calculator.app"].starts_with("data:image/png;base64,iVBOR"));
        }
        assert!(!icons.contains_key("/nonexistent.app"));
    }
    #[test]
    fn programs_are_known_by_their_signing_identifier() {
        // A signed universal system program and a library.
        assert_eq!(signing_identifier(Path::new("/usr/bin/curl")).as_deref(), Some("com.apple.curl"));
        assert_eq!(signing_identifier(Path::new("/usr/lib/libSystem.B.dylib")), None);
        let chatgpt = Path::new("/Applications/ChatGPT.app");
        if chatgpt.join("Contents/Resources/codex-cli/CodexCLI.app").is_dir() {
            let app = describe(chatgpt).unwrap();
            assert!(app.patterns.iter().any(|id| id == "codex"), "{:?}", app.patterns);
        }
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
