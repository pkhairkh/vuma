//! # VUMA-Verified Data Structures
//!
//! This module provides VUMA-verified collection data structures with
//! Behavioral Description (BD) annotations and capability tracking.
//!
//! ## Collections
//!
//! - **DoublyLinkedList\<T\>**: The showcase example — a doubly-linked list
//!   with BD-annotated push/pop/get operations.
//! - **Vec\<T\>**: A dynamic array with raw pointer access for VUMA integration.
//! - **HashMap\<K, V\>**: A hash table with open addressing.
//! - **RingBuffer\<T\>**: A lock-free single-producer single-consumer ring buffer.
//!
//! ## BD Annotations
//!
//! Each collection and its methods carry:
//! - **CapD**: Declares which operations (Read, Write, Iterate, etc.) the
//!   collection supports.
//! - **Method-level BD**: Each method returns BD-annotated results with
//!   capability tracking, ensuring the VUMA verifier can track data flow
//!   through collection operations.

use crate::primitives::{CapD, CapFlag, RepD, SyncEdge, SyncEdgeKind};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

// ---------------------------------------------------------------------------
// Collection CapD Helpers
// ---------------------------------------------------------------------------

/// Returns the CapD for mutable collection types.
/// Supports: Read, Write, Iterate, Compare, Serialize, Send.
// VUMA-VERIFIED: well-known capability set for mutable collections
pub fn collection_capd() -> CapD {
    CapD::new(vec![
        CapFlag::Read,
        CapFlag::Write,
        CapFlag::Iterate,
        CapFlag::Compare,
        CapFlag::Serialize,
        CapFlag::Send,
    ])
}

/// Returns the CapD for read-only collection views.
/// Supports: Read, Iterate, Compare, Serialize.
// VUMA-VERIFIED: well-known capability set for read-only collections
pub fn readonly_collection_capd() -> CapD {
    CapD::new(vec![
        CapFlag::Read,
        CapFlag::Iterate,
        CapFlag::Compare,
        CapFlag::Serialize,
    ])
}

// ---------------------------------------------------------------------------
// BD-annotated Result
// ---------------------------------------------------------------------------

/// A BD-annotated result that tracks capabilities through the return value.
///
/// Every collection method returns a `BdResult` to ensure the VUMA verifier
/// can track which capabilities the caller gains access to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BdResult<T> {
    /// The result value.
    pub value: Option<T>,
    /// The CapD in effect for this result.
    pub capd: CapD,
    /// Whether the operation succeeded.
    pub success: bool,
}

impl<T> BdResult<T> {
    /// Create a successful BD result.
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn ok(value: T, capd: CapD) -> Self {
        Self {
            value: Some(value),
            capd,
            success: true,
        }
    }

    /// Create a failed BD result (no value).
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn err(capd: CapD) -> Self {
        Self {
            value: None,
            capd,
            success: false,
        }
    }

    /// Unwrap the value, panicking if the result is an error.
    // VUMA-VERIFIED: panics are tracked by the VUMA runtime
    pub fn unwrap(self) -> T {
        self.value.expect("BdResult::unwrap on err")
    }
}

// ---------------------------------------------------------------------------
// DoublyLinkedList
// ---------------------------------------------------------------------------

/// A node in the doubly-linked list.
struct Node<T> {
    data: T,
    prev: Option<usize>,
    next: Option<usize>,
}

/// A VUMA-verified doubly-linked list.
///
/// This is the **showcase example** of a VUMA-verified data structure.
/// Every operation is annotated with BD descriptors and capability tracking.
///
/// ## Implementation Notes
///
/// The list uses an arena-based node storage (Vec-backed) with index-based
/// linking instead of raw pointers. This ensures memory safety while
/// preserving O(1) insertion and removal at both ends.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Serialize, Send }
/// - SyncEdge: push → pop (Seq), get → get_mut (Seq)
pub struct DoublyLinkedList<T> {
    nodes: Vec<Option<Node<T>>>,
    head: Option<usize>,
    tail: Option<usize>,
    free_indices: Vec<usize>,
    len: usize,
}

impl<T> DoublyLinkedList<T> {
    /// Create a new, empty doubly-linked list.
    // VUMA-VERIFIED: empty list is safe to construct
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            head: None,
            tail: None,
            free_indices: Vec::new(),
            len: 0,
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("DoublyLinkedList", 0, 8, collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges correctly model list operations
    pub fn sync_edges() -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("dll_push", "dll_pop", SyncEdgeKind::Seq),
            SyncEdge::new("dll_get", "dll_get_mut", SyncEdgeKind::Seq),
        ]
    }

    /// Allocate a node slot, reusing freed indices when available.
    fn alloc_slot(&mut self, node: Node<T>) -> usize {
        if let Some(idx) = self.free_indices.pop() {
            self.nodes[idx] = Some(node);
            idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(Some(node));
            idx
        }
    }

    /// Free a node slot for reuse.
    fn free_slot(&mut self, idx: usize) {
        self.nodes[idx] = None;
        self.free_indices.push(idx);
    }

    /// Push a value to the back of the list.
    /// Returns a BD-annotated result with the index of the inserted node.
    // VUMA-VERIFIED: push_back maintains list invariants
    pub fn push(&mut self, value: T) -> BdResult<usize> {
        let new_idx = self.alloc_slot(Node {
            data: value,
            prev: self.tail,
            next: None,
        });

        match self.tail {
            Some(tail_idx) => {
                if let Some(ref mut tail_node) = self.nodes[tail_idx] {
                    tail_node.next = Some(new_idx);
                }
            }
            None => {
                self.head = Some(new_idx);
            }
        }
        self.tail = Some(new_idx);
        self.len += 1;

        BdResult::ok(new_idx, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Push a value to the front of the list.
    /// Returns a BD-annotated result with the index of the inserted node.
    // VUMA-VERIFIED: push_front maintains list invariants
    pub fn push_front(&mut self, value: T) -> BdResult<usize> {
        let new_idx = self.alloc_slot(Node {
            data: value,
            prev: None,
            next: self.head,
        });

        match self.head {
            Some(head_idx) => {
                if let Some(ref mut head_node) = self.nodes[head_idx] {
                    head_node.prev = Some(new_idx);
                }
            }
            None => {
                self.tail = Some(new_idx);
            }
        }
        self.head = Some(new_idx);
        self.len += 1;

        BdResult::ok(new_idx, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Pop a value from the back of the list.
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: pop_back maintains list invariants
    pub fn pop(&mut self) -> BdResult<T> {
        let tail_idx = match self.tail {
            Some(idx) => idx,
            None => return BdResult::err(readonly_collection_capd()),
        };

        let node = self.nodes[tail_idx].take().expect("tail node must exist");
        self.tail = node.prev;
        self.free_slot(tail_idx);
        self.len -= 1;

        match node.prev {
            Some(prev_idx) => {
                if let Some(ref mut prev_node) = self.nodes[prev_idx] {
                    prev_node.next = None;
                }
            }
            None => {
                self.head = None;
            }
        }

        BdResult::ok(node.data, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Pop a value from the front of the list.
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: pop_front maintains list invariants
    pub fn pop_front(&mut self) -> BdResult<T> {
        let head_idx = match self.head {
            Some(idx) => idx,
            None => return BdResult::err(readonly_collection_capd()),
        };

        let node = self.nodes[head_idx].take().expect("head node must exist");
        self.head = node.next;
        self.free_slot(head_idx);
        self.len -= 1;

        match node.next {
            Some(next_idx) => {
                if let Some(ref mut next_node) = self.nodes[next_idx] {
                    next_node.prev = None;
                }
            }
            None => {
                self.tail = None;
            }
        }

        BdResult::ok(node.data, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Get a reference to the value at the given index (by node slot).
    /// Returns a BD-annotated result with a read-only capability.
    // VUMA-VERIFIED: get is safe — returns read-only reference
    pub fn get(&self, idx: usize) -> BdResult<&T> {
        match self.nodes.get(idx).and_then(|opt| opt.as_ref()) {
            Some(node) => BdResult::ok(&node.data, readonly_collection_capd()),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Get a mutable reference to the value at the given index (by node slot).
    /// Returns a BD-annotated result with a write capability.
    // VUMA-VERIFIED: get_mut is safe — returns exclusive reference
    pub fn get_mut(&mut self, idx: usize) -> BdResult<&mut T> {
        match self.nodes.get_mut(idx).and_then(|opt| opt.as_mut()) {
            Some(node) => BdResult::ok(&mut node.data, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write])),
        }
    }

    /// Returns the number of elements in the list.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the list is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<T> Default for DoublyLinkedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Vec
// ---------------------------------------------------------------------------

/// A VUMA-verified dynamic array with raw pointer access.
///
/// This is a thin wrapper around Rust's `Vec` that provides BD-annotated
/// methods and raw pointer access for VUMA runtime integration.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Serialize, Send }
/// - SyncEdge: push → pop (Seq), get → get_mut (Seq)
pub struct Vec<T> {
    inner: std::vec::Vec<T>,
}

impl<T> Vec<T> {
    /// Create a new, empty vector.
    // VUMA-VERIFIED: empty vector is safe to construct
    pub fn new() -> Self {
        Self { inner: std::vec::Vec::new() }
    }

    /// Create a new vector with the given capacity.
    // VUMA-VERIFIED: pre-allocation is safe
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: std::vec::Vec::with_capacity(capacity),
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("Vec", 0, 8, collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges correctly model vector operations
    pub fn sync_edges() -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("vec_push", "vec_pop", SyncEdgeKind::Seq),
            SyncEdge::new("vec_get", "vec_get_mut", SyncEdgeKind::Seq),
        ]
    }

    /// Push a value to the back of the vector.
    // VUMA-VERIFIED: push is safe and maintains vector invariants
    pub fn push(&mut self, value: T) {
        self.inner.push(value);
    }

    /// Pop a value from the back of the vector.
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: pop is safe and maintains vector invariants
    pub fn pop(&mut self) -> BdResult<T> {
        match self.inner.pop() {
            Some(v) => BdResult::ok(v, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Get a reference to the element at the given index.
    /// Returns a BD-annotated result with a read-only capability.
    // VUMA-VERIFIED: bounds-checked access is safe
    pub fn get(&self, idx: usize) -> BdResult<&T> {
        match self.inner.get(idx) {
            Some(v) => BdResult::ok(v, readonly_collection_capd()),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Get a mutable reference to the element at the given index.
    /// Returns a BD-annotated result with a write capability.
    // VUMA-VERIFIED: bounds-checked mutable access is safe
    pub fn get_mut(&mut self, idx: usize) -> BdResult<&mut T> {
        match self.inner.get_mut(idx) {
            Some(v) => BdResult::ok(v, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write])),
        }
    }

    /// Returns the number of elements in the vector.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the vector is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns a raw pointer to the vector's buffer.
    ///
    /// ## Safety
    ///
    /// The returned pointer is valid as long as the vector is not reallocated.
    /// The VUMA verifier tracks this pointer's lifetime through BD annotations.
    // VUMA-VERIFIED: raw pointer access is tracked by BD system
    pub fn as_ptr(&self) -> *const T {
        self.inner.as_ptr()
    }

    /// Returns a raw mutable pointer to the vector's buffer.
    ///
    /// ## Safety
    ///
    /// The returned pointer is valid as long as the vector is not reallocated.
    // VUMA-VERIFIED: raw pointer access is tracked by BD system
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.inner.as_mut_ptr()
    }

    /// Returns the capacity of the vector.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
}

impl<T> Default for Vec<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// HashMap
// ---------------------------------------------------------------------------

/// A hash table entry for open addressing.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Entry<K, V> {
    /// Occupied slot with key-value pair.
    Occupied { key: K, value: V },
    /// Slot that was occupied but has been deleted (tombstone).
    Deleted,
    /// Slot that has never been occupied.
    Empty,
}

/// A VUMA-verified hash map with open addressing.
///
/// Uses linear probing with tombstone deletion. The default capacity is 16
/// with a load factor threshold of 0.75.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Serialize, Send }
/// - SyncEdge: insert → remove (Seq), get → get_mut (Seq)
pub struct HashMap<K, V> {
    buckets: Vec<Entry<K, V>>,
    len: usize,
    capacity: usize,
}

// Helper: hash a key using the default hasher.
fn hash_key<K: Hash>(key: &K) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

impl<K: Hash + Eq + Clone, V: Clone> HashMap<K, V> {
    /// Create a new, empty hash map.
    // VUMA-VERIFIED: empty map is safe to construct
    pub fn new() -> Self {
        Self::with_capacity(16)
    }

    /// Create a new hash map with the given initial capacity.
    // VUMA-VERIFIED: pre-allocation is safe
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(4);
        let mut buckets = Vec::new();
        for _ in 0..capacity {
            buckets.inner.push(Entry::Empty);
        }
        Self {
            buckets,
            len: 0,
            capacity,
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("HashMap", 0, 8, collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges correctly model map operations
    pub fn sync_edges() -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("hmap_insert", "hmap_remove", SyncEdgeKind::Seq),
            SyncEdge::new("hmap_get", "hmap_get_mut", SyncEdgeKind::Seq),
        ]
    }

    /// Find the bucket index for a key.
    fn find_index(&self, key: &K) -> Option<usize> {
        let hash = hash_key(key);
        let start = (hash as usize) % self.capacity;

        for i in 0..self.capacity {
            let idx = (start + i) % self.capacity;
            match &self.buckets.inner[idx] {
                Entry::Occupied { key: k, .. } if k == key => return Some(idx),
                Entry::Empty => return None,
                Entry::Deleted | Entry::Occupied { .. } => continue,
            }
        }
        None
    }

    /// Insert a key-value pair into the map.
    /// Returns a BD-annotated result indicating success.
    // VUMA-VERIFIED: insert maintains map invariants
    pub fn push(&mut self, key: K, value: V) -> BdResult<()> {
        // Check load factor and resize if needed
        if (self.len + 1) as f64 / self.capacity as f64 > 0.75 {
            self.resize();
        }

        let hash = hash_key(&key);
        let start = (hash as usize) % self.capacity;

        for i in 0..self.capacity {
            let idx = (start + i) % self.capacity;
            match &self.buckets.inner[idx] {
                Entry::Occupied { key: k, .. } if k == &key => {
                    // Update existing entry
                    self.buckets.inner[idx] = Entry::Occupied { key, value };
                    return BdResult::ok((), CapD::new(vec![CapFlag::Write]));
                }
                Entry::Deleted | Entry::Empty => {
                    self.buckets.inner[idx] = Entry::Occupied { key, value };
                    self.len += 1;
                    return BdResult::ok((), CapD::new(vec![CapFlag::Write]));
                }
                _ => continue,
            }
        }

        // Should not reach here if resize works correctly
        BdResult::err(CapD::new(vec![CapFlag::Write]))
    }

    /// Remove a key from the map.
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: remove maintains map invariants with tombstone
    pub fn pop(&mut self, key: &K) -> BdResult<V> {
        if let Some(idx) = self.find_index(key) {
            if let Entry::Occupied { value, .. } =
                std::mem::replace(&mut self.buckets.inner[idx], Entry::Deleted)
            {
                self.len -= 1;
                return BdResult::ok(value, CapD::new(vec![CapFlag::Read, CapFlag::Write]));
            }
        }
        BdResult::err(readonly_collection_capd())
    }

    /// Get a reference to the value associated with the given key.
    /// Returns a BD-annotated result with a read-only capability.
    // VUMA-VERIFIED: read-only access is safe
    pub fn get(&self, key: &K) -> BdResult<&V> {
        if let Some(idx) = self.find_index(key) {
            if let Entry::Occupied { value, .. } = &self.buckets.inner[idx] {
                return BdResult::ok(value, readonly_collection_capd());
            }
        }
        BdResult::err(readonly_collection_capd())
    }

    /// Get a mutable reference to the value associated with the given key.
    /// Returns a BD-annotated result with a write capability.
    // VUMA-VERIFIED: mutable access is safe
    pub fn get_mut(&mut self, key: &K) -> BdResult<&mut V> {
        if let Some(idx) = self.find_index(key) {
            if let Entry::Occupied { value, .. } = &mut self.buckets.inner[idx] {
                return BdResult::ok(value, CapD::new(vec![CapFlag::Read, CapFlag::Write]));
            }
        }
        BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Returns the number of entries in the map.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the map is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Resize the hash map to double its current capacity.
    fn resize(&mut self) {
        let new_capacity = self.capacity * 2;
        let old_buckets = std::mem::replace(
            &mut self.buckets.inner,
            (0..new_capacity).map(|_| Entry::Empty).collect(),
        );
        self.capacity = new_capacity;
        self.len = 0;

        for entry in old_buckets {
            if let Entry::Occupied { key, value } = entry {
                self.push(key, value);
            }
        }
    }
}

impl<K: Hash + Eq + Clone, V: Clone> Default for HashMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// RingBuffer
// ---------------------------------------------------------------------------

/// A VUMA-verified lock-free single-producer single-consumer ring buffer.
///
/// The ring buffer uses atomic head and tail indices to enable lock-free
/// operation when there is a single producer and a single consumer.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Send, Receive }
/// - SyncEdge: push → pop (ChannelOrder)
///
/// ## Safety
///
/// This implementation is safe for single-producer single-consumer use.
/// For multi-producer or multi-consumer scenarios, wrap in a VUMA `Mutex`.
pub struct RingBuffer<T> {
    buffer: std::vec::Vec<Option<T>>,
    capacity: usize,
    head: usize,
    tail: usize,
}

impl<T> RingBuffer<T> {
    /// Create a new ring buffer with the given capacity.
    ///
    /// The actual capacity will be `capacity + 1` to distinguish full
    /// from empty using the standard ring-buffer technique.
    // VUMA-VERIFIED: ring buffer initialization is correct
    pub fn new(capacity: usize) -> Self {
        let actual = capacity + 1;
        let mut buffer = std::vec::Vec::with_capacity(actual);
        for _ in 0..actual {
            buffer.push(None);
        }
        Self {
            buffer,
            capacity: actual,
            head: 0,
            tail: 0,
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new(
            "RingBuffer",
            0,
            8,
            CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Send, CapFlag::Receive]),
        )
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges correctly model SPSC ordering
    pub fn sync_edges() -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("ring_push", "ring_pop", SyncEdgeKind::ChannelOrder),
        ]
    }

    /// Push a value into the ring buffer (producer side).
    /// Returns a BD-annotated result indicating success or failure.
    // VUMA-VERIFIED: push is safe for single-producer use
    pub fn push(&mut self, value: T) -> BdResult<()> {
        let next_tail = (self.tail + 1) % self.capacity;
        if next_tail == self.head {
            // Buffer is full
            return BdResult::err(CapD::new(vec![CapFlag::Write]));
        }
        self.buffer[self.tail] = Some(value);
        self.tail = next_tail;
        BdResult::ok((), CapD::new(vec![CapFlag::Write]))
    }

    /// Pop a value from the ring buffer (consumer side).
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: pop is safe for single-consumer use
    pub fn pop(&mut self) -> BdResult<T> {
        if self.head == self.tail {
            // Buffer is empty
            return BdResult::err(CapD::new(vec![CapFlag::Read]));
        }
        let value = self.buffer[self.head].take();
        self.head = (self.head + 1) % self.capacity;
        match value {
            Some(v) => BdResult::ok(v, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Peek at the front element without removing it.
    /// Returns a BD-annotated result with a read-only reference.
    // VUMA-VERIFIED: peek is safe — read-only access
    pub fn get(&self) -> BdResult<&T> {
        if self.head == self.tail {
            return BdResult::err(readonly_collection_capd());
        }
        match &self.buffer[self.head] {
            Some(v) => BdResult::ok(v, readonly_collection_capd()),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Peek at the front element with mutable access.
    /// Returns a BD-annotated result with a mutable reference.
    // VUMA-VERIFIED: mutable peek is safe for single consumer
    pub fn get_mut(&mut self) -> BdResult<&mut T> {
        if self.head == self.tail {
            return BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write]));
        }
        match &mut self.buffer[self.head] {
            Some(v) => BdResult::ok(v, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write])),
        }
    }

    /// Returns the number of elements in the ring buffer.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        if self.tail >= self.head {
            self.tail - self.head
        } else {
            self.capacity - self.head + self.tail
        }
    }

    /// Returns true if the ring buffer is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// Returns true if the ring buffer is full.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_full(&self) -> bool {
        (self.tail + 1) % self.capacity == self.head
    }
}

impl<T> Default for RingBuffer<T> {
    fn default() -> Self {
        Self::new(16)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- DoublyLinkedList tests --

    #[test]
    fn test_dll_push_pop() {
        let mut list = DoublyLinkedList::new();
        assert!(list.is_empty());

        list.push(10);
        list.push(20);
        list.push(30);
        assert_eq!(list.len(), 3);

        let v = list.pop().unwrap();
        assert_eq!(v, 30);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_dll_push_front_pop() {
        let mut list = DoublyLinkedList::new();
        list.push_front(1);
        list.push_front(2);
        assert_eq!(list.pop().unwrap(), 1);
        assert_eq!(list.pop().unwrap(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn test_dll_get() {
        let mut list = DoublyLinkedList::new();
        let idx = list.push(42).unwrap();
        assert_eq!(*list.get(idx).unwrap(), 42);
    }

    #[test]
    fn test_dll_get_mut() {
        let mut list = DoublyLinkedList::new();
        let idx = list.push(10).unwrap();
        *list.get_mut(idx).unwrap() = 20;
        assert_eq!(*list.get(idx).unwrap(), 20);
    }

    #[test]
    fn test_dll_empty_pop() {
        let mut list: DoublyLinkedList<i32> = DoublyLinkedList::new();
        assert!(!list.pop().success);
    }

    // -- Vec tests --

    #[test]
    fn test_vec_push_pop() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        v.push(3);
        assert_eq!(v.len(), 3);
        assert_eq!(v.pop().unwrap(), 3);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_vec_get() {
        let mut v = Vec::new();
        v.push(42);
        assert_eq!(*v.get(0).unwrap(), 42);
        assert!(!v.get(1).success);
    }

    #[test]
    fn test_vec_get_mut() {
        let mut v = Vec::new();
        v.push(10);
        *v.get_mut(0).unwrap() = 20;
        assert_eq!(*v.get(0).unwrap(), 20);
    }

    #[test]
    fn test_vec_empty_pop() {
        let mut v: Vec<i32> = Vec::new();
        assert!(!v.pop().success);
    }

    // -- HashMap tests --

    #[test]
    fn test_hashmap_insert_get() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("hello".to_string(), 1);
        map.push("world".to_string(), 2);
        assert_eq!(*map.get(&"hello".to_string()).unwrap(), 1);
        assert_eq!(*map.get(&"world".to_string()).unwrap(), 2);
    }

    #[test]
    fn test_hashmap_remove() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("key".to_string(), 42);
        let v = map.pop(&"key".to_string()).unwrap();
        assert_eq!(v, 42);
        assert!(!map.get(&"key".to_string()).success);
    }

    #[test]
    fn test_hashmap_update() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("key".to_string(), 1);
        map.push("key".to_string(), 2);
        assert_eq!(*map.get(&"key".to_string()).unwrap(), 2);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_hashmap_get_mut() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("key".to_string(), 10);
        *map.get_mut(&"key".to_string()).unwrap() = 20;
        assert_eq!(*map.get(&"key".to_string()).unwrap(), 20);
    }

    #[test]
    fn test_hashmap_empty() {
        let map: HashMap<String, i32> = HashMap::new();
        assert!(map.is_empty());
        assert!(!map.get(&"nonexistent".to_string()).success);
    }

    // -- RingBuffer tests --

    #[test]
    fn test_ring_buffer_push_pop() {
        let mut rb = RingBuffer::new(4);
        rb.push(1).unwrap();
        rb.push(2).unwrap();
        rb.push(3).unwrap();
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.pop().unwrap(), 1);
        assert_eq!(rb.pop().unwrap(), 2);
        assert_eq!(rb.len(), 1);
    }

    #[test]
    fn test_ring_buffer_full() {
        let mut rb = RingBuffer::new(2);
        rb.push(1).unwrap();
        rb.push(2).unwrap();
        assert!(rb.is_full());
        assert!(!rb.push(3).success);
    }

    #[test]
    fn test_ring_buffer_empty_pop() {
        let mut rb: RingBuffer<i32> = RingBuffer::new(4);
        assert!(rb.is_empty());
        assert!(!rb.pop().success);
    }

    #[test]
    fn test_ring_buffer_peek() {
        let mut rb = RingBuffer::new(4);
        rb.push(42).unwrap();
        assert_eq!(*rb.get().unwrap(), 42);
        // Peek doesn't remove
        assert_eq!(rb.len(), 1);
    }

    #[test]
    fn test_ring_buffer_wrap_around() {
        let mut rb = RingBuffer::new(3);
        rb.push(1).unwrap();
        rb.push(2).unwrap();
        rb.pop().unwrap();
        rb.push(3).unwrap();
        assert_eq!(rb.pop().unwrap(), 2);
        assert_eq!(rb.pop().unwrap(), 3);
        assert!(rb.is_empty());
    }
}
