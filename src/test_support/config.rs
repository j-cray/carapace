use parking_lot::{Mutex, MutexGuard};

static TEST_CONFIG_CACHE_LOCK: Mutex<()> = parking_lot::const_mutex(());

/// Scoped global config-cache mutation helper for tests.
///
/// Tests that call `crate::config::clear_cache()` / `update_cache()` and then
/// rely on that process-global state across assertions or `.await` points
/// should hold this guard for the full test lifetime so they cannot race other
/// config-cache-mutating tests.
///
/// Under the current `parking_lot` configuration this guard is not `Send`, so
/// async tests on a multi-thread runtime should not hold it across an `.await`.
pub(crate) struct ScopedConfigCache {
    _lock: MutexGuard<'static, ()>,
}

impl ScopedConfigCache {
    pub(crate) fn new() -> Self {
        Self {
            _lock: TEST_CONFIG_CACHE_LOCK.lock(),
        }
    }
}
