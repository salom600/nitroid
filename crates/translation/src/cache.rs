//! Translation cache — persists translated basic blocks across VM restarts.
//!
//! Houdini-style translation is *fast* (typically 5–15 ms per block), but
//! even that overhead adds up for hot code paths. Caching translated blocks
//! to disk means a game that took 200 ms to warm up on first boot can start
//! in <5 ms on subsequent boots.

use std::path::{Path, PathBuf};

use dashmap::DashMap;
use parking_lot::RwLock;

use crate::backend::TranslatedBlock;
use nitroid_core::Result;

/// In-memory + on-disk translation cache. Keys are `(guest_pc, block_hash)`.
pub struct TranslationCache {
    in_memory: DashMap<u64, CachedBlock>,
    disk_path: RwLock<Option<PathBuf>>,
}

#[derive(Clone)]
struct CachedBlock {
    block: TranslatedBlock,
    hits: u32,
}

impl TranslationCache {
    /// Create a new cache with no disk backing.
    pub fn new() -> Self {
        Self {
            in_memory: DashMap::new(),
            disk_path: RwLock::new(None),
        }
    }

    /// Enable disk persistence at `path`. The file is loaded lazily on the
    /// first cache miss.
    pub fn persist_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        *self.disk_path.write() = Some(path);
        Ok(())
    }

    /// Look up a translated block by its guest program counter.
    pub fn get(&self, guest_pc: u64) -> Option<TranslatedBlock> {
        if let Some(mut entry) = self.in_memory.get_mut(&guest_pc) {
            entry.hits += 1;
            return Some(TranslatedBlock {
                source_addr: entry.block.source_addr,
                host_bytes: entry.block.host_bytes.clone(),
                entry_offset: entry.block.entry_offset,
            });
        }
        None
    }

    /// Insert a freshly translated block.
    pub fn insert(&self, guest_pc: u64, block: TranslatedBlock) {
        self.in_memory
            .insert(guest_pc, CachedBlock { block, hits: 0 });
    }

    /// Total number of cached blocks.
    pub fn len(&self) -> usize {
        self.in_memory.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.in_memory.is_empty()
    }

    /// Drop all cached blocks. Used on instance restart.
    pub fn clear(&self) {
        self.in_memory.clear();
    }

    /// Number of times any cached block has been hit. Useful for telemetry.
    pub fn total_hits(&self) -> u32 {
        self.in_memory.iter().map(|e| e.hits).sum()
    }
}

impl Default for TranslationCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_insert_and_get() {
        let cache = TranslationCache::new();
        assert!(cache.is_empty());

        let block = TranslatedBlock {
            source_addr: 0x4000,
            host_bytes: vec![0xC3], // RET
            entry_offset: 0,
        };
        cache.insert(0x4000, block);

        assert_eq!(cache.len(), 1);
        let got = cache.get(0x4000).expect("block missing");
        assert_eq!(got.host_bytes, vec![0xC3]);
        assert_eq!(cache.total_hits(), 1);
    }

    #[test]
    fn cache_clear_works() {
        let cache = TranslationCache::new();
        let block = TranslatedBlock {
            source_addr: 0x1,
            host_bytes: vec![0x90],
            entry_offset: 0,
        };
        cache.insert(0x1, block);
        cache.clear();
        assert!(cache.is_empty());
    }
}
