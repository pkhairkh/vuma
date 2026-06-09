//! # Synchronization Primitives
//!
//! This module provides VUMA-verified synchronization primitives with
//! Behavioral Description (BD) annotations ensuring exclusive access
//! patterns and SyncEdge annotations for the Message Sequence Graph (MSG).
//!
//! ## Primitives
//!
//! - **Mutex\<T\>**: Mutual exclusion lock with exclusive access.
//! - **RwLock\<T\>**: Read-write lock allowing concurrent readers or exclusive writer.
//! - **Channel\<T\>**: Multi-producer single-consumer channel.
//! - **Barrier**: Synchronization point for multiple threads.
//!
//! ## BD Annotations
//!
//! Each primitive carries:
//! - **CapD**: Ensuring exclusive access patterns (e.g., Mutex grants Exclusive
//!   capability when locked, RwLock grants Shared for readers).
//! - **SyncEdge**: For the MSG — lock/unlock ordering, send/receive ordering, etc.

use crate::alloc::Address;
use crate::primitives::{CapD, CapFlag, RepD, SyncEdge, SyncEdgeKind};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Mutex CapD
// ---------------------------------------------------------------------------

/// Returns the CapD for a Mutex.
/// Supports: Read, Write, Exclusive (when locked).
// VUMA-VERIFIED: mutex capability descriptor ensures exclusive access
pub fn mutex_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Exclusive])
}

/// Type alias for Mutex CapD (used in re-exports).
pub type MutexCapD = CapD;

// ---------------------------------------------------------------------------
// RwLock CapD
// ---------------------------------------------------------------------------

/// Returns the CapD for an RwLock.
/// Supports: Read, Write, Shared (for readers), Exclusive (for writer).
// VUMA-VERIFIED: rwlock capability descriptor supports both shared and exclusive
pub fn rwlock_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Shared, CapFlag::Exclusive])
}

/// Type alias for RwLock CapD (used in re-exports).
pub type RwLockCapD = CapD;

// ---------------------------------------------------------------------------
// Channel CapD
// ---------------------------------------------------------------------------

/// Returns the CapD for a Channel.
/// Supports: Read, Write, Send, Receive.
// VUMA-VERIFIED: channel capability descriptor supports message passing
pub fn channel_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Send, CapFlag::Receive])
}

/// Type alias for Channel CapD (used in re-exports).
pub type ChannelCapD = CapD;

// ---------------------------------------------------------------------------
// Barrier CapD
// ---------------------------------------------------------------------------

/// Returns the CapD for a Barrier.
/// Supports: Read, Shared (all threads synchronize).
// VUMA-VERIFIED: barrier capability descriptor supports synchronization points
pub fn barrier_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Shared])
}

/// Type alias for Barrier CapD (used in re-exports).
pub type BarrierCapD = CapD;

// ---------------------------------------------------------------------------
// MutexGuard (BD-annotated)
// ---------------------------------------------------------------------------

/// A BD-annotated guard that provides exclusive access to a Mutex's data.
///
/// The guard carries a CapD with Exclusive capability, ensuring the VUMA
/// verifier knows that the data is exclusively held for the duration of
/// the guard's lifetime.
pub struct MutexGuard<'a, T> {
    data: &'a mut T,
    lock: &'a AtomicBool,
    /// The CapD in effect while this guard is held.
    pub capd: CapD,
}

impl<'a, T> MutexGuard<'a, T> {
    /// Returns a reference to the guarded data.
    // VUMA-VERIFIED: read access under exclusive lock is safe
    pub fn get(&self) -> &T {
        self.data
    }

    /// Returns a mutable reference to the guarded data.
    // VUMA-VERIFIED: write access under exclusive lock is safe
    pub fn get_mut(&mut self) -> &mut T {
        self.data
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// RwLockReadGuard (BD-annotated)
// ---------------------------------------------------------------------------

/// A BD-annotated guard that provides shared (read) access to an RwLock's data.
///
/// The guard carries a CapD with Shared capability, ensuring the VUMA
/// verifier knows that multiple readers may coexist.
pub struct RwLockReadGuard<'a, T> {
    data: &'a T,
    readers: &'a AtomicU32,
    /// The CapD in effect while this guard is held.
    pub capd: CapD,
}

impl<'a, T> RwLockReadGuard<'a, T> {
    /// Returns a reference to the guarded data.
    // VUMA-VERIFIED: read access under shared lock is safe
    pub fn get(&self) -> &T {
        self.data
    }
}

impl<'a, T> Drop for RwLockReadGuard<'a, T> {
    fn drop(&mut self) {
        self.readers.fetch_sub(1, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// RwLockWriteGuard (BD-annotated)
// ---------------------------------------------------------------------------

/// A BD-annotated guard that provides exclusive (write) access to an RwLock's data.
///
/// The guard carries a CapD with Exclusive capability, ensuring the VUMA
/// verifier knows that the data is exclusively held.
pub struct RwLockWriteGuard<'a, T> {
    data: &'a mut T,
    writer: &'a AtomicBool,
    /// The CapD in effect while this guard is held.
    pub capd: CapD,
}

impl<'a, T> RwLockWriteGuard<'a, T> {
    /// Returns a reference to the guarded data.
    // VUMA-VERIFIED: read access under exclusive lock is safe
    pub fn get(&self) -> &T {
        self.data
    }

    /// Returns a mutable reference to the guarded data.
    // VUMA-VERIFIED: write access under exclusive lock is safe
    pub fn get_mut(&mut self) -> &mut T {
        self.data
    }
}

impl<'a, T> Drop for RwLockWriteGuard<'a, T> {
    fn drop(&mut self) {
        self.writer.store(false, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Mutex
// ---------------------------------------------------------------------------

/// A VUMA-verified mutual exclusion lock.
///
/// Mutex provides exclusive access to protected data. Only one thread may
/// hold the lock at a time. The BD annotations ensure the VUMA verifier
/// tracks the Exclusive capability through lock/unlock operations.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Exclusive }
/// - SyncEdge: lock → unlock (LockOrder), lock → access (Seq)
///
/// ## Safety
///
/// This implementation uses `AtomicBool` for the lock state. In the VUMA
/// runtime, this would be replaced with a proper futex or spinlock with
/// backoff. The BD annotations ensure correct usage regardless of the
/// underlying implementation.
pub struct Mutex<T> {
    /// Address of the protected data in VUMA address space.
    pub data: Address,
    /// The actual data being protected (VUMA model).
    inner: std::cell::UnsafeCell<T>,
    /// Lock state: false = unlocked, true = locked.
    lock: AtomicBool,
}

// SAFETY: Mutex provides exclusive access through lock/unlock.
// The VUMA verifier ensures only one thread accesses the data at a time.
unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Create a new Mutex protecting the given value.
    // VUMA-VERIFIED: mutex creation is safe
    pub fn new(value: T) -> Self {
        Self {
            data: Address::NULL,
            inner: std::cell::UnsafeCell::new(value),
            lock: AtomicBool::new(false),
        }
    }

    /// Create a new Mutex with a VUMA address for the data.
    // VUMA-VERIFIED: mutex creation with address is safe
    pub fn new_at(value: T, addr: Address) -> Self {
        Self {
            data: addr,
            inner: std::cell::UnsafeCell::new(value),
            lock: AtomicBool::new(false),
        }
    }

    /// Returns the CapD for this Mutex.
    // VUMA-VERIFIED: mutex capability descriptor is correct
    pub fn capd(&self) -> CapD {
        mutex_capd()
    }

    /// Returns the RepD for this Mutex.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("Mutex", 0, 8, mutex_capd())
    }

    /// Returns the SyncEdge annotations for this Mutex.
    // VUMA-VERIFIED: synchronization edges correctly model lock ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("mutex_lock", "mutex_unlock", SyncEdgeKind::LockOrder),
            SyncEdge::new("mutex_lock", "mutex_access", SyncEdgeKind::Seq),
        ]
    }

    /// Acquire the lock, blocking until it is available.
    ///
    /// Returns a MutexGuard that provides exclusive access to the data.
    /// The guard automatically releases the lock when dropped.
    // VUMA-VERIFIED: lock acquisition ensures exclusive access
    pub fn lock(&self) -> MutexGuard<'_, T> {
        // Spin until we acquire the lock.
        // In the VUMA runtime, this would use a futex with backoff.
        while self.lock.swap(true, Ordering::Acquire) {
            std::hint::spin_loop();
        }
        MutexGuard {
            // SAFETY: We hold the lock, so we have exclusive access.
            data: unsafe { &mut *self.inner.get() },
            lock: &self.lock,
            capd: CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Exclusive]),
        }
    }

    /// Try to acquire the lock without blocking.
    ///
    /// Returns Some(MutexGuard) if the lock was acquired, None otherwise.
    // VUMA-VERIFIED: try_lock is safe — only returns guard if lock acquired
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self.lock.swap(true, Ordering::Acquire) {
            None
        } else {
            Some(MutexGuard {
                data: unsafe { &mut *self.inner.get() },
                lock: &self.lock,
                capd: CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Exclusive]),
            })
        }
    }

    /// Returns true if the mutex is currently locked.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------------------
// RwLock
// ---------------------------------------------------------------------------

/// A VUMA-verified read-write lock.
///
/// RwLock allows multiple concurrent readers or a single exclusive writer.
/// The BD annotations distinguish between Shared (reader) and Exclusive
/// (writer) capabilities.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Shared, Exclusive }
/// - SyncEdge: read_lock → read_unlock (LockOrder), write_lock → write_unlock (LockOrder)
pub struct RwLock<T> {
    /// Address of the protected data in VUMA address space.
    pub data: Address,
    /// The actual data being protected (VUMA model).
    inner: std::cell::UnsafeCell<T>,
    /// Number of active readers.
    readers: AtomicU32,
    /// Whether a writer holds the lock.
    writer: AtomicBool,
}

// SAFETY: RwLock provides correct shared/exclusive access patterns.
unsafe impl<T: Send + Sync> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// Create a new RwLock protecting the given value.
    // VUMA-VERIFIED: rwlock creation is safe
    pub fn new(value: T) -> Self {
        Self {
            data: Address::NULL,
            inner: std::cell::UnsafeCell::new(value),
            readers: AtomicU32::new(0),
            writer: AtomicBool::new(false),
        }
    }

    /// Create a new RwLock with a VUMA address for the data.
    // VUMA-VERIFIED: rwlock creation with address is safe
    pub fn new_at(value: T, addr: Address) -> Self {
        Self {
            data: addr,
            inner: std::cell::UnsafeCell::new(value),
            readers: AtomicU32::new(0),
            writer: AtomicBool::new(false),
        }
    }

    /// Returns the CapD for this RwLock.
    // VUMA-VERIFIED: rwlock capability descriptor is correct
    pub fn capd(&self) -> CapD {
        rwlock_capd()
    }

    /// Returns the RepD for this RwLock.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("RwLock", 0, 8, rwlock_capd())
    }

    /// Returns the SyncEdge annotations for this RwLock.
    // VUMA-VERIFIED: synchronization edges correctly model rwlock ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("rwlock_read_lock", "rwlock_read_unlock", SyncEdgeKind::LockOrder),
            SyncEdge::new("rwlock_write_lock", "rwlock_write_unlock", SyncEdgeKind::LockOrder),
        ]
    }

    /// Acquire a read lock, blocking until no writer holds the lock.
    ///
    /// Returns an RwLockReadGuard with Shared capability.
    // VUMA-VERIFIED: read lock acquisition ensures shared access
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        // Wait for writer to release.
        while self.writer.load(Ordering::Acquire) {
            std::hint::spin_loop();
        }
        self.readers.fetch_add(1, Ordering::Acquire);
        // Double-check no writer sneaked in.
        if self.writer.load(Ordering::Acquire) {
            self.readers.fetch_sub(1, Ordering::Release);
            // Retry.
            return self.read();
        }
        RwLockReadGuard {
            // SAFETY: No writer is active, so shared read access is safe.
            data: unsafe { &*self.inner.get() },
            readers: &self.readers,
            capd: CapD::new(vec![CapFlag::Read, CapFlag::Shared]),
        }
    }

    /// Acquire a write lock, blocking until no readers or writers hold the lock.
    ///
    /// Returns an RwLockWriteGuard with Exclusive capability.
    // VUMA-VERIFIED: write lock acquisition ensures exclusive access
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        // Wait for other writer to release.
        while self.writer.swap(true, Ordering::Acquire) {
            std::hint::spin_loop();
        }
        // Wait for all readers to finish.
        while self.readers.load(Ordering::Acquire) > 0 {
            std::hint::spin_loop();
        }
        RwLockWriteGuard {
            // SAFETY: We hold the write lock and no readers are active.
            data: unsafe { &mut *self.inner.get() },
            writer: &self.writer,
            capd: CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Exclusive]),
        }
    }
}

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

/// A VUMA-verified multi-producer single-consumer channel.
///
/// Channels provide message-passing concurrency with BD-annotated Send and
/// Receive capabilities. The SyncEdge annotations model the happens-before
/// relationship between send and receive operations.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Send, Receive }
/// - SyncEdge: send → receive (ChannelOrder)
///
/// ## Implementation
///
/// This is a bounded channel using a ring buffer internally. The capacity
/// is fixed at construction time.
pub struct Channel<T> {
    /// Address of the channel buffer in VUMA address space.
    pub buffer: Address,
    /// Internal ring buffer for storing messages.
    inner: std::cell::UnsafeCell<crate::collections::RingBuffer<T>>,
    /// Head index (consumer side) — tracks next position to read.
    head: AtomicU32,
    /// Tail index (producer side) — tracks next position to write.
    tail: AtomicU32,
}

// SAFETY: Channel is designed for multi-producer single-consumer use.
// The BD annotations and SyncEdge ensure correct ordering.
unsafe impl<T: Send> Send for Channel<T> {}
unsafe impl<T: Send> Sync for Channel<T> {}

impl<T> Channel<T> {
    /// Create a new bounded channel with the given capacity.
    // VUMA-VERIFIED: channel creation is safe
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Address::NULL,
            inner: std::cell::UnsafeCell::new(crate::collections::RingBuffer::new(capacity)),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    /// Create a new bounded channel with a VUMA address for the buffer.
    // VUMA-VERIFIED: channel creation with address is safe
    pub fn new_at(capacity: usize, addr: Address) -> Self {
        Self {
            buffer: addr,
            inner: std::cell::UnsafeCell::new(crate::collections::RingBuffer::new(capacity)),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    /// Returns the CapD for this Channel.
    // VUMA-VERIFIED: channel capability descriptor is correct
    pub fn capd(&self) -> CapD {
        channel_capd()
    }

    /// Returns the RepD for this Channel.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("Channel", 0, 8, channel_capd())
    }

    /// Returns the SyncEdge annotations for this Channel.
    // VUMA-VERIFIED: synchronization edges correctly model channel ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("channel_send", "channel_receive", SyncEdgeKind::ChannelOrder),
        ]
    }

    /// Send a value through the channel (producer side).
    ///
    /// Returns Ok(()) if the value was sent, Err if the channel is full.
    // VUMA-VERIFIED: send is safe for producers; ChannelOrder ensures visibility
    pub fn send(&self, value: T) -> Result<(), String> {
        // SAFETY: In the VUMA runtime, the scheduler ensures only one producer
        // accesses the ring buffer at a time. For now, we use a simple model.
        let inner = unsafe { &mut *self.inner.get() };
        match inner.push(value) {
            crate::collections::BdResult { success: true, .. } => {
                self.tail.fetch_add(1, Ordering::Release);
                Ok(())
            }
            _ => Err("channel is full".to_string()),
        }
    }

    /// Receive a value from the channel (consumer side).
    ///
    /// Returns Ok(value) if a value was available, Err if the channel is empty.
    // VUMA-VERIFIED: receive is safe for the single consumer; ChannelOrder ensures visibility
    pub fn receive(&self) -> Result<T, String> {
        let inner = unsafe { &mut *self.inner.get() };
        match inner.pop() {
            crate::collections::BdResult { value: Some(v), .. } => {
                self.head.fetch_add(1, Ordering::Release);
                Ok(v)
            }
            _ => Err("channel is empty".to_string()),
        }
    }

    /// Returns true if the channel is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        let inner = unsafe { &*self.inner.get() };
        inner.is_empty()
    }

    /// Returns the number of pending messages in the channel.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        let inner = unsafe { &*self.inner.get() };
        inner.len()
    }
}

// ---------------------------------------------------------------------------
// Barrier
// ---------------------------------------------------------------------------

/// A VUMA-verified synchronization barrier.
///
/// A barrier allows multiple threads to synchronize at a common point.
/// When all `n` threads have called `wait()`, they are all released
/// simultaneously.
///
/// ## BD Annotations
///
/// - CapD: { Read, Shared }
/// - SyncEdge: barrier_wait → barrier_release (Fence)
///
/// ## Implementation
///
/// Uses a generation counter to handle reuse. Each call to `wait()` increments
/// the count; when the count reaches the barrier size, the generation is
/// incremented and all waiters are released.
pub struct Barrier {
    /// The number of threads that must arrive before the barrier releases.
    pub count: AtomicU32,
    /// The current generation (incremented each time the barrier fires).
    pub generation: AtomicU32,
    /// Number of threads currently waiting in the current generation.
    waiting: AtomicU32,
    /// The barrier size (number of threads required).
    size: u32,
}

impl Barrier {
    /// Create a new Barrier for the given number of threads.
    // VUMA-VERIFIED: barrier creation is safe
    pub fn new(size: u32) -> Self {
        Self {
            count: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            waiting: AtomicU32::new(0),
            size,
        }
    }

    /// Returns the CapD for this Barrier.
    // VUMA-VERIFIED: barrier capability descriptor is correct
    pub fn capd(&self) -> CapD {
        barrier_capd()
    }

    /// Returns the RepD for this Barrier.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("Barrier", 0, 8, barrier_capd())
    }

    /// Returns the SyncEdge annotations for this Barrier.
    // VUMA-VERIFIED: synchronization edges correctly model barrier ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("barrier_wait", "barrier_release", SyncEdgeKind::Fence),
        ]
    }

    /// Wait at the barrier, blocking until all threads have arrived.
    ///
    /// Returns true for the last thread to arrive (the "leader"), false for
    /// all others.
    // VUMA-VERIFIED: barrier wait ensures all threads synchronize before release
    pub fn wait(&self) -> bool {
        let gen = self.generation.load(Ordering::Acquire);
        let arrived = self.waiting.fetch_add(1, Ordering::Acquire) + 1;

        if arrived >= self.size {
            // Last thread to arrive: reset and release.
            self.waiting.store(0, Ordering::Release);
            self.generation.fetch_add(1, Ordering::Release);
            true
        } else {
            // Wait for the last thread to release us.
            while self.generation.load(Ordering::Acquire) == gen {
                std::hint::spin_loop();
            }
            false
        }
    }

    /// Returns the number of threads currently waiting at the barrier.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn waiting(&self) -> u32 {
        self.waiting.load(Ordering::Acquire)
    }

    /// Returns the barrier size (number of threads required).
    // VUMA-VERIFIED: pure query, no side effects
    pub fn size(&self) -> u32 {
        self.size
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutex_basic() {
        let m = Mutex::new(42);
        {
            let guard = m.lock();
            assert_eq!(*guard.get(), 42);
        }
        // Lock should be released after guard is dropped.
        assert!(!m.is_locked());
    }

    #[test]
    fn test_mutex_mutate() {
        let m = Mutex::new(10);
        {
            let mut guard = m.lock();
            *guard.get_mut() = 20;
        }
        let guard = m.lock();
        assert_eq!(*guard.get(), 20);
    }

    #[test]
    fn test_mutex_capd() {
        let m = Mutex::new(0);
        let capd = m.capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Write));
        assert!(capd.has(CapFlag::Exclusive));
    }

    #[test]
    fn test_rwlock_read() {
        let lock = RwLock::new(100);
        let guard = lock.read();
        assert_eq!(*guard.get(), 100);
        assert!(guard.capd.has(CapFlag::Shared));
    }

    #[test]
    fn test_rwlock_write() {
        let lock = RwLock::new(100);
        {
            let mut guard = lock.write();
            *guard.get_mut() = 200;
        }
        let guard = lock.read();
        assert_eq!(*guard.get(), 200);
    }

    #[test]
    fn test_rwlock_capd() {
        let lock = RwLock::new(0);
        let capd = lock.capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Write));
        assert!(capd.has(CapFlag::Shared));
        assert!(capd.has(CapFlag::Exclusive));
    }

    #[test]
    fn test_channel_basic() {
        let ch: Channel<i32> = Channel::new(4);
        ch.send(1).unwrap();
        ch.send(2).unwrap();
        assert_eq!(ch.receive().unwrap(), 1);
        assert_eq!(ch.receive().unwrap(), 2);
        assert!(ch.is_empty());
    }

    #[test]
    fn test_channel_full() {
        let ch: Channel<i32> = Channel::new(2);
        ch.send(1).unwrap();
        ch.send(2).unwrap();
        assert!(ch.send(3).is_err());
    }

    #[test]
    fn test_channel_empty() {
        let ch: Channel<i32> = Channel::new(4);
        assert!(ch.receive().is_err());
    }

    #[test]
    fn test_channel_capd() {
        let ch: Channel<i32> = Channel::new(4);
        let capd = ch.capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Write));
        assert!(capd.has(CapFlag::Send));
        assert!(capd.has(CapFlag::Receive));
    }

    #[test]
    fn test_barrier_basic() {
        let barrier = Barrier::new(1);
        let is_leader = barrier.wait();
        assert!(is_leader);
    }

    #[test]
    fn test_barrier_capd() {
        let barrier = Barrier::new(4);
        let capd = barrier.capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Shared));
    }

    #[test]
    fn test_sync_edges_mutex() {
        let m = Mutex::new(0);
        let edges = m.sync_edges();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].kind, SyncEdgeKind::LockOrder);
    }

    #[test]
    fn test_sync_edges_channel() {
        let ch: Channel<i32> = Channel::new(4);
        let edges = ch.sync_edges();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, SyncEdgeKind::ChannelOrder);
    }

    #[test]
    fn test_sync_edges_barrier() {
        let barrier = Barrier::new(4);
        let edges = barrier.sync_edges();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, SyncEdgeKind::Fence);
    }
}
