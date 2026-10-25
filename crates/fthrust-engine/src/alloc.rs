//! glibc allocator tuning: fewer arenas and explicit trimming, so the
//! engine's resident memory returns to the OS after bursts of work.
#![allow(unsafe_code)]

/// Caps glibc malloc arenas at two (no-op off glibc).
pub fn configure() {
    #[cfg(target_env = "gnu")]
    {
        // SAFETY: mallopt only adjusts allocator parameters.
        let ok = unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
        tracing::debug!(ok, "mallopt(M_ARENA_MAX, 2)");
    }
}

/// Returns free heap memory to the OS (no-op off glibc).
pub fn trim() {
    #[cfg(target_env = "gnu")]
    {
        // SAFETY: malloc_trim has no preconditions.
        let released = unsafe { libc::malloc_trim(0) };
        tracing::debug!(released, "malloc_trim(0)");
    }
}
