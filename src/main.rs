fn main() {
    // SAFETY: setting process umask before any worker thread is spawned.
    unsafe {
        libc::umask(0o077);
    }
    match ai_quota_monitor::cli::run(std::env::args().skip(1).collect()) {
        Ok(code) => std::process::exit(code),
        Err(message) => {
            eprintln!("ai-quota: {message}");
            std::process::exit(2)
        }
    }
}
