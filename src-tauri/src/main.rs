#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "macos")]
    if secret_tunnel_lib::run_redirect_guardian_if_requested() {
        return;
    }
    secret_tunnel_lib::run()
}
