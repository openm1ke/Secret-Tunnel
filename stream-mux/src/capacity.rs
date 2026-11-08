/// Budget for a SOCKS control socket plus one UDP socket per flow, leaving
/// descriptors for SSH, runtime, files and UI. Only this process is affected.
pub fn socket_capacity(requested: usize) -> usize {
    #[cfg(unix)]
    unsafe {
        let mut limits = std::mem::zeroed::<libc::rlimit>();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) == 0 {
            let wanted = limits.rlim_max.min(8192);
            if limits.rlim_cur < wanted {
                limits.rlim_cur = wanted;
                let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &limits);
            }
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) == 0 {
                return requested.min((limits.rlim_cur as usize).saturating_sub(128) / 2);
            }
        }
    }
    requested
}
