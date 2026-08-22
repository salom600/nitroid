//! Translation runner — ties together the translator and the cache.
//!
//! The KVM run loop calls into this on every guest instruction fetch that
//! doesn't have a cached translation. The runner:
//!
//! 1. Checks the in-memory cache for a hit.
//! 2. On miss, calls the translator to produce a fresh [`TranslatedBlock`].
//! 3. Stores the result in the cache.
//! 4. Returns the bytes to the caller (which will `mmap` them as executable
//!    and jump into them).

use std::sync::Arc;

use parking_lot::RwLock;
use tracing::{debug, info};

use crate::backend::{TranslatedBlock, Translator, TranslatorBackend};
use crate::cache::TranslationCache;
use nitroid_core::Result;

/// Top-level translation runner. Owns the cache and (optionally) the
/// translator backend.
pub struct TranslationRunner {
    cache: Arc<TranslationCache>,
    translator: RwLock<Box<dyn Translator>>,
    /// Statistics for the UI / telemetry.
    stats: RwLock<RunnerStats>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RunnerStats {
    pub total_fetches: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub translation_errors: u64,
}

impl TranslationRunner {
    /// Create a new runner with the given backend and a fresh in-memory cache.
    pub fn new(backend: TranslatorBackend) -> Result<Self> {
        // Box the backend. We use a `Box<dyn Translator>` so the cache logic
        // doesn't need to be monomorphised per backend.
        let translator: Box<dyn Translator> = match backend {
            TranslatorBackend::Native(n) => Box::new(n),
            // Houdini and Unavailable are handled via the enum's Translator impl.
            other => Box::new(other),
        };
        Ok(Self {
            cache: Arc::new(TranslationCache::new()),
            translator: RwLock::new(translator),
            stats: RwLock::new(RunnerStats::default()),
        })
    }

    /// Replace the translator backend at runtime. Useful for hot-swapping
    /// Houdini in after the guest image is mounted.
    pub fn set_backend(&self, backend: TranslatorBackend) {
        let translator: Box<dyn Translator> = match backend {
            TranslatorBackend::Native(n) => Box::new(n),
            other => Box::new(other),
        };
        *self.translator.write() = translator;
        info!("translation backend swapped");
    }

    /// Fetch (and cache) the host bytes for a guest PC. Returns the
    /// translated block, ready to be `mmap`'d as executable.
    pub fn fetch(&self, guest_pc: u64, guest_bytes: &[u8]) -> Result<TranslatedBlock> {
        {
            let mut stats = self.stats.write();
            stats.total_fetches += 1;
        }

        // 1. Check the cache.
        if let Some(cached) = self.cache.get(guest_pc) {
            debug!(guest_pc, "translation cache hit");
            self.stats.write().cache_hits += 1;
            return Ok(cached);
        }

        // 2. Miss — translate.
        self.stats.write().cache_misses += 1;
        let translator = self.translator.read();
        let block = match translator.translate(guest_pc, guest_bytes) {
            Ok(b) => b,
            Err(e) => {
                self.stats.write().translation_errors += 1;
                return Err(e);
            }
        };

        // 3. Store in the cache for future hits.
        self.cache.insert(guest_pc, block.clone());

        // 4. Return a copy.
        Ok(block)
    }

    /// Get a snapshot of the current runner stats. Cheap to call.
    pub fn stats(&self) -> RunnerStats {
        *self.stats.read()
    }

    /// Get the underlying cache handle. Used by the UI to display the
    /// number of cached blocks.
    pub fn cache(&self) -> Arc<TranslationCache> {
        self.cache.clone()
    }

    /// Drop all cached translations. Called on instance reset.
    pub fn flush(&self) {
        self.cache.clear();
        *self.stats.write() = RunnerStats::default();
        info!("translation cache flushed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::NativeBackend;

    #[test]
    fn native_runner_translates_identity() {
        let runner = TranslationRunner::new(TranslatorBackend::Native(NativeBackend)).unwrap();
        let bytes = vec![0xC3]; // RET
        let block = runner.fetch(0x1000, &bytes).unwrap();
        assert_eq!(block.host_bytes, bytes);
        assert_eq!(block.source_addr, 0x1000);

        // Second fetch should hit the cache.
        let _block2 = runner.fetch(0x1000, &bytes).unwrap();
        let stats = runner.stats();
        assert_eq!(stats.total_fetches, 2);
        assert_eq!(stats.cache_hits, 1);
        assert_eq!(stats.cache_misses, 1);
    }

    #[test]
    fn flush_clears_stats_and_cache() {
        let runner = TranslationRunner::new(TranslatorBackend::Native(NativeBackend)).unwrap();
        runner.fetch(0x1, &[0x90]).unwrap();
        assert_eq!(runner.stats().cache_misses, 1);
        assert_eq!(runner.cache().len(), 1);

        runner.flush();
        assert_eq!(runner.stats().cache_misses, 0);
        assert_eq!(runner.cache().len(), 0);
    }

    #[test]
    fn unavailable_backend_errors() {
        let runner = TranslationRunner::new(TranslatorBackend::Unavailable).unwrap();
        let result = runner.fetch(0x1, &[0x90]);
        assert!(result.is_err());
        assert_eq!(runner.stats().translation_errors, 1);
    }
}
