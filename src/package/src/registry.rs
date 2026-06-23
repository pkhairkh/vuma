//! Local file-based package registry.
//!
//! The registry stores packages at `~/.vuma/registry/` with the following
//! layout:
//!
//! ```text
//! ~/.vuma/registry/
//! ├── index.toml                         — Global package index
//! ├── vuma-std/
//! │   └── 0.1.0/
//! │       ├── vuma.pkg                   — Package manifest
//! │       └── src/
//! │           └── ...
//! └── vuma-crypto/
//!     └── 0.2.0/
//!         ├── vuma.pkg
//!         └── src/
//!             └── ...
//! ```

use crate::manifest::PackageManifest;
use crate::{PackageError, PackageResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// RegistryIndex
// ---------------------------------------------------------------------------

/// The global registry index, mapping package names to available versions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistryIndex {
    /// Map from package name to a list of available version strings.
    pub packages: HashMap<String, Vec<String>>,
}

impl RegistryIndex {
    /// Create an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a version entry for a package.
    pub fn add_version(&mut self, name: &str, version: &str) {
        self.packages
            .entry(name.to_string())
            .or_default()
            .push(version.to_string());
    }

    /// Get the list of available versions for a package.
    pub fn get_versions(&self, name: &str) -> &[String] {
        self.packages
            .get(name)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

// ---------------------------------------------------------------------------
// PackageRegistry
// ---------------------------------------------------------------------------

/// A local file-based package registry.
///
/// The registry lives at `~/.vuma/registry/` and stores package manifests
/// and source files organized by name and version.
pub struct PackageRegistry {
    /// Root directory of the registry.
    root: PathBuf,
}

impl PackageRegistry {
    /// Create a registry rooted at the given directory.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Create a registry using the default location (`~/.vuma/registry/`).
    pub fn default_path() -> PathBuf {
        dirs_home().join(".vuma").join("registry")
    }

    /// Get the root directory of this registry.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Initialize the registry directory structure.
    pub fn init(&self) -> PackageResult<()> {
        std::fs::create_dir_all(&self.root)?;
        let index_path = self.root.join("index.toml");
        if !index_path.exists() {
            let index = RegistryIndex::new();
            let toml_str = toml::to_string_pretty(&index)
                .map_err(|e| PackageError::Other(e.to_string()))?;
            std::fs::write(&index_path, toml_str)?;
        }
        Ok(())
    }

    /// Read the registry index.
    pub fn read_index(&self) -> PackageResult<RegistryIndex> {
        let index_path = self.root.join("index.toml");
        if !index_path.exists() {
            return Ok(RegistryIndex::new());
        }
        let content = std::fs::read_to_string(&index_path)?;
        let index: RegistryIndex = toml::from_str(&content)
            .map_err(|e| PackageError::ManifestParse(e.to_string()))?;
        Ok(index)
    }

    /// Write the registry index.
    pub fn write_index(&self, index: &RegistryIndex) -> PackageResult<()> {
        let index_path = self.root.join("index.toml");
        let toml_str = toml::to_string_pretty(index)
            .map_err(|e| PackageError::Other(e.to_string()))?;
        std::fs::write(&index_path, toml_str)?;
        Ok(())
    }

    /// Publish a package to the registry from a source directory string.
    ///
    /// Copies the package manifest and source files into the registry.
    /// The manifest provides the package name and version; `source` is the
    /// path to the package root directory containing `vuma.pkg` and `src/`.
    pub fn publish(&self, manifest: &PackageManifest, source: &str) -> PackageResult<()> {
        let source_dir = Path::new(source);
        self.init()?;

        let pkg_dir = self.root.join(&manifest.name).join(&manifest.version);
        if pkg_dir.exists() {
            return Err(PackageError::AlreadyExists(format!(
                "{}@{} already exists in the registry",
                manifest.name, manifest.version
            )));
        }

        // Copy manifest
        let src_pkg = source_dir.join("vuma.pkg");
        if src_pkg.exists() {
            std::fs::create_dir_all(&pkg_dir)?;
            std::fs::copy(&src_pkg, pkg_dir.join("vuma.pkg"))?;
        }

        // Copy source directory
        let src_dir = source_dir.join("src");
        if src_dir.exists() {
            let dest_src = pkg_dir.join("src");
            copy_dir_recursive(&src_dir, &dest_src)?;
        }

        // Update index
        let mut index = self.read_index()?;
        index.add_version(&manifest.name, &manifest.version);
        self.write_index(&index)?;

        Ok(())
    }

    /// Get a package manifest from the registry.
    pub fn get_manifest(&self, name: &str, version: &str) -> PackageResult<PackageManifest> {
        let pkg_path = self.root.join(name).join(version).join("vuma.pkg");
        if !pkg_path.exists() {
            return Err(PackageError::DependencyNotFound(
                name.to_string(),
                version.to_string(),
            ));
        }
        let content = std::fs::read_to_string(&pkg_path)?;
        PackageManifest::from_toml(&content).map_err(|e| PackageError::ManifestParse(e.to_string()))
    }

    /// Get the source directory for a package in the registry.
    pub fn get_source_dir(&self, name: &str, version: &str) -> PackageResult<PathBuf> {
        let src_dir = self.root.join(name).join(version).join("src");
        if !src_dir.exists() {
            return Err(PackageError::DependencyNotFound(
                name.to_string(),
                version.to_string(),
            ));
        }
        Ok(src_dir)
    }

    /// Find the best matching version for a version requirement.
    ///
    /// Currently implements simple prefix matching (e.g. "0.1" matches "0.1.0",
    /// "0.1.3", etc.; picks the highest matching version).
    pub fn find_version(&self, name: &str, version_req: &str) -> PackageResult<String> {
        let index = self.read_index()?;
        let versions = index.get_versions(name);

        if versions.is_empty() {
            return Err(PackageError::DependencyNotFound(
                name.to_string(),
                version_req.to_string(),
            ));
        }

        // Exact match
        if versions.contains(&version_req.to_string()) {
            return Ok(version_req.to_string());
        }

        // Prefix match: "0.1" matches "0.1.0", "0.1.3", etc.
        let prefix = if version_req.ends_with('.') {
            version_req.to_string()
        } else {
            format!("{}.", version_req)
        };

        let mut matching: Vec<&String> = versions
            .iter()
            .filter(|v| v.starts_with(&prefix) || **v == version_req)
            .collect();

        if matching.is_empty() {
            return Err(PackageError::DependencyNotFound(
                name.to_string(),
                version_req.to_string(),
            ));
        }

        // Sort and pick the highest version
        matching.sort();
        Ok(matching.last().unwrap().to_string())
    }

    /// List all packages in the registry.
    pub fn list_packages(&self) -> PackageResult<Vec<(String, Vec<String>)>> {
        let index = self.read_index()?;
        let mut result: Vec<_> = index
            .packages
            .iter()
            .map(|(name, versions)| (name.clone(), versions.clone()))
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(result)
    }

    /// Fetch a package from the registry by name and version.
    ///
    /// Returns the manifest and the path to the package's source directory.
    pub fn fetch(&self, name: &str, version: &str) -> PackageResult<(PackageManifest, String)> {
        let manifest = self.get_manifest(name, version)?;
        let source_dir = self.get_source_dir(name, version)?;
        Ok((manifest, source_dir.to_string_lossy().to_string()))
    }

    /// List all packages as flat (name, version) pairs.
    ///
    /// Unlike [`list_packages`](Self::list_packages) which groups versions,
    /// this returns one entry per package-version combination.
    pub fn list(&self) -> PackageResult<Vec<(String, String)>> {
        let index = self.read_index()?;
        let mut result: Vec<(String, String)> = index
            .packages
            .iter()
            .flat_map(|(name, versions)| {
                versions
                    .iter()
                    .map(|v| (name.clone(), v.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        Ok(result)
    }
}

impl Default for PackageRegistry {
    fn default() -> Self {
        Self::new(Self::default_path())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the user's home directory.
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> PackageResult<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_index_roundtrip() {
        let mut index = RegistryIndex::new();
        index.add_version("vuma-std", "0.1.0");
        index.add_version("vuma-std", "0.2.0");
        index.add_version("vuma-crypto", "0.1.0");

        let toml_str = toml::to_string_pretty(&index).unwrap();
        let parsed: RegistryIndex = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.get_versions("vuma-std"), &["0.1.0", "0.2.0"]);
        assert_eq!(parsed.get_versions("vuma-crypto"), &["0.1.0"]);
        assert!(parsed.get_versions("nonexistent").is_empty());
    }

    #[test]
    fn test_find_version_prefix_match() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let registry = PackageRegistry::new(tmp_dir.path().to_path_buf());
        registry.init().unwrap();

        // Write an index manually
        let mut index = RegistryIndex::new();
        index.add_version("vuma-std", "0.1.0");
        index.add_version("vuma-std", "0.1.3");
        index.add_version("vuma-std", "0.2.0");
        registry.write_index(&index).unwrap();

        // Prefix match should pick highest
        assert_eq!(registry.find_version("vuma-std", "0.1").unwrap(), "0.1.3");
        // Exact match
        assert_eq!(registry.find_version("vuma-std", "0.2.0").unwrap(), "0.2.0");
    }
}
