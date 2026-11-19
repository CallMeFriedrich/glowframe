//! Small file cache with size-bounded LRU eviction (by modification time,
//! refreshed on read). Used for HD covers (SPEC §6.3) and later lyrics.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
}

impl DiskCache {
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        Self { dir, max_bytes }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Reads an entry and marks it as recently used.
    pub fn get(&self, name: &str) -> Option<Vec<u8>> {
        let path = self.path(name);
        let bytes = std::fs::read(&path).ok()?;
        touch(&path);
        Some(bytes)
    }

    /// Age of an entry (time since last write or use).
    pub fn age(&self, name: &str) -> Option<Duration> {
        let modified = std::fs::metadata(self.path(name)).ok()?.modified().ok()?;
        SystemTime::now().duration_since(modified).ok()
    }

    pub fn put(&self, name: &str, bytes: &[u8]) {
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(&self.dir)?;
            let tmp = self.path(&format!("{name}.tmp"));
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, self.path(name))
        })();
        match result {
            Ok(()) => self.evict(),
            Err(e) => tracing::warn!(error = %e, name, "cache write failed"),
        }
    }

    pub fn remove(&self, name: &str) {
        let _ = std::fs::remove_file(self.path(name));
    }

    /// Deletes the least recently used files until the size limit holds.
    fn evict(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(PathBuf, u64, SystemTime)> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                m.is_file().then(|| {
                    (
                        e.path(),
                        m.len(),
                        m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    )
                })
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        if total <= self.max_bytes {
            return;
        }
        files.sort_by_key(|f| f.2);
        for (path, len, _) in files {
            if total <= self.max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
    }
}

fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(name: &str, max: u64) -> DiskCache {
        let dir = std::env::temp_dir()
            .join("glowframe-cache-tests")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        DiskCache::new(dir, max)
    }

    #[test]
    fn put_get_remove() {
        let c = cache("basic", 1000);
        assert!(c.get("a").is_none());
        c.put("a", b"hello");
        assert_eq!(c.get("a").as_deref(), Some(&b"hello"[..]));
        assert!(c.age("a").is_some());
        c.remove("a");
        assert!(c.get("a").is_none());
    }

    #[test]
    fn evicts_least_recently_used() {
        let c = cache("lru", 25);
        c.put("old", &[0; 10]);
        // Make "old" clearly older, then use "mid" so it is newer.
        let past = SystemTime::now() - Duration::from_secs(100);
        std::fs::File::options()
            .write(true)
            .open(c.path("old"))
            .unwrap()
            .set_modified(past)
            .unwrap();
        c.put("mid", &[0; 10]);
        c.put("new", &[0; 10]); // 30 > 25 -> "old" goes
        assert!(c.get("old").is_none());
        assert!(c.get("mid").is_some());
        assert!(c.get("new").is_some());
    }
}
