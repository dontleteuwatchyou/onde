//! Keeps memory use down. glibc gives each busy thread its own pool and keeps
//! what was freed there: decoding a large cover on one of the UI framework's
//! many workers would otherwise stay counted against Onde for good.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    unsafe extern "C" {
        pub fn mallopt(param: i32, value: i32) -> i32;
        pub fn malloc_trim(pad: usize) -> i32;
    }
    /// From malloc.h.
    pub const M_ARENA_MAX: i32 = -8;
}

/// Call first thing, before any thread starts.
pub fn limit_pools() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: plain glibc tuning call, made before other threads exist.
    unsafe {
        glibc::mallopt(glibc::M_ARENA_MAX, 2);
    }
}

/// Returns freed memory to the system, after work that used a lot of it.
pub fn give_back() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only releases free pages; it is thread-safe.
    unsafe {
        glibc::malloc_trim(0);
    }
}
