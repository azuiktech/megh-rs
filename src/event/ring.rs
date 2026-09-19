//! Fixed-capacity circular ring buffer for transient replay.

use std::collections::VecDeque;

/// A fixed-capacity circular buffer that overwrites the oldest element when full.
#[derive(Clone, Debug)]
pub struct RingBuffer<T> {
    data: VecDeque<T>,
    capacity: usize,
}

impl<T> RingBuffer<T> {
    /// Creates a new ring buffer with the given maximum capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    /// Pushes an item to the back of the buffer.
    /// Overwrites the oldest item if capacity is reached.
    pub fn push(&mut self, item: T) {
        if self.data.len() >= self.capacity {
            self.data.pop_front();
        }
        self.data.push_back(item);
    }

    /// Returns the number of items currently stored.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns the maximum capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns true if empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Linearizes all stored elements in FIFO order (oldest first).
    pub fn to_vec(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.data.iter().cloned().collect()
    }

    /// Returns a slice of elements published strictly after the element matching `predicate`.
    /// If no match is found, returns None.
    pub fn replay_after<F>(&self, predicate: F) -> Option<Vec<T>>
    where
        T: Clone,
        F: Fn(&T) -> bool,
    {
        self.data
            .iter()
            .position(predicate)
            .map(|idx| self.data.iter().skip(idx + 1).cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_push_and_eviction() {
        let mut rb = RingBuffer::new(3);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        assert_eq!(rb.to_vec(), vec![1, 2, 3]);

        rb.push(4);
        assert_eq!(rb.to_vec(), vec![2, 3, 4]);
    }

    #[test]
    fn test_ring_buffer_replay_after() {
        let mut rb = RingBuffer::new(5);
        rb.push(10);
        rb.push(20);
        rb.push(30);

        let replayed = rb.replay_after(|&x| x == 10);
        assert_eq!(replayed, Some(vec![20, 30]));

        let not_found = rb.replay_after(|&x| x == 999);
        assert_eq!(not_found, None);
    }
}
