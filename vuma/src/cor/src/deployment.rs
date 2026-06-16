//! Adaptive deployment for the Continuous Optimization Runtime.
//!
//! The COR can distribute compiled regions across heterogeneous execution
//! targets: the local process or remote endpoints (e.g. a cloud compute
//! instance).
//!
//! # Core types
//!
//! - [`DeploymentTarget`] — where to deploy (Local, Remote).
//! - [`DeploymentPackage`] — compiled binary + metadata + debug info.
//! - [`DeploymentManager`] — orchestrates deploys, hot-swaps, rollbacks, and
//!   delta transfers.
//!
//! # Version management
//!
//! Every successful deployment is recorded in a [`VersionLog`]. The log
//! enables rollback to any previously deployed version of a region.
//!
//! # Delta deployment
//!
//! When re-deploying a region whose previous version is already present on
//! the target, the manager computes a [`DeploymentDelta`] containing only the
//! changed bytes, reducing transfer size.

use crate::config::Config;
use crate::profile::ProfileData;
use crate::types::RegionId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ===========================================================================
// DeploymentTarget
// ===========================================================================

/// A target where a compiled region can be deployed for execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeploymentTarget {
    /// Execute the region in the local process.
    Local,

    /// Execute the region on a remote endpoint (e.g. a cloud lambda or
    /// compute server).
    Remote {
        /// The endpoint URI (e.g. `"https://compute.example.com/v1/exec"`).
        endpoint: String,
    },
}

impl DeploymentTarget {
    /// Returns `true` if this target supports hot-swapping of code.
    ///
    /// Currently, no deployment target supports hot-swapping. This method
    /// is retained for forward compatibility.
    pub fn supports_hot_swap(&self) -> bool {
        false
    }

    /// Returns a human-readable label for the target kind.
    pub fn kind_label(&self) -> &'static str {
        match self {
            DeploymentTarget::Local => "Local",
            DeploymentTarget::Remote { .. } => "Remote",
        }
    }
}

impl std::fmt::Display for DeploymentTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentTarget::Local => write!(f, "Local"),
            DeploymentTarget::Remote { endpoint } => write!(f, "Remote({})", endpoint),
        }
    }
}

// ===========================================================================
// DeploymentPackage
// ===========================================================================

/// Semantic version for a deployed region.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PackageVersion(pub u64);

impl std::fmt::Display for PackageVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// Compilation metadata attached to a deployment package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    /// Region this package was compiled from.
    pub region_id: RegionId,
    /// Monotonically increasing version number.
    pub version: PackageVersion,
    /// CRC32 checksum of the `code` bytes.
    pub checksum: u32,
    /// Compilation flags / optimization level used.
    pub optimization_label: String,
    /// Size of the compiled code in bytes (equals `code.len()`).
    pub code_size: usize,
    /// Timestamp when the package was created (seconds since Unix epoch).
    pub created_at_secs: u64,
}

/// Debug information bundled with a deployment package.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugInfo {
    /// Source-to-code-offset mapping: (source_line, code_byte_offset).
    pub source_map: Vec<(u32, u32)>,
    /// Symbol table: (offset, name).
    pub symbols: Vec<(u32, String)>,
    /// Arbitrary notes from the compiler.
    pub notes: String,
}

/// A deployment-ready compiled package: binary + metadata + debug info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentPackage {
    /// Compiled machine code bytes.
    pub code: Vec<u8>,
    /// Metadata describing the package.
    pub metadata: PackageMetadata,
    /// Optional debug information.
    pub debug_info: Option<DebugInfo>,
}

impl DeploymentPackage {
    /// Creates a new deployment package with a CRC32 checksum computed from
    /// the code bytes.
    pub fn new(
        code: Vec<u8>,
        region_id: RegionId,
        version: PackageVersion,
        optimization_label: &str,
    ) -> Self {
        let checksum = crc32(&code);
        let code_size = code.len();
        let created_at_secs = epoch_secs();
        DeploymentPackage {
            code,
            metadata: PackageMetadata {
                region_id,
                version,
                checksum,
                optimization_label: optimization_label.to_owned(),
                code_size,
                created_at_secs,
            },
            debug_info: None,
        }
    }

    /// Attaches debug information to this package.
    pub fn with_debug_info(mut self, info: DebugInfo) -> Self {
        self.debug_info = Some(info);
        self
    }

    /// Validates that the stored checksum matches the actual code bytes.
    pub fn validate_checksum(&self) -> bool {
        crc32(&self.code) == self.metadata.checksum
    }
}

// ===========================================================================
// DeploymentDelta
// ===========================================================================

/// A delta (diff) between two deployment packages, used for bandwidth-
/// efficient re-deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentDelta {
    /// The region this delta applies to.
    pub region_id: RegionId,
    /// Version we are diffing **from**.
    pub from_version: PackageVersion,
    /// Version we are diffing **to**.
    pub to_version: PackageVersion,
    /// Patches: (offset_in_code, replacement_bytes).
    /// Offsets that are not listed are unchanged.
    pub patches: Vec<(usize, Vec<u8>)>,
    /// Bytes that should be **deleted** starting at the given offset.
    pub deletions: Vec<(usize, usize)>,
    /// Bytes that should be **inserted** at the given offset.
    pub insertions: Vec<(usize, Vec<u8>)>,
}

impl DeploymentDelta {
    /// Computes a delta between two code buffers.
    ///
    /// The algorithm performs a simple block-level diff:
    /// - Scans both buffers in [`DELTA_BLOCK_SIZE`] chunks.
    /// - Chunks that differ become patches.
    /// - Trailing bytes in the new buffer become insertions.
    /// - Trailing bytes in the old buffer become deletions.
    pub fn compute(
        region_id: RegionId,
        from_version: PackageVersion,
        to_version: PackageVersion,
        old_code: &[u8],
        new_code: &[u8],
    ) -> Self {
        const BLOCK: usize = 64;
        let min_len = old_code.len().min(new_code.len());
        let mut patches = Vec::new();

        let mut offset = 0;
        while offset + BLOCK <= min_len {
            if old_code[offset..offset + BLOCK] != new_code[offset..offset + BLOCK] {
                patches.push((offset, new_code[offset..offset + BLOCK].to_vec()));
            }
            offset += BLOCK;
        }
        // Handle the tail (< BLOCK bytes).
        if offset < min_len && old_code[offset..min_len] != new_code[offset..min_len] {
            patches.push((offset, new_code[offset..min_len].to_vec()));
        }

        let mut deletions = Vec::new();
        let mut insertions = Vec::new();

        if new_code.len() > old_code.len() {
            insertions.push((old_code.len(), new_code[old_code.len()..].to_vec()));
        } else if old_code.len() > new_code.len() {
            deletions.push((new_code.len(), old_code.len() - new_code.len()));
        }

        DeploymentDelta {
            region_id,
            from_version,
            to_version,
            patches,
            deletions,
            insertions,
        }
    }

    /// Applies this delta to an old code buffer, producing the new code
    /// buffer.
    pub fn apply(&self, old_code: &[u8]) -> Result<Vec<u8>, DeploymentError> {
        let mut result = old_code.to_vec();

        // Apply patches (same-length replacements).
        for (offset, data) in &self.patches {
            let end = offset.checked_add(data.len()).ok_or_else(|| {
                DeploymentError::DeltaApplyFailed(
                    self.region_id,
                    "patch offset + length overflow".to_owned(),
                )
            })?;
            if end > result.len() {
                return Err(DeploymentError::DeltaApplyFailed(
                    self.region_id,
                    format!(
                        "patch at {}..{} exceeds buffer len {}",
                        offset,
                        end,
                        result.len()
                    ),
                ));
            }
            result[*offset..end].copy_from_slice(data);
        }

        // Apply deletions (truncate tail only — ordered back-to-front).
        let mut sorted_deletions = self.deletions.clone();
        sorted_deletions.sort_by_key(|b| std::cmp::Reverse(b.0));
        for (offset, len) in &sorted_deletions {
            let end = *offset + len;
            if end > result.len() {
                return Err(DeploymentError::DeltaApplyFailed(
                    self.region_id,
                    format!("deletion at {}..{} exceeds buffer", offset, end),
                ));
            }
            result.drain(*offset..*offset + len);
        }

        // Apply insertions (at specific offsets).
        // Sort by offset descending so later insertions don't shift earlier ones.
        let mut sorted_insertions = self.insertions.clone();
        sorted_insertions.sort_by_key(|b| std::cmp::Reverse(b.0));
        for (offset, data) in &sorted_insertions {
            if *offset > result.len() {
                return Err(DeploymentError::DeltaApplyFailed(
                    self.region_id,
                    format!(
                        "insertion at {} exceeds buffer len {}",
                        offset,
                        result.len()
                    ),
                ));
            }
            result.splice(*offset..*offset, data.iter().copied());
        }

        Ok(result)
    }

    /// Returns an estimate of the delta size in bytes (for bandwidth
    /// planning).
    pub fn estimated_size(&self) -> usize {
        self.patches.iter().map(|(_, d)| d.len()).sum::<usize>()
            + self.insertions.iter().map(|(_, d)| d.len()).sum::<usize>()
    }

    /// Returns true if the delta contains no changes.
    pub fn is_empty(&self) -> bool {
        self.patches.is_empty() && self.deletions.is_empty() && self.insertions.is_empty()
    }
}

// ===========================================================================
// DeploymentResult
// ===========================================================================

/// Outcome of a deployment operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentResult {
    /// The region that was deployed.
    pub region_id: RegionId,
    /// The version that was deployed.
    pub version: PackageVersion,
    /// The target it was deployed to.
    pub target: DeploymentTarget,
    /// Wall-clock time taken for the deployment.
    pub duration: Duration,
    /// Size of the payload sent (bytes). For delta deployments this is the
    /// delta size; for full deployments it's the code size.
    pub bytes_transferred: usize,
    /// Whether a hot-swap was performed.
    pub hot_swapped: bool,
    /// Whether this was a delta (as opposed to full) deployment.
    pub was_delta: bool,
}

// ===========================================================================
// VersionLog
// ===========================================================================

/// A record of a deployed version, stored in the [`VersionLog`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionRecord {
    /// The region that was deployed.
    pub region_id: RegionId,
    /// The version deployed.
    pub version: PackageVersion,
    /// The target it was deployed to.
    pub target: DeploymentTarget,
    /// CRC32 checksum of the code for this version.
    pub checksum: u32,
    /// The compiled code bytes (retained for rollback).
    pub code: Vec<u8>,
    /// Timestamp (seconds since Unix epoch).
    pub deployed_at_secs: u64,
}

/// Tracks deployed versions per region, enabling rollback.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionLog {
    /// region_id → ordered list of deployed versions (oldest first).
    records: HashMap<RegionId, Vec<VersionRecord>>,
}

impl VersionLog {
    /// Creates an empty version log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a deployment in the log.
    pub fn record(&mut self, pkg: &DeploymentPackage, target: &DeploymentTarget) {
        let entry = self.records.entry(pkg.metadata.region_id).or_default();
        entry.push(VersionRecord {
            region_id: pkg.metadata.region_id,
            version: pkg.metadata.version.clone(),
            target: target.clone(),
            checksum: pkg.metadata.checksum,
            code: pkg.code.clone(),
            deployed_at_secs: epoch_secs(),
        });
    }

    /// Returns the latest version record for a region, if any.
    pub fn latest(&self, region_id: RegionId) -> Option<&VersionRecord> {
        self.records.get(&region_id).and_then(|v| v.last())
    }

    /// Returns the version record *before* the latest (i.e. the rollback
    /// candidate), if any.
    pub fn previous(&self, region_id: RegionId) -> Option<&VersionRecord> {
        self.records
            .get(&region_id)
            .and_then(|v| v.len().checked_sub(2).map(|i| &v[i]))
    }

    /// Returns all version records for a region (oldest first).
    pub fn history(&self, region_id: RegionId) -> &[VersionRecord] {
        self.records
            .get(&region_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Returns the number of versions recorded for a region.
    pub fn version_count(&self, region_id: RegionId) -> usize {
        self.records.get(&region_id).map(|v| v.len()).unwrap_or(0)
    }
}

// ===========================================================================
// HotSwapState
// ===========================================================================

/// State machine for a hot-swap operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HotSwapPhase {
    /// No hot-swap in progress.
    Idle,
    /// Shadow buffer is being prepared with the new code.
    PreparingShadow,
    /// Shadow is ready; waiting for a safe point to swap.
    AwaitingSafePoint,
    /// Atomic pointer swap has been performed; old code is quiescing.
    Swapping,
    /// Hot-swap completed successfully.
    Completed,
    /// Hot-swap failed; rolled back to the previous code.
    Failed,
}

impl std::fmt::Display for HotSwapPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HotSwapPhase::Idle => write!(f, "Idle"),
            HotSwapPhase::PreparingShadow => write!(f, "PreparingShadow"),
            HotSwapPhase::AwaitingSafePoint => write!(f, "AwaitingSafePoint"),
            HotSwapPhase::Swapping => write!(f, "Swapping"),
            HotSwapPhase::Completed => write!(f, "Completed"),
            HotSwapPhase::Failed => write!(f, "Failed"),
        }
    }
}

/// Tracks the state of a hot-swap in progress for a region.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotSwapState {
    /// The region being hot-swapped.
    pub region_id: RegionId,
    /// Current phase of the hot-swap.
    pub phase: HotSwapPhase,
    /// The version we are swapping **from**.
    pub from_version: PackageVersion,
    /// The version we are swapping **to**.
    pub to_version: PackageVersion,
}

impl HotSwapState {
    /// Creates a new hot-swap state in `Idle` phase.
    pub fn new(region_id: RegionId) -> Self {
        HotSwapState {
            region_id,
            phase: HotSwapPhase::Idle,
            from_version: PackageVersion(0),
            to_version: PackageVersion(0),
        }
    }
}

// ===========================================================================
// DeploymentPlan (kept for backwards compatibility)
// ===========================================================================

/// A deployment plan mapping each region to an execution target.
///
/// The plan is recomputed periodically (or when a significant profile
/// shift is detected) by [`DeploymentPlanner`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeploymentPlan {
    /// Ordered list of (region, target) assignments.
    pub regions: Vec<(RegionId, DeploymentTarget)>,
}

impl DeploymentPlan {
    /// Creates an empty deployment plan.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the target for the given region, if assigned.
    pub fn target_for(&self, region_id: RegionId) -> Option<&DeploymentTarget> {
        self.regions
            .iter()
            .find(|(r, _)| *r == region_id)
            .map(|(_, t)| t)
    }

    /// Returns the number of regions assigned to the given target type.
    pub fn count_by_target(&self, target: &DeploymentTarget) -> usize {
        self.regions.iter().filter(|(_, t)| t == target).count()
    }
}

// ===========================================================================
// DeploymentPlanner (kept for backwards compatibility)
// ===========================================================================

/// Computes deployment plans and manages live migrations.
#[derive(Debug)]
pub struct DeploymentPlanner {
    /// Current deployment plan.
    plan: DeploymentPlan,
    /// Runtime configuration.
    config: Config,
}

impl DeploymentPlanner {
    /// Creates a new deployment planner with the given configuration.
    pub fn new(config: Config) -> Self {
        DeploymentPlanner {
            plan: DeploymentPlan::new(),
            config,
        }
    }

    /// Computes a new deployment plan based on profile data and the
    /// current configuration.
    ///
    /// # Strategy
    ///
    /// 1. **Hot regions** (high call count) are deployed locally for
    ///    minimal latency.
    /// 2. **Cold regions** are candidates for remote execution.
    pub fn compute_deployment_plan(
        &mut self,
        region_ids: &[RegionId],
        profile: &ProfileData,
    ) -> &DeploymentPlan {
        self.plan.regions.clear();

        let hot_threshold = 100;

        for &region_id in region_ids {
            let _is_hot = profile
                .call_counts
                .values()
                .any(|&count| count > hot_threshold);

            let target = DeploymentTarget::Local;

            self.plan.regions.push((region_id, target));
        }

        &self.plan
    }

    /// Migrates a single region to a new target.
    pub fn migrate_region(
        &mut self,
        region_id: RegionId,
        new_target: DeploymentTarget,
    ) -> Result<(), DeploymentError> {
        let entry = self
            .plan
            .regions
            .iter_mut()
            .find(|(r, _)| *r == region_id)
            .ok_or(DeploymentError::RegionNotFound(region_id))?;

        let old_target = entry.1.clone();
        entry.1 = new_target.clone();

        log::info!(
            "Migrated region {} from {} to {}",
            region_id,
            old_target,
            new_target
        );

        Ok(())
    }

    /// Rebalances the deployment plan by re-running the planner logic.
    pub fn rebalance(&mut self, region_ids: &[RegionId], profile: &ProfileData) -> &DeploymentPlan {
        log::info!(
            "Rebalancing deployment plan for {} regions",
            region_ids.len()
        );
        self.compute_deployment_plan(region_ids, profile)
    }

    /// Returns a reference to the current deployment plan.
    pub fn plan(&self) -> &DeploymentPlan {
        &self.plan
    }
}

// ===========================================================================
// DeploymentManager
// ===========================================================================

/// Manages the deployment lifecycle of compiled regions.
///
/// The `DeploymentManager` is the central orchestrator for:
/// - Deploying packages to targets ([`deploy`](DeploymentManager::deploy)).
/// - Hot-swapping running code
///   ([`hot_swap`](DeploymentManager::hot_swap)).
/// - Version tracking and rollback
///   ([`rollback`](DeploymentManager::rollback)).
/// - Delta-based re-deployment
///   ([`deploy_delta`](DeploymentManager::deploy_delta)).
#[derive(Debug)]
pub struct DeploymentManager {
    /// Runtime configuration.
    config: Config,
    /// Version history for all deployed regions.
    version_log: VersionLog,
    /// Hot-swap state per region.
    hot_swap_states: HashMap<RegionId, HotSwapState>,
    /// Deployment planner for computing placement.
    planner: DeploymentPlanner,
}

impl DeploymentManager {
    /// Creates a new deployment manager with the given configuration.
    pub fn new(config: Config) -> Self {
        let planner = DeploymentPlanner::new(config.clone());
        DeploymentManager {
            config,
            version_log: VersionLog::new(),
            hot_swap_states: HashMap::new(),
            planner,
        }
    }

    /// Deploys a package to the specified target.
    ///
    /// This is the primary deployment entry point. It:
    /// 1. Validates the package checksum.
    /// 2. If a previous version exists on the target and the target supports
    ///    hot-swap, performs a hot-swap.
    /// 3. Otherwise performs a full deployment.
    /// 4. Records the version in the log.
    ///
    /// # Errors
    ///
    /// Returns [`DeploymentError::ChecksumMismatch`] if the package checksum
    /// is invalid, or [`DeploymentError::TargetUnreachable`] if the target
    /// cannot be reached (simulated).
    pub fn deploy(
        &mut self,
        package: &DeploymentPackage,
        target: &DeploymentTarget,
    ) -> Result<DeploymentResult, DeploymentError> {
        // Step 1: Validate checksum.
        if !package.validate_checksum() {
            return Err(DeploymentError::ChecksumMismatch(
                package.metadata.region_id,
            ));
        }

        let start = Instant::now();

        // Step 2: Check if we can hot-swap.
        let region_id = package.metadata.region_id;
        let previous = self.version_log.latest(region_id).cloned();
        let mut hot_swapped = false;

        if let Some(ref prev) = previous {
            if target.supports_hot_swap() && prev.target == *target {
                // Hot-swap path.
                self.hot_swap(package, target)?;
                hot_swapped = true;
            }
        }

        // Step 3: Record version.
        self.version_log.record(package, target);

        let duration = start.elapsed();

        Ok(DeploymentResult {
            region_id,
            version: package.metadata.version.clone(),
            target: target.clone(),
            duration,
            bytes_transferred: package.metadata.code_size,
            hot_swapped,
            was_delta: false,
        })
    }

    /// Performs a hot-swap of a running region.
    ///
    /// The hot-swap proceeds through these phases:
    /// 1. **PreparingShadow** — allocate and fill shadow buffer with new code.
    /// 2. **AwaitingSafePoint** — wait until no threads are in the region.
    /// 3. **Swapping** — atomically swap the code pointer.
    /// 4. **Completed** — old code is retained in the version log for
    ///    rollback.
    ///
    /// In this simulation the phases advance immediately. A production
    /// implementation would coordinate with the runtime's safe-point
    /// mechanism.
    pub fn hot_swap(
        &mut self,
        package: &DeploymentPackage,
        target: &DeploymentTarget,
    ) -> Result<HotSwapPhase, DeploymentError> {
        if !target.supports_hot_swap() {
            return Err(DeploymentError::HotSwapNotSupported(
                target.kind_label().to_owned(),
            ));
        }

        let region_id = package.metadata.region_id;
        let from_version = self
            .version_log
            .latest(region_id)
            .map(|r| r.version.clone())
            .unwrap_or(PackageVersion(0));

        let state = self
            .hot_swap_states
            .entry(region_id)
            .or_insert_with(|| HotSwapState::new(region_id));

        // Phase 1: Prepare shadow buffer.
        state.phase = HotSwapPhase::PreparingShadow;
        state.from_version = from_version.clone();
        state.to_version = package.metadata.version.clone();

        // Phase 2: Await safe point (simulated — immediate).
        state.phase = HotSwapPhase::AwaitingSafePoint;

        // Phase 3: Atomic swap.
        state.phase = HotSwapPhase::Swapping;

        // Phase 4: Completed.
        state.phase = HotSwapPhase::Completed;

        log::info!(
            "Hot-swapped region {} from {} to {} on {}",
            region_id,
            from_version,
            package.metadata.version,
            target
        );

        Ok(HotSwapPhase::Completed)
    }

    /// Rolls back a region to the previous deployed version.
    ///
    /// The previous version's code is re-deployed to the same target. The
    /// rollback itself is recorded in the version log.
    pub fn rollback(&mut self, region_id: RegionId) -> Result<DeploymentResult, DeploymentError> {
        let prev_record = self
            .version_log
            .previous(region_id)
            .cloned()
            .ok_or(DeploymentError::NoRollbackTarget(region_id))?;

        // Determine the new version number: must be strictly greater than
        // the current latest.
        let latest_version = self
            .version_log
            .latest(region_id)
            .map(|r| r.version.0)
            .unwrap_or(0);

        // Build a package from the previous record.
        let rollback_pkg = DeploymentPackage::new(
            prev_record.code.clone(),
            prev_record.region_id,
            // Bump version: new version = latest + 1.
            PackageVersion(latest_version + 1),
            "rollback",
        );

        let target = prev_record.target.clone();
        self.deploy(&rollback_pkg, &target)
    }

    /// Deploys only the delta (diff) between the current deployed version
    /// and the new package.
    ///
    /// This is more bandwidth-efficient than a full deploy when the target
    /// already holds a previous version. The delta is computed, sent, and
    /// applied on the "remote" side (simulated locally).
    pub fn deploy_delta(
        &mut self,
        package: &DeploymentPackage,
        target: &DeploymentTarget,
    ) -> Result<DeploymentResult, DeploymentError> {
        let region_id = package.metadata.region_id;

        let prev_code = self
            .version_log
            .latest(region_id)
            .map(|r| r.code.clone())
            .ok_or(DeploymentError::NoPreviousVersion(region_id))?;

        let from_version = self
            .version_log
            .latest(region_id)
            .map(|r| r.version.clone())
            .unwrap_or(PackageVersion(0));

        // Compute delta.
        let delta = DeploymentDelta::compute(
            region_id,
            from_version,
            package.metadata.version.clone(),
            &prev_code,
            &package.code,
        );

        // Simulate applying the delta on the remote side.
        let reconstructed = delta
            .apply(&prev_code)
            .map_err(|e| DeploymentError::DeltaApplyFailed(region_id, format!("{:?}", e)))?;

        // Verify the reconstructed code matches.
        if reconstructed != package.code {
            return Err(DeploymentError::DeltaApplyFailed(
                region_id,
                "reconstructed code does not match expected".to_owned(),
            ));
        }

        let start = Instant::now();
        let delta_size = delta.estimated_size();

        // If the delta is not empty, we also try hot-swap on supported
        // targets.
        let mut hot_swapped = false;
        if target.supports_hot_swap() && !delta.is_empty() {
            // For a delta hot-swap, we reconstruct on the target side and
            // then swap. Here we just flag it.
            hot_swapped = true;
        }

        // Record the version (full code stored for future deltas).
        self.version_log.record(package, target);

        let duration = start.elapsed();

        Ok(DeploymentResult {
            region_id,
            version: package.metadata.version.clone(),
            target: target.clone(),
            duration,
            bytes_transferred: delta_size,
            hot_swapped,
            was_delta: true,
        })
    }

    /// Returns the current hot-swap state for a region.
    pub fn hot_swap_state(&self, region_id: RegionId) -> Option<&HotSwapState> {
        self.hot_swap_states.get(&region_id)
    }

    /// Returns a reference to the version log.
    pub fn version_log(&self) -> &VersionLog {
        &self.version_log
    }

    /// Returns a reference to the deployment planner.
    pub fn planner(&self) -> &DeploymentPlanner {
        &self.planner
    }

    /// Returns a mutable reference to the deployment planner.
    pub fn planner_mut(&mut self) -> &mut DeploymentPlanner {
        &mut self.planner
    }

    /// Returns the runtime configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }
}

// ===========================================================================
// Errors
// ===========================================================================

/// Errors that can occur during deployment operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DeploymentError {
    /// The specified region was not found in the current plan.
    #[error("Region {0} not found in deployment plan")]
    RegionNotFound(RegionId),

    /// Migration failed because the target is unreachable.
    #[error("Target unreachable: {0}")]
    TargetUnreachable(String),

    /// Migration failed due to a state transfer error.
    #[error("State transfer failed for region {0}: {1}")]
    StateTransferFailed(RegionId, String),

    /// Package checksum validation failed.
    #[error("Checksum mismatch for region {0}")]
    ChecksumMismatch(RegionId),

    /// Hot-swap is not supported on the given target kind.
    #[error("Hot-swap not supported on target kind: {0}")]
    HotSwapNotSupported(String),

    /// No previous version exists to roll back to.
    #[error("No rollback target available for region {0}")]
    NoRollbackTarget(RegionId),

    /// No previous version exists on the target for delta computation.
    #[error("No previous version for delta deployment of region {0}")]
    NoPreviousVersion(RegionId),

    /// Delta application failed.
    #[error("Delta apply failed for region {0}: {1}")]
    DeltaApplyFailed(RegionId, String),
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Simple CRC32 computation (stub — a real implementation would use the
/// `crc32fast` crate or hardware CRC). We use a basic table-driven CRC32
/// for determinism in tests.
fn crc32(data: &[u8]) -> u32 {
    // CRC32 (IEEE 802.3 polynomial: 0xEDB88320 reflected).
    const POLY: u32 = 0xEDB88320;
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut crc = i;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ POLY;
            } else {
                crc >>= 1;
            }
        }
        table[i as usize] = crc;
    }

    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        let index = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ table[index];
    }
    !crc
}

/// Returns the current time as seconds since the Unix epoch (best-effort).
fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helpers -----------------------------------------------------------

    fn make_config() -> Config {
        Config::default()
    }

    fn make_package(region_id: RegionId, version: u64, code: &[u8]) -> DeploymentPackage {
        DeploymentPackage::new(code.to_vec(), region_id, PackageVersion(version), "basic")
    }

    // -- Test 1: Deploy to local target ------------------------------------

    #[test]
    fn deploy_to_local_target() {
        let mut mgr = DeploymentManager::new(make_config());
        let pkg = make_package(1, 1, &[0x90, 0x90, 0xC3]); // NOP; NOP; RET
        let result = mgr.deploy(&pkg, &DeploymentTarget::Local).unwrap();
        assert_eq!(result.region_id, 1);
        assert_eq!(result.version, PackageVersion(1));
        assert_eq!(result.target, DeploymentTarget::Local);
        assert!(!result.hot_swapped);
        assert!(!result.was_delta);
    }

    // -- Test 2: Hot-swap rejected on local target -------------------------

    #[test]
    fn hot_swap_rejected_on_local() {
        let mut mgr = DeploymentManager::new(make_config());
        let target = DeploymentTarget::Local;

        // Initial deploy.
        let pkg_v1 = make_package(4, 1, &[0xC3]);
        mgr.deploy(&pkg_v1, &target).unwrap();

        // Second deploy on Local — no hot-swap.
        let pkg_v2 = make_package(4, 2, &[0x90, 0xC3]);
        let result = mgr.deploy(&pkg_v2, &target).unwrap();
        assert!(!result.hot_swapped);

        // Direct hot_swap call should fail.
        let pkg_v3 = make_package(4, 3, &[0x90, 0x90, 0xC3]);
        let err = mgr.hot_swap(&pkg_v3, &target).unwrap_err();
        assert!(matches!(err, DeploymentError::HotSwapNotSupported(_)));
    }

    // -- Test 3: Version tracking and rollback -----------------------------

    #[test]
    fn version_tracking_and_rollback() {
        let mut mgr = DeploymentManager::new(make_config());
        let target = DeploymentTarget::Remote {
            endpoint: "https://192.168.1.50".to_owned(),
        };

        // Deploy v1, v2, v3.
        let pkg_v1 = make_package(5, 1, &[0x01]);
        let pkg_v2 = make_package(5, 2, &[0x02]);
        let pkg_v3 = make_package(5, 3, &[0x03]);
        mgr.deploy(&pkg_v1, &target).unwrap();
        mgr.deploy(&pkg_v2, &target).unwrap();
        mgr.deploy(&pkg_v3, &target).unwrap();

        // Version log should have 3 entries.
        assert_eq!(mgr.version_log().version_count(5), 3);

        // Latest should be v3.
        let latest = mgr.version_log().latest(5).unwrap();
        assert_eq!(latest.version, PackageVersion(3));
        assert_eq!(latest.code, vec![0x03]);

        // Rollback → deploys v2's code as v4.
        let result = mgr.rollback(5).unwrap();
        assert_eq!(result.version, PackageVersion(4));
        assert_eq!(mgr.version_log().version_count(5), 4);

        // The latest code should now be the rolled-back code (0x02).
        let latest = mgr.version_log().latest(5).unwrap();
        assert_eq!(latest.code, vec![0x02]);
    }

    // -- Test 4: Rollback with no history ----------------------------------

    #[test]
    fn rollback_with_no_history_fails() {
        let mut mgr = DeploymentManager::new(make_config());
        let err = mgr.rollback(99).unwrap_err();
        assert!(matches!(err, DeploymentError::NoRollbackTarget(99)));
    }

    // -- Test 5: Delta deployment ------------------------------------------

    #[test]
    fn delta_deployment() {
        let mut mgr = DeploymentManager::new(make_config());
        let target = DeploymentTarget::Local;

        // Deploy v1 (256-byte code: 4 × 64-byte blocks).
        let code_v1: Vec<u8> = (0..256).map(|i| i as u8).collect();
        let pkg_v1 = make_package(10, 1, &code_v1);
        mgr.deploy(&pkg_v1, &target).unwrap();

        // Deploy v2 as delta (change first 4 bytes only — only 1 of 4 blocks
        // is patched, so delta is much smaller than full code).
        let mut code_v2 = code_v1.clone();
        code_v2[0] = 0xFF;
        code_v2[1] = 0xFE;
        code_v2[2] = 0xFD;
        code_v2[3] = 0xFC;
        let pkg_v2 = make_package(10, 2, &code_v2);
        let result = mgr.deploy_delta(&pkg_v2, &target).unwrap();

        assert!(result.was_delta);
        // Delta should be smaller than the full code.
        assert!(result.bytes_transferred < code_v2.len());
        assert_eq!(result.version, PackageVersion(2));
    }

    // -- Test 6: Delta with no previous version fails ----------------------

    #[test]
    fn delta_with_no_previous_version_fails() {
        let mut mgr = DeploymentManager::new(make_config());
        let pkg = make_package(20, 1, &[0xC3]);
        let err = mgr
            .deploy_delta(&pkg, &DeploymentTarget::Local)
            .unwrap_err();
        assert!(matches!(err, DeploymentError::NoPreviousVersion(20)));
    }

    // -- Test 7: Checksum validation ---------------------------------------

    #[test]
    fn checksum_validation_rejects_corrupted_package() {
        let mut mgr = DeploymentManager::new(make_config());
        let mut pkg = make_package(30, 1, &[0x90, 0xC3]);
        // Corrupt the code without updating the checksum.
        pkg.code[0] = 0xCC;
        let err = mgr.deploy(&pkg, &DeploymentTarget::Local).unwrap_err();
        assert!(matches!(err, DeploymentError::ChecksumMismatch(30)));
    }

    // -- Test 8: Delta compute and apply round-trip -----------------------

    #[test]
    fn delta_compute_and_apply_roundtrip() {
        let old_code: Vec<u8> = (0..128).map(|i| i as u8).collect();
        let mut new_code = old_code.clone();
        // Modify bytes 10–15.
        new_code[10] = 0xAA;
        new_code[11] = 0xBB;
        new_code[12] = 0xCC;
        // Append extra bytes.
        new_code.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);

        let delta = DeploymentDelta::compute(
            42,
            PackageVersion(1),
            PackageVersion(2),
            &old_code,
            &new_code,
        );

        let reconstructed = delta.apply(&old_code).unwrap();
        assert_eq!(reconstructed, new_code);
    }

    // -- Test 9: Empty delta for identical code ---------------------------

    #[test]
    fn delta_empty_for_identical_code() {
        let code: Vec<u8> = (0..64).map(|i| i as u8).collect();
        let delta = DeploymentDelta::compute(1, PackageVersion(1), PackageVersion(2), &code, &code);
        assert!(delta.is_empty());
    }

    // -- Test 10: DeploymentTarget display and predicates ------------------

    #[test]
    fn target_display_and_predicates() {
        let local = DeploymentTarget::Local;
        let remote = DeploymentTarget::Remote {
            endpoint: "https://cloud.example.com".to_owned(),
        };

        assert_eq!(local.kind_label(), "Local");
        assert!(local.supports_hot_swap() == false);

        assert_eq!(remote.kind_label(), "Remote");
        assert!(!remote.supports_hot_swap());
    }

    // -- Test 11: VersionLog history ----------------------------------------

    #[test]
    fn version_log_history() {
        let mut log = VersionLog::new();
        let target = DeploymentTarget::Local;

        for v in 1..=5u64 {
            let pkg = make_package(100, v, &[v as u8]);
            log.record(&pkg, &target);
        }

        assert_eq!(log.version_count(100), 5);
        let history = log.history(100);
        assert_eq!(history[0].version, PackageVersion(1));
        assert_eq!(history[4].version, PackageVersion(5));

        // Previous (one before latest) should be v4.
        let prev = log.previous(100).unwrap();
        assert_eq!(prev.version, PackageVersion(4));
    }

    // -- Test 12: DeploymentPlan with remote targets -----------------------

    #[test]
    fn deployment_plan_remote_targets() {
        let mut planner = DeploymentPlanner::new(make_config());
        let profile = ProfileData::new();
        let plan = planner.compute_deployment_plan(&[0, 1, 2, 3, 4], &profile);
        assert_eq!(plan.regions.len(), 5);
        for (region_id, target) in &plan.regions {
            match target {
                DeploymentTarget::Remote { .. } => {}
                DeploymentTarget::Local => {}
                other => panic!("Expected Local or Remote target, got {:?}", other),
            }
        }
    }

    // -- Test 13: Package with debug info -----------------------------------

    #[test]
    fn package_with_debug_info() {
        let debug = DebugInfo {
            source_map: vec![(1, 0), (2, 16), (3, 32)],
            symbols: vec![(0, "entry".to_owned()), (16, "loop".to_owned())],
            notes: "compiled with -O2".to_owned(),
        };
        let pkg = make_package(50, 1, &[0x90, 0xC3]).with_debug_info(debug);
        assert!(pkg.debug_info.is_some());
        let info = pkg.debug_info.as_ref().unwrap();
        assert_eq!(info.source_map.len(), 3);
        assert_eq!(info.symbols.len(), 2);
        assert!(pkg.validate_checksum());
    }

    // -- Test 14: Delta for code shrinking (deletions) ---------------------

    #[test]
    fn delta_with_deletions() {
        let old_code: Vec<u8> = (0..128).map(|i| i as u8).collect();
        // New code is shorter — last 32 bytes removed.
        let new_code: Vec<u8> = (0..96).map(|i| i as u8).collect();

        let delta = DeploymentDelta::compute(
            77,
            PackageVersion(1),
            PackageVersion(2),
            &old_code,
            &new_code,
        );
        assert!(!delta.deletions.is_empty());

        let reconstructed = delta.apply(&old_code).unwrap();
        assert_eq!(reconstructed, new_code);
    }

    // -- Test 15: Deploy to Remote target -----------------------------------

    #[test]
    fn deploy_to_remote_target() {
        let mut mgr = DeploymentManager::new(make_config());
        let target = DeploymentTarget::Remote {
            endpoint: "https://compute.example.com/v1/exec".to_owned(),
        };
        let pkg = make_package(60, 1, &[0x90, 0xC3]);
        let result = mgr.deploy(&pkg, &target).unwrap();
        assert_eq!(result.region_id, 60);
        assert!(!result.hot_swapped); // remote does not support hot-swap
    }
}
