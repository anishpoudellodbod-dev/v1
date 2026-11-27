use chrono::Utc;
use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;

pub const DEFAULT_IDEMPOTENCY_TTL_SECS: i64 = 86_400;
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 10_000;

#[derive(Debug, Clone)]
struct TimedEntry<Req, Res> {
    pair: (Req, Res),
    expires_at: i64,
}

#[derive(Debug)]
pub struct BoundedIdempotencyMap<Req, Res> {
    entries: HashMap<String, TimedEntry<Req, Res>>,
    expiry_order: BTreeMap<i64, Vec<String>>,
    ttl_secs: i64,
    max_entries: usize,
}

impl<Req, Res> Default for BoundedIdempotencyMap<Req, Res> {
    fn default() -> Self {
        Self::new(DEFAULT_IDEMPOTENCY_TTL_SECS, MAX_IDEMPOTENCY_ENTRIES)
    }
}

impl<Req, Res> BoundedIdempotencyMap<Req, Res> {
    pub fn new(ttl_secs: i64, max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            expiry_order: BTreeMap::new(),
            ttl_secs: ttl_secs.max(1),
            max_entries: max_entries.max(1),
        }
    }

    pub fn prune_expired_at(&mut self, now: i64) {
        let expired_buckets: Vec<i64> = self
            .expiry_order
            .range(..=now)
            .map(|(&ts, _)| ts)
            .collect();
        for ts in expired_buckets {
            if let Some(keys) = self.expiry_order.remove(&ts) {
                for key in keys {
                    if let Some(entry) = self.entries.get(&key) {
                        if entry.expires_at <= now {
                            self.entries.remove(&key);
                        }
                    }
                }
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&(Req, Res)> {
        let now = Utc::now().timestamp();
        let entry = self.entries.get(key)?;
        if entry.expires_at <= now {
            return None;
        }
        Some(&entry.pair)
    }

    pub fn get_pair(&self, key: &str) -> Option<(&Req, &Res)> {
        let (r, res) = self.get(key)?;
        Some((r, res))
    }

    pub fn insert_pair(&mut self, key: String, req: Req, res: Res) {
        let now = Utc::now().timestamp();
        self.prune_expired_at(now);
        while self.entries.len() >= self.max_entries {
            if let Some((&oldest_ts, _)) = self.expiry_order.iter().next() {
                if let Some(keys) = self.expiry_order.remove(&oldest_ts) {
                    for old_k in keys {
                        self.entries.remove(&old_k);
                    }
                }
            } else {
                break;
            }
        }
        let exp = now + self.ttl_secs;
        self.entries.insert(
            key.clone(),
            TimedEntry {
                pair: (req, res),
                expires_at: exp,
            },
        );
        self.expiry_order.entry(exp).or_default().push(key);
    }

    pub fn insert(&mut self, key: String, pair: (Req, Res)) {
        self.insert_pair(key, pair.0, pair.1);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Thread-safe TTL + capacity bounded idempotency cache replacing unbounded static HashMap.
pub struct IdempotencyCache<Req, Res> {
    inner: RwLock<BoundedIdempotencyMap<Req, Res>>,
}

impl<Req, Res> Default for IdempotencyCache<Req, Res> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Req, Res> IdempotencyCache<Req, Res> {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(BoundedIdempotencyMap::default()),
        }
    }

    pub fn with_ttl_and_capacity(ttl_secs: i64, max_entries: usize) -> Self {
        Self {
            inner: RwLock::new(BoundedIdempotencyMap::new(ttl_secs, max_entries)),
        }
    }

    pub fn read(
        &self,
    ) -> std::sync::LockResult<std::sync::RwLockReadGuard<'_, BoundedIdempotencyMap<Req, Res>>> {
        self.inner.read()
    }

    pub fn write(
        &self,
    ) -> std::sync::LockResult<std::sync::RwLockWriteGuard<'_, BoundedIdempotencyMap<Req, Res>>>
    {
        self.inner.write()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_idempotency_evicts_expired_and_caps_size() {
        let mut map = BoundedIdempotencyMap::new(60, 2);
        map.insert_pair("k1".into(), 1, "r1");
        map.insert_pair("k2".into(), 2, "r2");
        map.insert_pair("k3".into(), 3, "r3");
        assert!(map.len() <= 2);
        assert_eq!(map.get_pair("k3"), Some((&3, &"r3")));
    }
}
