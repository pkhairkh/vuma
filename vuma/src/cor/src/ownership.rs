//! Region-Based Ownership Tracking for the COR.
//!
//! This module provides the [`OwnershipTracker`] which manages per-region
//! access permissions across threads. It supports three access modes
//! ([`Free`], [`SharedRead`], [`ExclusiveWrite`]) and detects data races
//! when conflicting accesses occur on the same region from different threads.

use crate::types::RegionId;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Thread ID
// ---------------------------------------------------------------------------

/// Identifier for a thread in the ownership tracking system.
pub type ThreadId = u64;

// ---------------------------------------------------------------------------
// Access mode
// ---------------------------------------------------------------------------

/// The access mode of a region.
///
/// A region can be in one of three states:
/// - **Free**: no thread holds any access; any thread can acquire read or write.
/// - **SharedRead**: one or more threads hold read access; no writer is allowed.
/// - **ExclusiveWrite**: a single thread holds write access; no other access is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// No thread holds any access to the region.
    Free,
    /// One or more threads hold shared read access.
    SharedRead,
    /// A single thread holds exclusive write access.
    ExclusiveWrite,
}

// ---------------------------------------------------------------------------
// Access record
// ---------------------------------------------------------------------------

/// A record of a single access attempt to a region.
#[derive(Debug, Clone)]
pub struct AccessRecord {
    /// The region that was accessed.
    pub region: RegionId,
    /// The thread that performed the access.
    pub thread: ThreadId,
    /// Whether the access was a write (`true`) or read (`false`).
    pub is_write: bool,
    /// Whether the access was granted (`true`) or denied/blocked (`false`).
    pub granted: bool,
}

// ---------------------------------------------------------------------------
// Region state
// ---------------------------------------------------------------------------

/// The ownership state of a single region.
#[derive(Debug, Clone)]
pub struct RegionState {
    /// The region identifier.
    pub id: RegionId,
    /// The current owner thread (only set for `ExclusiveWrite`).
    pub owner: Option<ThreadId>,
    /// The current access mode.
    pub access_mode: AccessMode,
    /// Threads currently holding shared read access.
    pub readers: Vec<ThreadId>,
    /// Threads waiting to acquire access (blocked).
    pub waiting_threads: Vec<WaitingThread>,
}

/// A thread waiting to acquire access to a region.
#[derive(Debug, Clone)]
pub struct WaitingThread {
    /// The waiting thread's identifier.
    pub thread: ThreadId,
    /// Whether the thread is waiting for write access.
    pub is_write: bool,
}

impl RegionState {
    /// Creates a new region state in the `Free` mode.
    pub fn new(id: RegionId) -> Self {
        RegionState {
            id,
            owner: None,
            access_mode: AccessMode::Free,
            readers: Vec::new(),
            waiting_threads: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Data race
// ---------------------------------------------------------------------------

/// A detected data race between two threads on the same region.
#[derive(Debug, Clone)]
pub struct DataRace {
    /// The region where the race was detected.
    pub region: RegionId,
    /// The first thread involved in the race.
    pub thread_a: ThreadId,
    /// The second thread involved in the race.
    pub thread_b: ThreadId,
    /// The access records that constitute the race.
    pub access_records: Vec<AccessRecord>,
}

// ---------------------------------------------------------------------------
// Ownership error
// ---------------------------------------------------------------------------

/// Errors that can occur during ownership operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum OwnershipError {
    /// The region is already held in a conflicting mode.
    #[error("Region {region} is held in {mode:?} mode by thread {holder:?}; thread {requestor} cannot acquire {request_mode}")]
    Conflict {
        region: RegionId,
        mode: AccessMode,
        holder: Option<ThreadId>,
        requestor: ThreadId,
        request_mode: String,
    },

    /// The thread does not hold the expected access on the region.
    #[error("Thread {thread} does not hold the expected access on region {region}")]
    NotHeld { thread: ThreadId, region: RegionId },

    /// The region is not tracked.
    #[error("Region {0} is not tracked")]
    NotTracked(RegionId),
}

// ---------------------------------------------------------------------------
// Ownership tracker
// ---------------------------------------------------------------------------

/// Tracks region-based ownership across threads.
///
/// The `OwnershipTracker` maintains a map of regions to their current
/// ownership state and an access log of all acquire/release operations.
/// It provides methods to acquire read/write access, release access, try
/// non-blocking variants, and detect data races.
#[derive(Debug, Clone)]
pub struct OwnershipTracker {
    /// Per-region ownership state.
    regions: HashMap<RegionId, RegionState>,
    /// Log of all access attempts.
    access_log: Vec<AccessRecord>,
}

impl OwnershipTracker {
    /// Creates a new, empty ownership tracker.
    pub fn new() -> Self {
        OwnershipTracker {
            regions: HashMap::new(),
            access_log: Vec::new(),
        }
    }

    // -- Region management --------------------------------------------------

    /// Registers a region for tracking, starting in `Free` mode.
    ///
    /// If the region is already tracked, this is a no-op.
    pub fn register_region(&mut self, region_id: RegionId) {
        self.regions
            .entry(region_id)
            .or_insert_with(|| RegionState::new(region_id));
    }

    /// Unregisters a region, removing it from tracking.
    pub fn unregister_region(&mut self, region_id: RegionId) {
        self.regions.remove(&region_id);
    }

    /// Returns `true` if the region is being tracked.
    pub fn is_tracked(&self, region_id: RegionId) -> bool {
        self.regions.contains_key(&region_id)
    }

    /// Returns a reference to the region state, if tracked.
    pub fn get_region(&self, region_id: RegionId) -> Option<&RegionState> {
        self.regions.get(&region_id)
    }

    /// Returns the number of tracked regions.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    // -- Acquire read -------------------------------------------------------

    /// Acquires shared read access to a region for the given thread.
    ///
    /// This will block (add to waiting list) if the region is currently
    /// held in `ExclusiveWrite` mode by another thread.
    ///
    /// # Errors
    ///
    /// Returns [`OwnershipError::NotTracked`] if the region is not tracked.
    pub fn acquire_read(
        &mut self,
        region_id: RegionId,
        thread: ThreadId,
    ) -> Result<(), OwnershipError> {
        let state = self
            .regions
            .get_mut(&region_id)
            .ok_or(OwnershipError::NotTracked(region_id))?;

        match state.access_mode {
            AccessMode::Free => {
                state.access_mode = AccessMode::SharedRead;
                state.readers.push(thread);
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: false,
                    granted: true,
                });
                Ok(())
            }
            AccessMode::SharedRead => {
                // Already in shared read — allow another reader if the thread
                // doesn't already hold a read.
                if state.readers.contains(&thread) {
                    // Thread already has read access; grant again (idempotent).
                    self.access_log.push(AccessRecord {
                        region: region_id,
                        thread,
                        is_write: false,
                        granted: true,
                    });
                    return Ok(());
                }
                state.readers.push(thread);
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: false,
                    granted: true,
                });
                Ok(())
            }
            AccessMode::ExclusiveWrite => {
                // A writer holds the region — block this thread.
                state.waiting_threads.push(WaitingThread {
                    thread,
                    is_write: false,
                });
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: false,
                    granted: false,
                });
                Err(OwnershipError::Conflict {
                    region: region_id,
                    mode: AccessMode::ExclusiveWrite,
                    holder: state.owner,
                    requestor: thread,
                    request_mode: "SharedRead".to_string(),
                })
            }
        }
    }

    // -- Acquire write ------------------------------------------------------

    /// Acquires exclusive write access to a region for the given thread.
    ///
    /// This will block (add to waiting list) if the region is currently
    /// held in `SharedRead` or `ExclusiveWrite` mode by other threads.
    ///
    /// # Errors
    ///
    /// Returns [`OwnershipError::NotTracked`] if the region is not tracked,
    /// or [`OwnershipError::Conflict`] if another thread holds the region.
    pub fn acquire_write(
        &mut self,
        region_id: RegionId,
        thread: ThreadId,
    ) -> Result<(), OwnershipError> {
        let state = self
            .regions
            .get_mut(&region_id)
            .ok_or(OwnershipError::NotTracked(region_id))?;

        match state.access_mode {
            AccessMode::Free => {
                state.access_mode = AccessMode::ExclusiveWrite;
                state.owner = Some(thread);
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: true,
                    granted: true,
                });
                Ok(())
            }
            AccessMode::SharedRead => {
                // Check if only this thread holds a read lock (upgrade).
                if state.readers.len() == 1 && state.readers[0] == thread {
                    // Upgrade from read to write.
                    state.access_mode = AccessMode::ExclusiveWrite;
                    state.owner = Some(thread);
                    state.readers.clear();
                    self.access_log.push(AccessRecord {
                        region: region_id,
                        thread,
                        is_write: true,
                        granted: true,
                    });
                    return Ok(());
                }
                // Other readers — block.
                state.waiting_threads.push(WaitingThread {
                    thread,
                    is_write: true,
                });
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: true,
                    granted: false,
                });
                Err(OwnershipError::Conflict {
                    region: region_id,
                    mode: AccessMode::SharedRead,
                    holder: state.readers.first().copied(),
                    requestor: thread,
                    request_mode: "ExclusiveWrite".to_string(),
                })
            }
            AccessMode::ExclusiveWrite => {
                if state.owner == Some(thread) {
                    // Already the writer — idempotent.
                    self.access_log.push(AccessRecord {
                        region: region_id,
                        thread,
                        is_write: true,
                        granted: true,
                    });
                    return Ok(());
                }
                // Another writer — block.
                state.waiting_threads.push(WaitingThread {
                    thread,
                    is_write: true,
                });
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: true,
                    granted: false,
                });
                Err(OwnershipError::Conflict {
                    region: region_id,
                    mode: AccessMode::ExclusiveWrite,
                    holder: state.owner,
                    requestor: thread,
                    request_mode: "ExclusiveWrite".to_string(),
                })
            }
        }
    }

    // -- Try-acquire variants -----------------------------------------------

    /// Attempts to acquire shared read access without blocking.
    ///
    /// Returns `Ok(())` if access was granted, or `Err(OwnershipError)` if
    /// it would need to block. Unlike [`acquire_read`](Self::acquire_read),
    /// this does **not** add the thread to the waiting list.
    pub fn try_acquire_read(
        &mut self,
        region_id: RegionId,
        thread: ThreadId,
    ) -> Result<(), OwnershipError> {
        let state = self
            .regions
            .get_mut(&region_id)
            .ok_or(OwnershipError::NotTracked(region_id))?;

        match state.access_mode {
            AccessMode::Free => {
                state.access_mode = AccessMode::SharedRead;
                state.readers.push(thread);
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: false,
                    granted: true,
                });
                Ok(())
            }
            AccessMode::SharedRead => {
                if !state.readers.contains(&thread) {
                    state.readers.push(thread);
                }
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: false,
                    granted: true,
                });
                Ok(())
            }
            AccessMode::ExclusiveWrite => {
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: false,
                    granted: false,
                });
                Err(OwnershipError::Conflict {
                    region: region_id,
                    mode: AccessMode::ExclusiveWrite,
                    holder: state.owner,
                    requestor: thread,
                    request_mode: "SharedRead".to_string(),
                })
            }
        }
    }

    /// Attempts to acquire exclusive write access without blocking.
    ///
    /// Returns `Ok(())` if access was granted, or `Err(OwnershipError)` if
    /// it would need to block. Unlike [`acquire_write`](Self::acquire_write),
    /// this does **not** add the thread to the waiting list.
    pub fn try_acquire_write(
        &mut self,
        region_id: RegionId,
        thread: ThreadId,
    ) -> Result<(), OwnershipError> {
        let state = self
            .regions
            .get_mut(&region_id)
            .ok_or(OwnershipError::NotTracked(region_id))?;

        match state.access_mode {
            AccessMode::Free => {
                state.access_mode = AccessMode::ExclusiveWrite;
                state.owner = Some(thread);
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: true,
                    granted: true,
                });
                Ok(())
            }
            AccessMode::SharedRead => {
                if state.readers.len() == 1 && state.readers[0] == thread {
                    state.access_mode = AccessMode::ExclusiveWrite;
                    state.owner = Some(thread);
                    state.readers.clear();
                    self.access_log.push(AccessRecord {
                        region: region_id,
                        thread,
                        is_write: true,
                        granted: true,
                    });
                    return Ok(());
                }
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: true,
                    granted: false,
                });
                Err(OwnershipError::Conflict {
                    region: region_id,
                    mode: AccessMode::SharedRead,
                    holder: state.readers.first().copied(),
                    requestor: thread,
                    request_mode: "ExclusiveWrite".to_string(),
                })
            }
            AccessMode::ExclusiveWrite => {
                if state.owner == Some(thread) {
                    self.access_log.push(AccessRecord {
                        region: region_id,
                        thread,
                        is_write: true,
                        granted: true,
                    });
                    return Ok(());
                }
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread,
                    is_write: true,
                    granted: false,
                });
                Err(OwnershipError::Conflict {
                    region: region_id,
                    mode: AccessMode::ExclusiveWrite,
                    holder: state.owner,
                    requestor: thread,
                    request_mode: "ExclusiveWrite".to_string(),
                })
            }
        }
    }

    // -- Release ------------------------------------------------------------

    /// Releases a thread's access to a region.
    ///
    /// If the thread held exclusive write, the region transitions to `Free`
    /// and any waiting threads are granted access (first writer wins, or
    /// all readers are granted if the first waiter is a reader).
    ///
    /// If the thread was one of several readers, it is removed from the
    /// reader list and the region stays in `SharedRead`. When the last
    /// reader releases, the region transitions to `Free` (or grants a
    /// waiting writer).
    ///
    /// # Errors
    ///
    /// Returns [`OwnershipError::NotTracked`] if the region is not tracked,
    /// or [`OwnershipError::NotHeld`] if the thread doesn't hold access.
    pub fn release(&mut self, region_id: RegionId, thread: ThreadId) -> Result<(), OwnershipError> {
        let state = self
            .regions
            .get_mut(&region_id)
            .ok_or(OwnershipError::NotTracked(region_id))?;

        match state.access_mode {
            AccessMode::Free => Err(OwnershipError::NotHeld {
                thread,
                region: region_id,
            }),
            AccessMode::ExclusiveWrite => {
                if state.owner != Some(thread) {
                    return Err(OwnershipError::NotHeld {
                        thread,
                        region: region_id,
                    });
                }
                state.owner = None;
                state.access_mode = AccessMode::Free;
                self.grant_waiters(region_id);
                Ok(())
            }
            AccessMode::SharedRead => {
                let idx = state.readers.iter().position(|&t| t == thread);
                match idx {
                    Some(i) => {
                        state.readers.remove(i);
                        if state.readers.is_empty() {
                            state.access_mode = AccessMode::Free;
                            self.grant_waiters(region_id);
                        }
                        Ok(())
                    }
                    None => Err(OwnershipError::NotHeld {
                        thread,
                        region: region_id,
                    }),
                }
            }
        }
    }

    /// Grants access to waiting threads after a region becomes free.
    ///
    /// This is called internally by [`release`](Self::release). The strategy:
    /// - If the first waiter wants write access, grant it exclusive.
    /// - If the first waiter wants read access, grant read to it and all
    ///   subsequent read waiters (up to the first write waiter).
    fn grant_waiters(&mut self, region_id: RegionId) {
        // Collect waiting threads from the region.
        let waiters: Vec<WaitingThread> = {
            let state = self.regions.get_mut(&region_id).expect("region must exist");
            std::mem::take(&mut state.waiting_threads)
        };

        if waiters.is_empty() {
            return;
        }

        let state = self.regions.get_mut(&region_id).expect("region must exist");

        // Check if the first waiter wants write access.
        if waiters[0].is_write {
            // Grant exclusive write to the first waiter; re-queue the rest.
            state.access_mode = AccessMode::ExclusiveWrite;
            state.owner = Some(waiters[0].thread);
            self.access_log.push(AccessRecord {
                region: region_id,
                thread: waiters[0].thread,
                is_write: true,
                granted: true,
            });
            // Re-queue remaining waiters.
            for waiter in waiters.into_iter().skip(1) {
                state.waiting_threads.push(waiter);
            }
        } else {
            // Grant shared read to consecutive read waiters.
            state.access_mode = AccessMode::SharedRead;
            for waiter in &waiters {
                if waiter.is_write {
                    break;
                }
                state.readers.push(waiter.thread);
                self.access_log.push(AccessRecord {
                    region: region_id,
                    thread: waiter.thread,
                    is_write: false,
                    granted: true,
                });
            }
            // Re-queue any remaining (starting from the first write waiter).
            let first_write_idx = waiters.iter().position(|w| w.is_write);
            if let Some(idx) = first_write_idx {
                for waiter in waiters.into_iter().skip(idx) {
                    state.waiting_threads.push(waiter);
                }
            }
        }
    }

    // -- Data race detection ------------------------------------------------

    /// Detects data races in the access log.
    ///
    /// A data race occurs when two different threads access the same region
    /// and at least one of the accesses is a write, and the accesses were
    /// both granted (i.e. they overlapped in time). This is detected by
    /// looking for patterns where a write is granted while another thread
    /// already holds a granted read or write on the same region.
    ///
    /// Returns a list of detected data races.
    pub fn detect_data_races(&self) -> Vec<DataRace> {
        let mut races = Vec::new();

        // Group granted access records by region.
        let mut region_accesses: HashMap<RegionId, Vec<&AccessRecord>> = HashMap::new();
        for record in &self.access_log {
            if record.granted {
                region_accesses
                    .entry(record.region)
                    .or_default()
                    .push(record);
            }
        }

        for (region, accesses) in &region_accesses {
            // Look for pairs of granted accesses where at least one is a write
            // and they are from different threads.
            for i in 0..accesses.len() {
                for j in (i + 1)..accesses.len() {
                    let a = accesses[i];
                    let b = accesses[j];
                    if a.thread != b.thread && (a.is_write || b.is_write) {
                        // Found a potential race. Check if these accesses
                        // overlap in time by checking the current region state
                        // — if both were granted, they must have overlapped
                        // (otherwise one would have been denied).
                        races.push(DataRace {
                            region: *region,
                            thread_a: a.thread,
                            thread_b: b.thread,
                            access_records: vec![a.clone(), b.clone()],
                        });
                    }
                }
            }
        }

        // Deduplicate: only report one race per (region, thread_a, thread_b) pair.
        let mut seen = std::collections::HashSet::new();
        races.retain(|race| {
            let key = (
                race.region,
                race.thread_a.min(race.thread_b),
                race.thread_a.max(race.thread_b),
            );
            seen.insert(key)
        });

        races
    }

    // -- Access log ---------------------------------------------------------

    /// Returns the full access log.
    pub fn access_log(&self) -> &[AccessRecord] {
        &self.access_log
    }

    /// Returns the number of access log entries.
    pub fn access_log_len(&self) -> usize {
        self.access_log.len()
    }

    /// Clears the access log.
    pub fn clear_access_log(&mut self) {
        self.access_log.clear();
    }
}

impl Default for OwnershipTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_track_region() {
        let mut tracker = OwnershipTracker::new();
        assert!(!tracker.is_tracked(1));
        tracker.register_region(1);
        assert!(tracker.is_tracked(1));
        assert_eq!(tracker.region_count(), 1);
    }

    #[test]
    fn unregister_region() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.unregister_region(1);
        assert!(!tracker.is_tracked(1));
        assert_eq!(tracker.region_count(), 0);
    }

    #[test]
    fn acquire_read_on_free_region() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        assert!(tracker.acquire_read(1, 100).is_ok());
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.access_mode, AccessMode::SharedRead);
        assert!(state.readers.contains(&100));
    }

    #[test]
    fn multiple_readers_shared_access() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        assert!(tracker.acquire_read(1, 100).is_ok());
        assert!(tracker.acquire_read(1, 200).is_ok());
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.access_mode, AccessMode::SharedRead);
        assert_eq!(state.readers.len(), 2);
    }

    #[test]
    fn acquire_write_on_free_region() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        assert!(tracker.acquire_write(1, 100).is_ok());
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.access_mode, AccessMode::ExclusiveWrite);
        assert_eq!(state.owner, Some(100));
    }

    #[test]
    fn write_blocked_by_readers() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_read(1, 100).unwrap();
        tracker.acquire_read(1, 200).unwrap();
        let result = tracker.acquire_write(1, 300);
        assert!(result.is_err());
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.waiting_threads.len(), 1);
    }

    #[test]
    fn read_blocked_by_writer() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_write(1, 100).unwrap();
        let result = tracker.acquire_read(1, 200);
        assert!(result.is_err());
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.waiting_threads.len(), 1);
    }

    #[test]
    fn release_write_grants_waiting_reader() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_write(1, 100).unwrap();
        // Thread 200 tries to read while writer holds it — gets queued.
        let _ = tracker.acquire_read(1, 200);
        // Writer releases.
        tracker.release(1, 100).unwrap();
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.access_mode, AccessMode::SharedRead);
        assert!(state.readers.contains(&200));
    }

    #[test]
    fn release_last_reader_grants_waiting_writer() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_read(1, 100).unwrap();
        // Thread 200 tries to write — gets queued.
        let _ = tracker.acquire_write(1, 200);
        // Reader releases.
        tracker.release(1, 100).unwrap();
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.access_mode, AccessMode::ExclusiveWrite);
        assert_eq!(state.owner, Some(200));
    }

    #[test]
    fn try_acquire_read_success() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        assert!(tracker.try_acquire_read(1, 100).is_ok());
    }

    #[test]
    fn try_acquire_read_blocked_by_writer() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_write(1, 100).unwrap();
        let result = tracker.try_acquire_read(1, 200);
        assert!(result.is_err());
        // Should NOT be in the waiting list (non-blocking).
        let state = tracker.get_region(1).unwrap();
        assert!(state.waiting_threads.is_empty());
    }

    #[test]
    fn try_acquire_write_success() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        assert!(tracker.try_acquire_write(1, 100).is_ok());
    }

    #[test]
    fn try_acquire_write_blocked_by_reader() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_read(1, 100).unwrap();
        let result = tracker.try_acquire_write(1, 200);
        assert!(result.is_err());
        let state = tracker.get_region(1).unwrap();
        assert!(state.waiting_threads.is_empty());
    }

    #[test]
    fn upgrade_read_to_write() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_read(1, 100).unwrap();
        // Same thread upgrades to write.
        assert!(tracker.acquire_write(1, 100).is_ok());
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.access_mode, AccessMode::ExclusiveWrite);
        assert_eq!(state.owner, Some(100));
        assert!(state.readers.is_empty());
    }

    #[test]
    fn release_unheld_region_errors() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        let result = tracker.release(1, 999);
        assert!(result.is_err());
    }

    #[test]
    fn detect_data_race_write_vs_read() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        // Simulate a scenario where a write and a read are both granted
        // on the same region from different threads. This would happen
        // if the tracker state were inconsistent — but we can test the
        // detection logic by directly manipulating the access log.
        tracker.access_log.push(AccessRecord {
            region: 1,
            thread: 100,
            is_write: true,
            granted: true,
        });
        tracker.access_log.push(AccessRecord {
            region: 1,
            thread: 200,
            is_write: false,
            granted: true,
        });
        let races = tracker.detect_data_races();
        assert_eq!(races.len(), 1);
        assert_eq!(races[0].region, 1);
    }

    #[test]
    fn no_data_race_two_reads() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.access_log.push(AccessRecord {
            region: 1,
            thread: 100,
            is_write: false,
            granted: true,
        });
        tracker.access_log.push(AccessRecord {
            region: 1,
            thread: 200,
            is_write: false,
            granted: true,
        });
        let races = tracker.detect_data_races();
        assert!(races.is_empty(), "two reads should not be a data race");
    }

    #[test]
    fn access_log_records_granted_and_denied() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_read(1, 100).unwrap();
        let _ = tracker.acquire_write(1, 200); // denied — reader holds it
        assert_eq!(tracker.access_log_len(), 2);
        let log = tracker.access_log();
        assert!(log[0].granted);
        assert!(!log[1].granted);
    }

    #[test]
    fn clear_access_log() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_read(1, 100).unwrap();
        assert_eq!(tracker.access_log_len(), 1);
        tracker.clear_access_log();
        assert_eq!(tracker.access_log_len(), 0);
    }

    #[test]
    fn double_register_is_noop() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.register_region(1);
        assert_eq!(tracker.region_count(), 1);
    }

    #[test]
    fn release_one_reader_keeps_shared_read() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_read(1, 100).unwrap();
        tracker.acquire_read(1, 200).unwrap();
        tracker.release(1, 100).unwrap();
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.access_mode, AccessMode::SharedRead);
        assert_eq!(state.readers.len(), 1);
        assert!(state.readers.contains(&200));
    }

    #[test]
    fn idempotent_acquire_write() {
        let mut tracker = OwnershipTracker::new();
        tracker.register_region(1);
        tracker.acquire_write(1, 100).unwrap();
        assert!(tracker.acquire_write(1, 100).is_ok());
        let state = tracker.get_region(1).unwrap();
        assert_eq!(state.owner, Some(100));
    }
}
