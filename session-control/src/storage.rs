//! Replace a file without exposing a truncated destination to readers.
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing config directory"))?;
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("Missing config filename"))?
        .to_string_lossy();
    let temp = parent.join(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        #[cfg(not(windows))]
        fs::rename(&temp, path)?;
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            #[link(name = "kernel32")]
            extern "system" {
                fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
            }
            let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // Same-directory rename; replacement + write-through, never delete first.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            loop {
                if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } != 0 {
                    break;
                }
                let error = io::Error::last_os_error();
                // A reader or antivirus can briefly deny replacement. Keep the old
                // file intact while retrying; never fall back to delete + write.
                if !matches!(error.raw_os_error(), Some(5 | 32 | 33))
                    || std::time::Instant::now() >= deadline
                {
                    return Err(error);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_readers_never_see_partial_contents() {
        let dir = std::env::temp_dir().join(format!(
            "st-atomic-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let path = dir.join("config.json");
        replace(&path, &vec![1; 65536]).unwrap();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let path = path.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    let data = fs::read(&path).unwrap();
                    assert_eq!(data.len(), 65536);
                    assert!(data.iter().all(|b| *b == data[0]));
                }
            })
        };
        for i in 0..32 {
            replace(&path, &vec![i; 65536]).unwrap();
        }
        done.store(true, Ordering::SeqCst);
        reader.join().unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
