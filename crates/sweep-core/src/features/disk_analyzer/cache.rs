//! A tiny LRU keyed by scan id, shared by the disk analyzer and the duplicate finder.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Keeps at most `cap` entries; inserting beyond that evicts the least recently used.
pub(crate) struct LruCache<T> {
    cap: usize,
    /// Most recently used last.
    entries: VecDeque<(String, Arc<Mutex<T>>)>,
}

impl<T> LruCache<T> {
    pub const fn new(cap: usize) -> Self {
        Self {
            cap,
            entries: VecDeque::new(),
        }
    }

    pub fn insert(&mut self, id: String, value: T) -> Arc<Mutex<T>> {
        let arc = Arc::new(Mutex::new(value));
        self.entries.retain(|(k, _)| *k != id);
        self.entries.push_back((id, arc.clone()));
        while self.entries.len() > self.cap {
            self.entries.pop_front();
        }
        arc
    }

    /// Look up and mark as most recently used.
    pub fn get(&mut self, id: &str) -> Option<Arc<Mutex<T>>> {
        let pos = self.entries.iter().position(|(k, _)| k == id)?;
        let e = self.entries.remove(pos)?;
        let arc = e.1.clone();
        self.entries.push_back(e);
        Some(arc)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Process-wide store of scans, one [`LruCache`] per app data directory so independent
/// instances (and parallel tests, each with its own data dir) never evict each other.
pub(crate) struct ScanStore<T> {
    cap: usize,
    map: OnceLock<Mutex<HashMap<PathBuf, LruCache<T>>>>,
}

impl<T> ScanStore<T> {
    pub const fn new(cap: usize) -> Self {
        Self {
            cap,
            map: OnceLock::new(),
        }
    }

    fn with<R>(&self, owner: &Path, f: impl FnOnce(&mut LruCache<T>) -> R) -> R {
        let mut map = self
            .map
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let cache = map
            .entry(owner.to_path_buf())
            .or_insert_with(|| LruCache::new(self.cap));
        f(cache)
    }

    pub fn insert(&self, owner: &Path, id: String, value: T) -> Arc<Mutex<T>> {
        self.with(owner, |c| c.insert(id, value))
    }

    pub fn get(&self, owner: &Path, id: &str) -> Option<Arc<Mutex<T>>> {
        self.with(owner, |c| c.get(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let mut c = LruCache::new(3);
        for i in 0..3 {
            c.insert(format!("s{i}"), i);
        }
        assert!(c.get("s0").is_some()); // s0 is now the freshest
        c.insert("s3".into(), 3);
        assert_eq!(c.len(), 3);
        assert!(c.get("s1").is_none(), "s1 was the least recently used");
        assert!(c.get("s0").is_some());
        assert!(c.get("s2").is_some());
        assert!(c.get("s3").is_some());
    }
}
