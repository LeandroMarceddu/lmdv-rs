use std::collections::HashMap;

use crate::{MathRequest, RenderedMath};

/// Everything that changes the pixels or the layout of a formula.
///
/// Floats are keyed by their bit pattern so `2.0` and `2.000_001` stay distinct
/// without requiring `Eq`/`Hash` on the public request type.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CacheKey {
    tex: String,
    size_bits: u32,
    color: [u8; 4],
    display: bool,
    scale_bits: u32,
}

impl CacheKey {
    pub(crate) fn new(request: &MathRequest) -> Self {
        Self {
            tex: request.tex.clone(),
            size_bits: request.size_pt.to_bits(),
            color: request.color,
            display: request.display,
            scale_bits: request.scale.to_bits(),
        }
    }
}

struct Entry {
    value: RenderedMath,
    last_used: u64,
    bytes: usize,
}

/// A small least-recently-used cache with both an entry and a byte limit.
///
/// The entry limit bounds bookkeeping; the byte limit bounds the copied RGBA
/// buffers, which are the expensive part. An entry larger than the whole byte
/// budget is rendered but never retained, so one huge display formula cannot
/// flush or exceed the cache.
pub(crate) struct MathCache {
    entries: HashMap<CacheKey, Entry>,
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
    clock: u64,
}

impl MathCache {
    pub(crate) fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self { entries: HashMap::new(), max_entries, max_bytes, bytes: 0, clock: 0 }
    }

    pub(crate) fn get(&mut self, key: &CacheKey) -> Option<RenderedMath> {
        let next = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(key)?;
        self.clock = next;
        entry.last_used = next;
        Some(entry.value.clone())
    }

    pub(crate) fn insert(&mut self, key: CacheKey, value: RenderedMath) {
        if self.max_entries == 0 || self.max_bytes == 0 {
            return;
        }

        let bytes = entry_bytes(&key, &value);
        if bytes > self.max_bytes {
            return;
        }

        self.clock = self.clock.wrapping_add(1);
        if let Some(previous) = self.entries.insert(key, Entry { value, last_used: self.clock, bytes }) {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.evict();
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn evict(&mut self) {
        while self.entries.len() > self.max_entries || self.bytes > self.max_bytes {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, entry)| entry.last_used).map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
        }
    }
}

/// Approximate retained bytes for one entry: the structs plus the two heap
/// allocations (TeX source and RGBA). `HashMap` bucket overhead is not counted;
/// the entry cap keeps that bounded.
fn entry_bytes(key: &CacheKey, value: &RenderedMath) -> usize {
    std::mem::size_of::<Entry>() + key.tex.capacity() + value.rgba.capacity()
}
