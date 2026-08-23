//! A process-wide, byte-budgeted LRU cache of decoded Parquet partitions.
//!
//! Partitions are immutable once written (rewrites land under a new name, or
//! atomically replace the file and change its mtime), so a decoded frame can be
//! shared by every query that touches it: entries are `Arc<DataFrame>`s whose
//! Arrow buffers are reference-counted, letting concurrent requests fold over
//! the same columns with no copying and no repeated disk reads. The trade-off
//! is deliberate: steady-state memory rises toward the configured budget, and
//! in exchange a warm query costs only its own aggregate state.
//!
//! Freshness is keyed by `(len, mtime)`: a partition rewritten in place (the
//! exception re-grouper) or replaced by consolidation simply misses and is
//! re-read. The compactor additionally invalidates the files it merges away or
//! deletes, so stale entries don't sit in the budget until eviction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use polars::prelude::DataFrame;

use super::tables::STORAGE_ADVICE;
use crate::errors::{Result, ResultExt};

/// Default cache budget when none is configured (256 MB).
const DEFAULT_BUDGET: usize = 256 * 1024 * 1024;

static CACHE: LazyLock<PartitionCache> = LazyLock::new(|| PartitionCache {
    state: Mutex::new(CacheState {
        budget: DEFAULT_BUDGET,
        tick: 0,
        total: 0,
        misses: 0,
        entries: HashMap::new(),
    }),
});

/// Return freed heap pages to the OS (glibc arenas otherwise retain the
/// decode transients of every cache fill forever, so container RSS would
/// plateau at the allocator's high-water mark rather than live data). Called
/// after scans that filled the cache and after compaction merges — the two
/// paths that allocate large short-lived buffers. A no-op off glibc.
pub fn trim_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

/// The process-wide partition cache (one archive per process, like the layout
/// lock in [`super::parquet`]).
pub fn partition_cache() -> &'static PartitionCache {
    &CACHE
}

pub struct PartitionCache {
    state: Mutex<CacheState>,
}

struct CacheState {
    budget: usize,
    /// Logical clock for LRU ordering.
    tick: u64,
    /// Estimated bytes held across all entries.
    total: usize,
    /// Total cache misses served (fills); scans compare snapshots to detect
    /// that they decoded fresh partitions and should trim the allocator.
    misses: u64,
    entries: HashMap<PathBuf, Entry>,
}

struct Entry {
    frame: Arc<DataFrame>,
    len: u64,
    modified: Option<SystemTime>,
    size: usize,
    last_used: u64,
}

impl PartitionCache {
    fn lock(&self) -> MutexGuard<'_, CacheState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Set the byte budget (evicting immediately if the cache is over it).
    pub fn set_budget(&self, bytes: usize) {
        let mut state = self.lock();
        state.budget = bytes;
        state.evict_to_budget(None);
    }

    /// The decoded frame for the partition at `path`, from cache when fresh.
    /// Concurrent misses on one path may each read the file; the last insert
    /// wins and every returned `Arc` stays valid regardless.
    pub fn get(&self, path: &Path) -> Result<Arc<DataFrame>> {
        self.get_with(path, Ok)
    }

    /// Like [`get`](Self::get), but a miss runs `prepare` over the decoded
    /// frame before it is cached — one-time normalization (canonical dtypes,
    /// missing columns) paid at fill rather than by every query.
    pub fn get_with(
        &self,
        path: &Path,
        prepare: impl FnOnce(DataFrame) -> Result<DataFrame>,
    ) -> Result<Arc<DataFrame>> {
        let meta = std::fs::metadata(path).or_system_err(STORAGE_ADVICE)?;
        let (len, modified) = (meta.len(), meta.modified().ok());

        {
            let mut state = self.lock();
            state.tick += 1;
            let tick = state.tick;
            if let Some(entry) = state.entries.get_mut(path)
                && entry.len == len
                && entry.modified == modified
            {
                entry.last_used = tick;
                return Ok(entry.frame.clone());
            }
        }

        // Decode outside the lock so hits never wait on a miss.
        let frame = Arc::new(prepare(super::parquet::read_partition(path)?)?);
        let size = frame.estimated_size();

        let mut state = self.lock();
        state.tick += 1;
        state.misses += 1;
        let tick = state.tick;
        // A frame bigger than the whole budget is served but never cached.
        if size <= state.budget {
            if let Some(old) = state.entries.insert(
                path.to_path_buf(),
                Entry {
                    frame: frame.clone(),
                    len,
                    modified,
                    size,
                    last_used: tick,
                },
            ) {
                state.total -= old.size;
            }
            state.total += size;
            state.evict_to_budget(Some(path));
        }
        Ok(frame)
    }

    /// Total cache misses served so far (monotonic).
    pub fn misses(&self) -> u64 {
        self.lock().misses
    }

    /// Drop the entry for `path`, if any (the file was merged away or deleted).
    pub fn invalidate(&self, path: &Path) {
        let mut state = self.lock();
        if let Some(entry) = state.entries.remove(path) {
            state.total -= entry.size;
        }
    }

    /// Drop every entry whose file no longer exists (after retention sweeps).
    pub fn prune_missing(&self) {
        let mut state = self.lock();
        let missing: Vec<PathBuf> = state
            .entries
            .keys()
            .filter(|path| !path.exists())
            .cloned()
            .collect();
        for path in missing {
            if let Some(entry) = state.entries.remove(&path) {
                state.total -= entry.size;
            }
        }
    }
}

impl CacheState {
    /// Evict least-recently-used entries until within budget, never evicting
    /// `keep` (the entry just inserted).
    fn evict_to_budget(&mut self, keep: Option<&Path>) {
        while self.total > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(path, _)| keep.is_none_or(|keep| path.as_path() != keep))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(path, _)| path.clone());
            let Some(victim) = victim else {
                break;
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.total -= entry.size;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventKind, StoredEvent, write_partition};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_partition(events: &[StoredEvent]) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "analytics-cache-{}-{}.parquet",
            std::process::id(),
            n
        ));
        write_partition(events, &path).unwrap();
        path
    }

    fn event(received_ms: i64) -> StoredEvent {
        StoredEvent {
            received_ms,
            created_ms: received_ms,
            bid: "b".into(),
            kind: EventKind::PageLoad,
            source: "https://example.com".into(),
            ..Default::default()
        }
    }

    #[test]
    fn serves_fresh_entries_and_notices_rewrites() {
        // A private cache so parallel tests don't interact.
        let cache = PartitionCache {
            state: Mutex::new(CacheState {
                budget: DEFAULT_BUDGET,
                tick: 0,
                total: 0,
                misses: 0,
                entries: HashMap::new(),
            }),
        };
        let path = temp_partition(&[event(1_000)]);

        let first = cache.get(&path).unwrap();
        let second = cache.get(&path).unwrap();
        assert_eq!(first.height(), 1);
        // The same decoded frame is shared, not re-read.
        assert!(Arc::ptr_eq(&first, &second));

        // An atomic rewrite (new content, new mtime/len) misses and re-reads.
        write_partition(&[event(1_000), event(2_000)], &path).unwrap();
        let rewritten = cache.get(&path).unwrap();
        assert_eq!(rewritten.height(), 2);

        // Invalidation and pruning drop entries.
        cache.invalidate(&path);
        assert_eq!(cache.lock().total, 0);
        cache.get(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        cache.prune_missing();
        assert!(cache.lock().entries.is_empty());
    }

    #[test]
    fn evicts_least_recently_used_beyond_budget() {
        let cache = PartitionCache {
            state: Mutex::new(CacheState {
                budget: DEFAULT_BUDGET,
                tick: 0,
                total: 0,
                misses: 0,
                entries: HashMap::new(),
            }),
        };
        let a = temp_partition(&[event(1_000)]);
        let b = temp_partition(&[event(2_000)]);
        let c = temp_partition(&[event(3_000)]);

        let size = cache.get(&a).unwrap().estimated_size();
        // Room for two frames; the third insert must evict the LRU.
        cache.set_budget(size * 2 + size / 2);
        cache.get(&b).unwrap();
        cache.get(&a).unwrap(); // touch a: b is now the LRU
        cache.get(&c).unwrap();

        let state = cache.lock();
        assert!(state.entries.contains_key(&a));
        assert!(!state.entries.contains_key(&b), "LRU entry evicted");
        assert!(state.entries.contains_key(&c));
        assert!(state.total <= state.budget);
        drop(state);

        for path in [a, b, c] {
            let _ = std::fs::remove_file(path);
        }
    }
}
