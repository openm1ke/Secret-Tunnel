#![windows_subsystem = "windows"]
// A disposable installer/app for the real Windows update handoff tests.
use std::{env, fs, path::PathBuf, thread, time::Duration};
fn main() {
    if env::args().nth(1).as_deref() == Some("--wait") {
        thread::sleep(Duration::from_secs(120));
        return;
    }
    let exe = env::current_exe().unwrap();
    let data = fs::read_to_string(exe.parent().unwrap().join("fixture.txt")).unwrap();
    let mut fields = data.lines();
    let root = PathBuf::from(fields.next().unwrap());
    if exe.file_name().unwrap() == "setup.exe" {
        fs::write(
            root.join("installer-args"),
            env::args().skip(1).collect::<Vec<_>>().join(" "),
        )
        .unwrap();
        fs::write(
            root.join("installer-cwd"),
            env::current_dir().unwrap().to_string_lossy().as_bytes(),
        )
        .unwrap();
        let code: i32 = fields.next().unwrap().parse().unwrap();
        if code == 0 {
            let target = fields.next().unwrap_or("");
            if let Err(error) = fs::copy(&exe, target) {
                fs::write(root.join("installer-error"), error.to_string()).unwrap();
                std::process::exit(12);
            }
            fs::write(root.join("app-replaced"), "yes").unwrap();
        }
        std::process::exit(code);
    }
    fs::write(root.join("app-restarted"), "yes").unwrap();
}
