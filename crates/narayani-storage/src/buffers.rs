use std::collections::VecDeque;

pub const MAX_LOG_ENTRIES: usize = 10_000;
pub const MAX_AUDIT_ENTRIES: usize = 10_000;

/// Bounded FIFO ring buffer backed by `VecDeque`. Drops the oldest entry when `capacity` is reached.
#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> RingBuffer<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            items: VecDeque::with_capacity(cap.min(1024)),
            capacity: cap,
        }
    }

    pub fn push(&mut self, item: T) {
        if self.items.len() >= self.capacity {
            self.items.pop_front();
        }
        self.items.push_back(item);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, T> {
        self.items.iter()
    }

    pub fn iter_mut(&mut self) -> std::collections::vec_deque::IterMut<'_, T> {
        self.items.iter_mut()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&T) -> bool,
    {
        self.items.retain(f);
    }

    pub fn pop_front_batch(&mut self, max: usize) -> Vec<T> {
        let count = max.min(self.items.len());
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(item) = self.items.pop_front() {
                out.push(item);
            }
        }
        out
    }
}

impl<T: Clone> RingBuffer<T> {
    pub fn to_vec(&self) -> Vec<T> {
        self.items.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_evicts_oldest_when_full() {
        let mut buf = RingBuffer::with_capacity(3);
        buf.push(1);
        buf.push(2);
        buf.push(3);
        buf.push(4);
        assert_eq!(buf.to_vec(), vec![2, 3, 4]);
        assert_eq!(buf.len(), 3);
    }
}
