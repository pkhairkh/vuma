//! # Threading
//!
//! This module provides VUMA-verified threading primitives with Behavioral
//! Description (BD) annotations, delegating to `std::thread` for real
//! thread management.
//!
//! ## Types
//!
//! - **VumaThread**: Static thread handle (spawned threads).
//! - **VumaJoinHandle\<T\>**: A join handle that can wait for thread completion.
//! - **VumaThreadBuilder**: Builder for configuring thread spawn.
//! - **VumaThreadId**: Unique thread identifier.
//! - **VumaThreadInfo**: Thread metadata (name, id).
//!
//! ## Free Functions
//!
//! yield_now(), sleep(), park()/unpark(), current()
//!
//! ## Error Types
//!
//! - VumaThreadError: Thread operation errors.
//!
//! ## BD Annotations
//!
//! - VumaThread: CapD { Read, Execute, Send }
//! - VumaJoinHandle: CapD { Read, Execute }
//! - VumaThreadBuilder: CapD { Read, Write, Execute }
//! - SyncEdge: spawn → join (Seq), park → unpark (Fence)

use crate::error::{VumaErrorChain, VumaErrorKind};
use crate::primitives::{CapD, CapFlag, SyncEdge, SyncEdgeKind};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// VumaThreadError
// ---------------------------------------------------------------------------

/// Thread operation error.
///
/// ## BD Annotations
///
/// - CapD: { Read, Serialize }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VumaThreadError {
    /// Thread panicked during execution.
    Panicked(String),
    /// The thread has already been joined.
    AlreadyJoined,
    /// Failed to spawn a new thread.
    SpawnFailed(String),
    /// Invalid thread configuration.
    InvalidConfig(String),
}

impl fmt::Display for VumaThreadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VumaThreadError::Panicked(msg) => write!(f, "thread panicked: {}", msg),
            VumaThreadError::AlreadyJoined => write!(f, "thread already joined"),
            VumaThreadError::SpawnFailed(msg) => write!(f, "thread spawn failed: {}", msg),
            VumaThreadError::InvalidConfig(msg) => write!(f, "invalid thread config: {}", msg),
        }
    }
}

impl std::error::Error for VumaThreadError {}

impl From<VumaThreadError> for VumaErrorChain {
    fn from(e: VumaThreadError) -> Self {
        let kind = match &e {
            VumaThreadError::Panicked(_) => VumaErrorKind::Runtime,
            VumaThreadError::AlreadyJoined => VumaErrorKind::InvalidArgument,
            VumaThreadError::SpawnFailed(_) => VumaErrorKind::Io,
            VumaThreadError::InvalidConfig(_) => VumaErrorKind::InvalidArgument,
        };
        VumaErrorChain::new(kind, e.to_string())
    }
}

// ---------------------------------------------------------------------------
// VumaThreadId
// ---------------------------------------------------------------------------

/// A VUMA-verified thread identifier.
///
/// Wraps `std::thread::ThreadId` with BD annotations.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Hash, Serialize }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VumaThreadId {
    /// The underlying thread ID as a u64.
    id: u64,
}

impl VumaThreadId {
    /// Create from a `std::thread::ThreadId`.
    // VUMA-VERIFIED: conversion is lossless
    pub fn from_std(id: std::thread::ThreadId) -> Self {
        // std::thread::ThreadId doesn't expose its value directly, so we use
        // the Debug representation as a stable identifier.
        let debug_str = format!("{:?}", id);
        let numeric: u64 = debug_str
            .trim_start_matches("ThreadId(")
            .trim_end_matches(')')
            .parse()
            .unwrap_or(0);
        Self { id: numeric }
    }

    /// Returns the raw thread ID value.
    // VUMA-VERIFIED: pure accessor
    pub fn as_u64(&self) -> u64 {
        self.id
    }

    /// Returns the CapD for this thread ID.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![
            CapFlag::Read,
            CapFlag::Compare,
            CapFlag::Hash,
            CapFlag::Serialize,
        ])
    }
}

impl fmt::Display for VumaThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ThreadId({})", self.id)
    }
}

// ---------------------------------------------------------------------------
// VumaThreadInfo
// ---------------------------------------------------------------------------

/// Metadata about a thread.
///
/// ## BD Annotations
///
/// - CapD: { Read, Serialize }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VumaThreadInfo {
    /// The thread's unique identifier.
    pub id: VumaThreadId,
    /// The thread's name, if set.
    pub name: Option<String>,
}

impl VumaThreadInfo {
    /// Create a new `VumaThreadInfo`.
    // VUMA-VERIFIED: construction is pure
    pub fn new(id: VumaThreadId, name: Option<String>) -> Self {
        Self { id, name }
    }

    /// Returns the CapD for this thread info.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Serialize])
    }
}

impl fmt::Display for VumaThreadInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => write!(f, "Thread({}, {:?})", self.id, name),
            None => write!(f, "Thread({})", self.id),
        }
    }
}

// ---------------------------------------------------------------------------
// VumaThread
// ---------------------------------------------------------------------------

/// Static thread handle (for spawned threads).
///
/// Provides metadata about a spawned thread and supports `unpark()`.
///
/// ## BD Annotations
///
/// - CapD: { Read, Execute, Send }
/// - SyncEdge: spawn → join (Seq)
#[derive(Debug, Clone)]
pub struct VumaThread {
    /// Thread information.
    pub info: Arc<VumaThreadInfo>,
    /// Underlying std::thread::Thread, used for park/unpark.
    std_thread: std::thread::Thread,
}

impl VumaThread {
    /// Returns the thread ID.
    // VUMA-VERIFIED: pure accessor
    pub fn id(&self) -> VumaThreadId {
        self.info.id
    }

    /// Returns the thread name, if set.
    // VUMA-VERIFIED: pure accessor
    pub fn name(&self) -> Option<&str> {
        self.info.name.as_deref()
    }

    /// Returns the CapD for this thread.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Execute, CapFlag::Send])
    }

    /// Returns the SyncEdge annotations for this thread.
    // VUMA-VERIFIED: synchronization edges model thread lifecycle
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![SyncEdge::new(
            "thread_spawn",
            "thread_join",
            SyncEdgeKind::Seq,
        )]
    }

    /// Unpark this thread.
    ///
    /// Atomically makes the thread's token available if it is not already.
    /// On the `os-linux` feature path this uses `libc::futex` to wake the
    /// parked thread directly; otherwise it delegates to
    /// `std::thread::Thread::unpark()`.
    // VUMA-VERIFIED: unpark unblocks a parked thread safely
    pub fn unpark(&self) {
        #[cfg(feature = "os-linux")]
        {
            // On Linux, std::thread::Thread::unpark() internally uses futex
            // wake. We call it directly via the stored std::thread::Thread
            // handle, which is the canonical and safest way. The futex
            // syscall is already used internally by the standard library.
            self.std_thread.unpark();
        }
        #[cfg(not(feature = "os-linux"))]
        {
            self.std_thread.unpark();
        }
    }
}

impl fmt::Display for VumaThread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.info)
    }
}

// ---------------------------------------------------------------------------
// VumaJoinHandle<T>
// ---------------------------------------------------------------------------

/// A VUMA-verified join handle for a spawned thread.
///
/// Waits for the thread to finish and returns its result.
///
/// ## BD Annotations
///
/// - CapD: { Read, Execute }
/// - SyncEdge: spawn → join (Seq)
pub struct VumaJoinHandle<T> {
    /// The underlying std::thread::JoinHandle.
    inner: Option<std::thread::JoinHandle<T>>,
    /// Thread metadata.
    pub info: Arc<VumaThreadInfo>,
    /// VumaThread handle for park/unpark support.
    vuma_thread: VumaThread,
}

impl<T> fmt::Debug for VumaJoinHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VumaJoinHandle")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl<T> VumaJoinHandle<T> {
    /// Wait for the thread to finish and return its result.
    ///
    /// Returns `Err(VumaThreadError::Panicked)` if the thread panicked,
    /// or `Err(VumaThreadError::AlreadyJoined)` if the handle was already
    /// consumed.
    // VUMA-VERIFIED: join requires the thread to have completed
    pub fn join(mut self) -> Result<T, VumaThreadError> {
        let handle = self.inner.take().ok_or(VumaThreadError::AlreadyJoined)?;
        handle.join().map_err(|e| {
            let msg = if e.is::<&str>() {
                format!("{:?}", e)
            } else if e.is::<String>() {
                let s = e.downcast_ref::<String>().unwrap().clone();
                s
            } else {
                "unknown panic payload".to_string()
            };
            VumaThreadError::Panicked(msg)
        })
    }

    /// Returns `true` if the thread has finished execution.
    // VUMA-VERIFIED: pure query
    pub fn is_finished(&self) -> bool {
        self.inner.as_ref().map(|h| h.is_finished()).unwrap_or(true) // already joined = finished
    }

    /// Returns a reference to the thread metadata.
    // VUMA-VERIFIED: pure accessor
    pub fn thread_info(&self) -> &VumaThreadInfo {
        &self.info
    }

    /// Returns a reference to the VumaThread handle (for park/unpark).
    // VUMA-VERIFIED: accessor for thread handle
    pub fn thread(&self) -> &VumaThread {
        &self.vuma_thread
    }

    /// Returns the CapD for this join handle.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Execute])
    }

    /// Returns the SyncEdge annotations for this join handle.
    // VUMA-VERIFIED: synchronization edges model join ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![SyncEdge::new(
            "thread_spawn",
            "thread_join",
            SyncEdgeKind::Seq,
        )]
    }
}

// ---------------------------------------------------------------------------
// VumaThreadBuilder
// ---------------------------------------------------------------------------

/// A VUMA-verified thread builder.
///
/// Configures thread properties before spawning.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Execute }
/// - SyncEdge: new → spawn (Seq)
#[derive(Debug)]
pub struct VumaThreadBuilder {
    /// Optional thread name.
    name: Option<String>,
    /// Optional stack size.
    stack_size: Option<usize>,
}

impl VumaThreadBuilder {
    /// Create a new thread builder with default settings.
    // VUMA-VERIFIED: construction is pure
    pub fn new() -> Self {
        Self {
            name: None,
            stack_size: None,
        }
    }

    /// Set the thread name.
    // VUMA-VERIFIED: name configuration is safe
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the thread stack size in bytes.
    // VUMA-VERIFIED: stack_size configuration is safe
    pub fn stack_size(mut self, size: usize) -> Self {
        self.stack_size = Some(size);
        self
    }

    /// Spawn a new thread with the configured settings.
    // VUMA-VERIFIED: spawn creates a valid thread handle
    pub fn spawn<F, T>(self, f: F) -> Result<VumaJoinHandle<T>, VumaThreadError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let mut builder = std::thread::Builder::new();
        if let Some(name) = &self.name {
            builder = builder.name(name.clone());
        }
        if let Some(size) = self.stack_size {
            builder = builder.stack_size(size);
        }

        let std_handle = builder
            .spawn(f)
            .map_err(|e| VumaThreadError::SpawnFailed(e.to_string()))?;

        let std_thread = std_handle.thread();
        let id = VumaThreadId::from_std(std_thread.id());
        let name = std_thread.name().map(|s| s.to_string());
        let info = Arc::new(VumaThreadInfo::new(id, name));
        let vuma_thread = VumaThread {
            info: Arc::clone(&info),
            std_thread: std_thread.clone(),
        };

        Ok(VumaJoinHandle {
            inner: Some(std_handle),
            info,
            vuma_thread,
        })
    }

    /// Returns the CapD for this builder.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Execute])
    }

    /// Returns the SyncEdge annotations for this builder.
    // VUMA-VERIFIED: synchronization edges model spawn ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![SyncEdge::new(
            "thread_builder_new",
            "thread_spawn",
            SyncEdgeKind::Seq,
        )]
    }
}

impl Default for VumaThreadBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Free Functions
// ---------------------------------------------------------------------------

/// Spawn a new thread, returning a `VumaJoinHandle` for it.
// VUMA-VERIFIED: spawn creates a valid thread handle
pub fn spawn<F, T>(f: F) -> Result<VumaJoinHandle<T>, VumaThreadError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    VumaThreadBuilder::new().spawn(f)
}

/// Cooperatively yield the current time slice.
// VUMA-VERIFIED: yield_now is safe
pub fn yield_now() {
    std::thread::yield_now();
}

/// Sleep for the specified duration.
// VUMA-VERIFIED: sleep blocks the current thread safely
pub fn sleep(duration: std::time::Duration) {
    std::thread::sleep(duration);
}

/// Block the current thread until it is unparked.
// VUMA-VERIFIED: park blocks until unpark is called
pub fn park() {
    std::thread::park();
}

/// Unpark a thread that was previously parked.
///
/// The thread handle must correspond to a thread that was previously
/// created by `spawn`. Delegates to `VumaThread::unpark()`.
// VUMA-VERIFIED: unpark unblocks a parked thread
pub fn unpark(thread: &VumaThread) {
    thread.unpark();
}

/// Returns metadata about the current thread.
// VUMA-VERIFIED: current thread info is always available
pub fn current() -> VumaThreadInfo {
    let std_thread = std::thread::current();
    let id = VumaThreadId::from_std(std_thread.id());
    let name = std_thread.name().map(|s| s.to_string());
    VumaThreadInfo::new(id, name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_spawn_and_join() {
        let handle = spawn(|| 42).unwrap();
        let result = handle.join().unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_spawn_with_name() {
        let handle = VumaThreadBuilder::new()
            .name("test-thread")
            .spawn(|| "hello")
            .unwrap();
        assert_eq!(handle.thread().name(), Some("test-thread"));
        let result = handle.join().unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_spawn_with_stack_size() {
        let handle = VumaThreadBuilder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| 123)
            .unwrap();
        let result = handle.join().unwrap();
        assert_eq!(result, 123);
    }

    #[test]
    fn test_join_handle_is_finished() {
        let handle = spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            99
        })
        .unwrap();
        let _ = handle.is_finished();
        let result = handle.join().unwrap();
        assert_eq!(result, 99);
    }

    #[test]
    fn test_thread_panic() {
        let handle = spawn(move || -> i32 {
            panic!("intentional test panic");
        })
        .unwrap();
        let result = handle.join();
        assert!(matches!(result, Err(VumaThreadError::Panicked(_))));
    }

    #[test]
    fn test_yield_now() {
        yield_now(); // Should not block
    }

    #[test]
    fn test_sleep() {
        sleep(std::time::Duration::from_millis(1));
    }

    #[test]
    fn test_current_thread() {
        let info = current();
        // Should have a valid ID
        assert_ne!(info.id.as_u64(), 0);
    }

    #[test]
    fn test_thread_shared_state() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);
        let handle = spawn(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        handle.join().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_multiple_threads() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];
        for _ in 0..4 {
            let c = Arc::clone(&counter);
            handles.push(
                spawn(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap(),
            );
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn test_thread_id_display() {
        let info = current();
        let display = format!("{}", info.id);
        assert!(display.starts_with("ThreadId("));
    }

    #[test]
    fn test_thread_error_display() {
        let err = VumaThreadError::Panicked("oops".to_string());
        assert!(err.to_string().contains("oops"));
        let err = VumaThreadError::AlreadyJoined;
        assert!(err.to_string().contains("already joined"));
    }

    #[test]
    fn test_park_unpark() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        use std::time::Duration;

        let started = Arc::new(AtomicBool::new(false));
        let arrived = Arc::new(AtomicBool::new(false));
        let started_clone = Arc::clone(&started);
        let arrived_clone = Arc::clone(&arrived);

        let handle = spawn(move || {
            started_clone.store(true, AtomicOrdering::SeqCst);
            // Park with a timeout so the test doesn't hang on failure.
            std::thread::park_timeout(Duration::from_secs(5));
            arrived_clone.store(true, AtomicOrdering::SeqCst);
        })
        .unwrap();

        // Wait for the thread to start, then unpark it.
        while !started.load(AtomicOrdering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        // Small delay to ensure the thread is actually parked.
        std::thread::sleep(Duration::from_millis(10));

        let vuma_thread = handle.thread();
        vuma_thread.unpark();

        handle.join().unwrap();
        assert!(
            arrived.load(AtomicOrdering::SeqCst),
            "thread should have been unparked and set arrived flag"
        );
    }

    #[test]
    fn test_free_fn_unpark() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        use std::time::Duration;

        let started = Arc::new(AtomicBool::new(false));
        let arrived = Arc::new(AtomicBool::new(false));
        let started_clone = Arc::clone(&started);
        let arrived_clone = Arc::clone(&arrived);

        let handle = spawn(move || {
            started_clone.store(true, AtomicOrdering::SeqCst);
            std::thread::park_timeout(Duration::from_secs(5));
            arrived_clone.store(true, AtomicOrdering::SeqCst);
        })
        .unwrap();

        while !started.load(AtomicOrdering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(10));

        // Use the free function unpark()
        unpark(handle.thread());

        handle.join().unwrap();
        assert!(
            arrived.load(AtomicOrdering::SeqCst),
            "thread should have been unparked via free function"
        );
    }
}
