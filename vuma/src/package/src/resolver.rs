//! Dependency resolver for VUMA packages.
//!
//! The resolver performs topological sorting of the dependency graph to
//! determine a valid build order. It uses the local file-based registry
//! to look up package manifests and their transitive dependencies.

use crate::manifest::{Dependency, PackageManifest};
use crate::registry::PackageRegistry;
use crate::{PackageError, PackageResult};
use std::collections::{HashMap, HashSet, VecDeque};

// ---------------------------------------------------------------------------
// ResolvedPackage
// ---------------------------------------------------------------------------

/// A package that has been resolved from the registry.
#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    /// Package name.
    pub name: String,
    /// Resolved version string.
    pub version: String,
    /// Parsed manifest (if available).
    pub manifest: Option<PackageManifest>,
}

// ---------------------------------------------------------------------------
// ResolveResult
// ---------------------------------------------------------------------------

/// The result of dependency resolution.
#[derive(Debug, Clone)]
pub struct ResolveResult {
    /// Packages in topological order (dependencies first).
    pub packages: Vec<ResolvedPackage>,
    /// The full dependency graph (package name → its dependencies).
    pub graph: HashMap<String, Vec<String>>,
}

// ---------------------------------------------------------------------------
// DependencyResolver
// ---------------------------------------------------------------------------

/// Resolves package dependencies from a local registry.
///
/// The resolver:
/// 1. Walks the dependency tree starting from the root package
/// 2. Looks up each dependency in the registry
/// 3. Recursively resolves transitive dependencies
/// 4. Detects circular dependencies
/// 5. Returns a topologically sorted list of packages
pub struct DependencyResolver {
    /// The package registry to look up dependencies from.
    registry: PackageRegistry,
}

impl DependencyResolver {
    /// Create a new resolver backed by the given registry.
    pub fn new(registry: PackageRegistry) -> Self {
        Self { registry }
    }

    /// Resolve all dependencies for the given root manifest.
    ///
    /// Returns a topologically sorted list of packages (dependencies first,
    /// so that each package appears before any package that depends on it).
    pub fn resolve(&self, root: &PackageManifest) -> PackageResult<ResolveResult> {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        let mut resolved: HashMap<String, ResolvedPackage> = HashMap::new();
        let mut visiting: HashSet<String> = HashSet::new();
        let mut visited: HashSet<String> = HashSet::new();

        // Resolve the root package's dependencies recursively
        self.resolve_recursive(
            &root.name,
            &root.version,
            &root.dependencies,
            &mut graph,
            &mut resolved,
            &mut visiting,
            &mut visited,
        )?;

        // Topological sort using Kahn's algorithm
        let sorted = self.topological_sort(&graph)?;

        // Build the final list in topological order
        let packages: Vec<ResolvedPackage> = sorted
            .into_iter()
            .filter_map(|name| resolved.remove(&name))
            .collect();

        Ok(ResolveResult { packages, graph })
    }

    /// Recursively resolve dependencies.
    fn resolve_recursive(
        &self,
        name: &str,
        version: &str,
        deps: &[Dependency],
        graph: &mut HashMap<String, Vec<String>>,
        resolved: &mut HashMap<String, ResolvedPackage>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
    ) -> PackageResult<()> {
        let key = name.to_string();
        let _ = version; // version used for registry lookup below

        // Already fully processed
        if visited.contains(&key) {
            return Ok(());
        }

        // Circular dependency detected
        if visiting.contains(&key) {
            return Err(PackageError::CircularDependency(key));
        }

        visiting.insert(key.clone());

        // Look up each dependency in the registry
        let mut dep_names = Vec::new();
        for dep in deps {
            let resolved_version = self.registry.find_version(&dep.name, &dep.version)?;

            // Get the manifest for this dependency
            let dep_manifest = self
                .registry
                .get_manifest(&dep.name, &resolved_version)
                .ok();

            // Record the resolved package
            resolved.insert(
                dep.name.clone(),
                ResolvedPackage {
                    name: dep.name.clone(),
                    version: resolved_version.clone(),
                    manifest: dep_manifest.clone(),
                },
            );

            dep_names.push(dep.name.clone());

            // Recursively resolve this dependency's dependencies
            if let Some(ref manifest) = dep_manifest {
                self.resolve_recursive(
                    &dep.name,
                    &resolved_version,
                    &manifest.dependencies,
                    graph,
                    resolved,
                    visiting,
                    visited,
                )?;
            }
        }

        graph.insert(key.clone(), dep_names);
        visiting.remove(&key);
        visited.insert(key);

        Ok(())
    }

    /// Topological sort of the dependency graph using Kahn's algorithm.
    fn topological_sort(
        &self,
        graph: &HashMap<String, Vec<String>>,
    ) -> PackageResult<Vec<String>> {
        // Compute in-degrees
        let mut in_degree: HashMap<&String, usize> = HashMap::new();
        for node in graph.keys() {
            in_degree.entry(node).or_insert(0);
        }
        for deps in graph.values() {
            for dep in deps {
                *in_degree.entry(dep).or_insert(0) += 1;
            }
        }

        // Initialize queue with nodes that have no incoming edges
        let mut queue: VecDeque<&String> = VecDeque::new();
        for (node, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(node);
            }
        }

        let mut sorted = Vec::new();
        while let Some(node) = queue.pop_front() {
            sorted.push(node.clone());
            if let Some(deps) = graph.get(node) {
                for dep in deps {
                    if let Some(degree) = in_degree.get_mut(dep) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dep);
                        }
                    }
                }
            }
        }

        // Reverse so that dependencies come before dependents (build order).
        sorted.reverse();

        if sorted.len() != graph.len() {
            // There's a cycle (should have been caught earlier, but just in case)
            let remaining: Vec<_> = graph
                .keys()
                .filter(|k| !sorted.contains(k))
                .cloned()
                .collect();
            return Err(PackageError::CircularDependency(remaining.join(" -> ")));
        }

        Ok(sorted)
    }
}

// ---------------------------------------------------------------------------
// Standalone convenience function
// ---------------------------------------------------------------------------

/// Resolve all dependencies for a package manifest using a local registry path.
///
/// This is a convenience function that creates a registry and resolver,
/// then returns the resolved manifests in topological order.
pub fn resolve_dependencies(
    manifest: &PackageManifest,
    registry_path: &std::path::Path,
) -> PackageResult<Vec<PackageManifest>> {
    let registry = PackageRegistry::new(registry_path.to_path_buf());
    let resolver = DependencyResolver::new(registry);
    let result = resolver.resolve(manifest)?;
    Ok(result
        .packages
        .into_iter()
        .filter_map(|rp| rp.manifest)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{PackageTarget, TargetKind};

    fn make_manifest(name: &str, version: &str, deps: Vec<Dependency>) -> PackageManifest {
        PackageManifest {
            name: name.to_string(),
            version: version.to_string(),
            description: None,
            dependencies: deps,
            targets: vec![PackageTarget {
                name: name.to_string(),
                kind: TargetKind::Bin,
                src: "src/main.vuma".to_string(),
            }],
        }
    }

    #[test]
    fn test_resolve_no_deps() {
        let manifest = make_manifest("app", "0.1.0", vec![]);
        let registry = PackageRegistry::new(std::path::PathBuf::from("/tmp/test-registry"));
        let resolver = DependencyResolver::new(registry);
        let result = resolver.resolve(&manifest).unwrap();
        assert!(result.packages.is_empty());
    }

    #[test]
    fn test_topological_sort_simple() {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        graph.insert("app".to_string(), vec!["lib-a".to_string(), "lib-b".to_string()]);
        graph.insert("lib-a".to_string(), vec!["lib-c".to_string()]);
        graph.insert("lib-b".to_string(), vec!["lib-c".to_string()]);
        graph.insert("lib-c".to_string(), vec![]);

        let registry = PackageRegistry::new(std::path::PathBuf::from("/tmp/test-registry"));
        let resolver = DependencyResolver::new(registry);
        let sorted = resolver.topological_sort(&graph).unwrap();

        // lib-c should come before lib-a and lib-b, which should come before app
        let pos: HashMap<&str, usize> = sorted.iter().map(|s| (s.as_str(), sorted.iter().position(|x| x == s).unwrap())).collect();
        assert!(pos["lib-c"] < pos["lib-a"]);
        assert!(pos["lib-c"] < pos["lib-b"]);
        assert!(pos["lib-a"] < pos["app"]);
        assert!(pos["lib-b"] < pos["app"]);
    }
}
