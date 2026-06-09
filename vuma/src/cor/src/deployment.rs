//! Adaptive deployment for the Continuous Optimization Runtime.
//!
//! The COR can distribute compiled regions across heterogeneous execution
//! targets: the local process, remote endpoints (e.g. a cloud compute
//! instance), or specific cores on a Raspberry Pi 5. The
//! [`DeploymentPlanner`] computes an optimal placement based on region
//! characteristics (hotness, size, latency sensitivity) and migrates
//! regions at runtime to rebalance load.

use crate::config::Config;
use crate::profile::ProfileData;
use crate::types::RegionId;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// DeploymentTarget
// ---------------------------------------------------------------------------

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

    /// Pin the region to a specific core on a Raspberry Pi 5 (Cortex-A76).
    Pi5Core {
        /// Core identifier (0–3 on the BCM2712).
        core_id: u32,
    },
}

impl std::fmt::Display for DeploymentTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentTarget::Local => write!(f, "Local"),
            DeploymentTarget::Remote { endpoint } => write!(f, "Remote({})", endpoint),
            DeploymentTarget::Pi5Core { core_id } => write!(f, "Pi5Core({})", core_id),
        }
    }
}

// ---------------------------------------------------------------------------
// DeploymentPlan
// ---------------------------------------------------------------------------

/// A deployment plan mapping each region to an execution target.
///
/// The plan is recomputed periodically (or when a significant profile
/// shift is detected) by [`compute_deployment_plan`].
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
        self.regions
            .iter()
            .filter(|(_, t)| t == target)
            .count()
    }
}

// ---------------------------------------------------------------------------
// Deployment planner
// ---------------------------------------------------------------------------

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
    /// 3. On Pi 5 targets, regions are round-robin-assigned to cores.
    pub fn compute_deployment_plan(
        &mut self,
        region_ids: &[RegionId],
        profile: &ProfileData,
    ) -> &DeploymentPlan {
        self.plan.regions.clear();

        let hot_threshold = 100;

        for &region_id in region_ids {
            // Heuristic: a region is "hot" if any of its nodes have been
            // called more than the threshold.
            let is_hot = profile
                .call_counts
                .values()
                .any(|&count| count > hot_threshold);

            let target = if self.config.is_pi5_target() {
                // Round-robin across Pi 5 cores (0–3).
                let core_id = (region_id % 4) as u32;
                DeploymentTarget::Pi5Core { core_id }
            } else if is_hot {
                DeploymentTarget::Local
            } else {
                // Cold regions could be offloaded; for now we keep them
                // local as a safe default.
                DeploymentTarget::Local
            };

            self.plan.regions.push((region_id, target));
        }

        &self.plan
    }

    /// Migrates a single region to a new target.
    ///
    /// In a full implementation this would involve:
    /// 1. Pausing execution of the region.
    /// 2. Serializing any live state.
    /// 3. Transferring state to the new target.
    /// 4. Resuming execution.
    ///
    /// This stub updates the plan and logs the migration.
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
    ///
    /// This is called periodically or when a significant shift in profile
    /// data is detected (e.g. a previously cold region becomes hot).
    pub fn rebalance(
        &mut self,
        region_ids: &[RegionId],
        profile: &ProfileData,
    ) -> &DeploymentPlan {
        log::info!("Rebalancing deployment plan for {} regions", region_ids.len());
        self.compute_deployment_plan(region_ids, profile)
    }

    /// Returns a reference to the current deployment plan.
    pub fn plan(&self) -> &DeploymentPlan {
        &self.plan
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TargetArch;

    fn make_config() -> Config {
        Config::default()
    }

    fn make_pi5_config() -> Config {
        Config::default().with_target_arch(TargetArch::ArmV8A)
    }

    #[test]
    fn compute_plan_local_by_default() {
        let mut planner = DeploymentPlanner::new(make_config());
        let profile = ProfileData::new();
        let plan = planner.compute_deployment_plan(&[1, 2, 3], &profile);
        assert_eq!(plan.regions.len(), 3);
        assert!(plan.regions.iter().all(|(_, t)| matches!(t, DeploymentTarget::Local)));
    }

    #[test]
    fn compute_plan_pi5_assigns_cores() {
        let mut planner = DeploymentPlanner::new(make_pi5_config());
        let profile = ProfileData::new();
        let plan = planner.compute_deployment_plan(&[0, 1, 2, 3, 4], &profile);
        assert_eq!(plan.regions.len(), 5);
        for (region_id, target) in &plan.regions {
            if let DeploymentTarget::Pi5Core { core_id } = target {
                assert_eq!(*core_id, (*region_id % 4) as u32);
            } else {
                panic!("Expected Pi5Core target, got {:?}", target);
            }
        }
    }

    #[test]
    fn migrate_region_updates_plan() {
        let mut planner = DeploymentPlanner::new(make_config());
        let profile = ProfileData::new();
        planner.compute_deployment_plan(&[1], &profile);
        planner
            .migrate_region(1, DeploymentTarget::Remote {
                endpoint: "https://compute.example.com".to_owned(),
            })
            .unwrap();
        let target = planner.plan().target_for(1).unwrap();
        assert!(matches!(target, DeploymentTarget::Remote { .. }));
    }

    #[test]
    fn migrate_nonexistent_region_errors() {
        let mut planner = DeploymentPlanner::new(make_config());
        let profile = ProfileData::new();
        planner.compute_deployment_plan(&[1], &profile);
        let result = planner.migrate_region(999, DeploymentTarget::Local);
        assert!(result.is_err());
    }
}
