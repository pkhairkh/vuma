//! # VUMA Package Manager
//!
//! Provides the package management infrastructure for the VUMA ecosystem:
//!
//! - **`vuma.pkg` manifest** — TOML-based package descriptor (name, version,
//!   dependencies, targets)
//! - **`PackageManifest`** — parsed representation of a `vuma.pkg` file
//! - **`DependencyResolver`** — resolves package dependencies from a registry
//!   (local file-based for now)
//! - **`PackageRegistry`** — simple local file-based registry at `~/.vuma/registry/`
//!
//! # CLI Integration
//!
//! The package manager is wired into the `vuma pkg` subcommand:
//!
//! ```text
//! vuma pkg init           — Create a new vuma.pkg in the current directory
//! vuma pkg build          — Build the package and its dependencies
//! vuma pkg add <dep>      — Add a dependency to vuma.pkg
//! ```

pub mod manifest;
pub mod registry;
pub mod resolver;

pub use manifest::{PackageManifest, PackageTarget, Dependency, TargetKind, parse_manifest};
pub use registry::PackageRegistry;
pub use resolver::{DependencyResolver, ResolveResult, resolve_dependencies};

use thiserror::Error;

/// Errors that can occur during package operations.
#[derive(Debug, Error)]
pub enum PackageError {
    /// I/O error reading or writing files.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error parsing a `vuma.pkg` TOML file.
    #[error("manifest parse error: {0}")]
    ManifestParse(String),

    /// A dependency was not found in the registry.
    #[error("dependency not found: {0}@{1}")]
    DependencyNotFound(String, String),

    /// A version conflict was detected during resolution.
    #[error("version conflict for {name}: required {required}, found {found}")]
    VersionConflict {
        /// Package name.
        name: String,
        /// Required version string.
        required: String,
        /// Version found in the registry.
        found: String,
    },

    /// A circular dependency was detected.
    #[error("circular dependency detected: {0}")]
    CircularDependency(String),

    /// The package directory already exists.
    #[error("package already exists: {0}")]
    AlreadyExists(String),

    /// Generic package error.
    #[error("{0}")]
    Other(String),
}

/// Result type for package operations.
pub type PackageResult<T> = Result<T, PackageError>;

/// Initialize a new package in the given directory.
///
/// Creates a `vuma.pkg` manifest file with default values.
pub fn init_package(dir: &std::path::Path, name: &str) -> PackageResult<()> {
    let pkg_path = dir.join("vuma.pkg");
    if pkg_path.exists() {
        return Err(PackageError::AlreadyExists(pkg_path.display().to_string()));
    }

    let manifest = PackageManifest {
        name: name.to_string(),
        version: "0.1.0".to_string(),
        description: Some(format!("VUMA package: {}", name)),
        dependencies: Vec::new(),
        targets: vec![PackageTarget {
            name: name.to_string(),
            kind: TargetKind::Bin,
            src: "src/main.vuma".to_string(),
        }],
    };

    let toml_str = manifest.to_toml().map_err(|e| PackageError::ManifestParse(e.to_string()))?;
    std::fs::write(&pkg_path, toml_str)?;

    // Create src directory
    let src_dir = dir.join("src");
    if !src_dir.exists() {
        std::fs::create_dir_all(&src_dir)?;
    }

    // Create a minimal main.vuma
    let main_path = src_dir.join("main.vuma");
    if !main_path.exists() {
        std::fs::write(&main_path, "fn main() {\n    // Entry point\n}\n")?;
    }

    Ok(())
}

/// Add a dependency to the package manifest in the given directory.
pub fn add_dependency(dir: &std::path::Path, dep_name: &str, version: &str) -> PackageResult<()> {
    let pkg_path = dir.join("vuma.pkg");
    if !pkg_path.exists() {
        return Err(PackageError::ManifestParse(
            "no vuma.pkg found in the current directory".to_string(),
        ));
    }

    let content = std::fs::read_to_string(&pkg_path)?;
    let mut manifest = PackageManifest::from_toml(&content)
        .map_err(|e| PackageError::ManifestParse(e.to_string()))?;

    // Check if dependency already exists
    if manifest.dependencies.iter().any(|d| d.name == dep_name) {
        return Err(PackageError::Other(format!(
            "dependency '{}' already exists",
            dep_name
        )));
    }

    manifest.dependencies.push(Dependency {
        name: dep_name.to_string(),
        version: version.to_string(),
        registry: None,
    });

    let toml_str = manifest.to_toml().map_err(|e| PackageError::ManifestParse(e.to_string()))?;
    std::fs::write(&pkg_path, toml_str)?;

    Ok(())
}

/// Build the package in the given directory.
///
/// Resolves dependencies, then compiles all targets.
pub fn build_package(dir: &std::path::Path) -> PackageResult<()> {
    let pkg_path = dir.join("vuma.pkg");
    if !pkg_path.exists() {
        return Err(PackageError::ManifestParse(
            "no vuma.pkg found in the current directory".to_string(),
        ));
    }

    let content = std::fs::read_to_string(&pkg_path)?;
    let manifest = PackageManifest::from_toml(&content)
        .map_err(|e| PackageError::ManifestParse(e.to_string()))?;

    log::info!("Building package {} v{}", manifest.name, manifest.version);

    // Resolve dependencies
    let registry = PackageRegistry::default();
    let resolver = DependencyResolver::new(registry);
    let resolved = resolver.resolve(&manifest)?;

    if !resolved.packages.is_empty() {
        log::info!("Resolved {} dependencies:", resolved.packages.len());
        for pkg in &resolved.packages {
            log::info!("  {} v{}", pkg.name, pkg.version);
        }
    }

    // Build each target
    for target in &manifest.targets {
        log::info!("Building target: {} ({:?})", target.name, target.kind);
        let src_path = dir.join(&target.src);
        if !src_path.exists() {
            log::warn!("Source file not found: {}", src_path.display());
        }
        // The actual compilation would be delegated to the main pipeline.
        // For now we just validate the manifest and resolve deps.
    }

    Ok(())
}
