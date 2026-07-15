//! Package manifest — the `vuma.pkg` file format.
//!
//! The manifest is a TOML file that describes a VUMA package:
//!
//! ```toml
//! [package]
//! name = "my-app"
//! version = "0.1.0"
//! description = "A VUMA application"
//!
//! [dependencies]
//! vuma-std = "0.1"
//! vuma-crypto = "0.2"
//!
//! [[target]]
//! name = "my-app"
//! kind = "bin"
//! src = "src/main.vuma"
//! ```

use std::collections::BTreeMap;
use std::fmt;

use crate::PackageError;
use crate::toml_lite::{self, Value};

// ---------------------------------------------------------------------------
// PackageManifest
// ---------------------------------------------------------------------------

/// The parsed representation of a `vuma.pkg` manifest file.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageManifest {
    /// Package name (must be a valid identifier: lowercase, hyphens allowed).
    pub name: String,
    /// Semantic version string (e.g. "0.1.0").
    pub version: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// List of package dependencies.
    pub dependencies: Vec<Dependency>,
    /// Build targets (binaries, libraries, tests).
    pub targets: Vec<PackageTarget>,
}

impl PackageManifest {
    /// Parse a manifest from a TOML string.
    ///
    /// Wave 43 serde-migration: this previously used `#[derive(Deserialize)]`
    /// helper structs (`RawManifest`/`RawPackage`) and a third-party TOML
    /// deserializer. It now uses the in-tree `toml_lite` parser and
    /// navigates the value tree by hand. The on-disk TOML format is
    /// unchanged.
    ///
    /// The error type changed from a third-party deserialize error to
    /// `PackageError` — `PackageError::ManifestParse` carries the
    /// human-readable message for both low-level TOML syntax errors and
    /// high-level missing-field validation errors.
    pub fn from_toml(toml_str: &str) -> Result<Self, PackageError> {
        let root = toml_lite::parse(toml_str)
            .map_err(|e| PackageError::ManifestParse(e.to_string()))?;
        let package = root
            .get("package")
            .and_then(|v| v.as_table())
            .ok_or_else(|| {
                PackageError::ManifestParse("missing `[package]` section in manifest".to_string())
            })?;
        let name = package
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                PackageError::ManifestParse("missing `package.name` field".to_string())
            })?
            .to_string();
        let version = package
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                PackageError::ManifestParse("missing `package.version` field".to_string())
            })?
            .to_string();
        let description = package
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let dependencies_value = root
            .get("dependencies")
            .cloned()
            .unwrap_or_else(Value::table);
        let dependencies = Self::parse_dependencies(&dependencies_value)?;

        let target_value = root
            .get("target")
            .cloned()
            .unwrap_or_else(Value::array);
        let targets = Self::parse_targets(&target_value)?;

        Ok(PackageManifest {
            name,
            version,
            description,
            dependencies,
            targets,
        })
    }

    /// Serialize the manifest to a TOML string.
    ///
    /// Wave 43 serde-migration: this previously used `#[derive(Serialize)]`
    /// helper structs (`RawManifest`/`RawPackage`/`RawTarget`) and a
    /// third-party TOML serializer. It now builds a `toml_lite::Value::Table`
    /// by hand and serializes that with the in-tree `toml_lite` serializer.
    /// The on-disk TOML format is unchanged.
    ///
    /// The error type changed from a third-party serialize error to
    /// `PackageError` — `PackageError::Other` carries the human-readable
    /// message.
    pub fn to_toml(&self) -> Result<String, PackageError> {
        let mut package_table = BTreeMap::new();
        package_table.insert("name".to_string(), Value::String(self.name.clone()));
        package_table.insert(
            "version".to_string(),
            Value::String(self.version.clone()),
        );
        if let Some(ref desc) = self.description {
            package_table.insert("description".to_string(), Value::String(desc.clone()));
        }

        // Build dependencies as a TOML table
        let mut dep_table = BTreeMap::new();
        for dep in &self.dependencies {
            if let Some(ref registry) = dep.registry {
                let mut dep_table_inner = BTreeMap::new();
                dep_table_inner.insert(
                    "version".to_string(),
                    Value::String(dep.version.clone()),
                );
                dep_table_inner.insert(
                    "registry".to_string(),
                    Value::String(registry.clone()),
                );
                dep_table.insert(dep.name.clone(), Value::Table(dep_table_inner));
            } else {
                dep_table.insert(dep.name.clone(), Value::String(dep.version.clone()));
            }
        }

        // Build targets as a TOML array of tables
        let targets_arr: Vec<Value> = self
            .targets
            .iter()
            .map(|t| {
                let mut tbl = BTreeMap::new();
                tbl.insert("name".to_string(), Value::String(t.name.clone()));
                tbl.insert("kind".to_string(), Value::String(t.kind.to_string()));
                tbl.insert("src".to_string(), Value::String(t.src.clone()));
                Value::Table(tbl)
            })
            .collect();

        let mut root = BTreeMap::new();
        root.insert("package".to_string(), Value::Table(package_table));
        root.insert("dependencies".to_string(), Value::Table(dep_table));
        root.insert("target".to_string(), Value::Array(targets_arr));

        toml_lite::to_string_pretty(&Value::Table(root))
            .map_err(|e| PackageError::Other(e.to_string()))
    }

    /// Parse the `[dependencies]` section.
    fn parse_dependencies(value: &Value) -> Result<Vec<Dependency>, PackageError> {
        let mut deps = Vec::new();

        if let Value::Table(table) = value {
            for (name, val) in table {
                match val {
                    Value::String(version) => {
                        deps.push(Dependency {
                            name: name.clone(),
                            version: version.clone(),
                            registry: None,
                        });
                    }
                    Value::Table(inner) => {
                        let version = inner
                            .get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or("*")
                            .to_string();
                        let registry = inner
                            .get("registry")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        deps.push(Dependency {
                            name: name.clone(),
                            version,
                            registry,
                        });
                    }
                    _ => {}
                }
            }
        }

        Ok(deps)
    }

    /// Parse the `[[target]]` array.
    fn parse_targets(value: &Value) -> Result<Vec<PackageTarget>, PackageError> {
        let mut targets = Vec::new();

        if let Value::Array(arr) = value {
            for item in arr {
                if let Value::Table(table) = item {
                    let name = table
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("main")
                        .to_string();
                    let kind = table
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("bin");
                    let kind = match kind {
                        "lib" => TargetKind::Lib,
                        "test" => TargetKind::Test,
                        "example" => TargetKind::Example,
                        _ => TargetKind::Bin,
                    };
                    let src = table
                        .get("src")
                        .and_then(|v| v.as_str())
                        .unwrap_or("src/main.vuma")
                        .to_string();
                    targets.push(PackageTarget { name, kind, src });
                }
            }
        }

        Ok(targets)
    }
}

// ---------------------------------------------------------------------------
// Dependency
// ---------------------------------------------------------------------------

/// A single package dependency.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    /// Dependency package name.
    pub name: String,
    /// Version requirement string (semver range, e.g. "0.1", "^1.0", "*").
    pub version: String,
    /// Optional registry source (defaults to the local registry).
    pub registry: Option<String>,
}

// ---------------------------------------------------------------------------
// PackageTarget
// ---------------------------------------------------------------------------

/// A build target within a package.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageTarget {
    /// Target name (used as the output binary name).
    pub name: String,
    /// Kind of target (binary, library, test, example).
    pub kind: TargetKind,
    /// Source file path relative to the package root.
    pub src: String,
}

/// The kind of build target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// Binary executable.
    Bin,
    /// Library (compiled unit, can be imported by other packages).
    Lib,
    /// Test target.
    Test,
    /// Example binary.
    Example,
}

impl fmt::Display for TargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetKind::Bin => write!(f, "bin"),
            TargetKind::Lib => write!(f, "lib"),
            TargetKind::Test => write!(f, "test"),
            TargetKind::Example => write!(f, "example"),
        }
    }
}

// ---------------------------------------------------------------------------
// Standalone convenience functions
// ---------------------------------------------------------------------------

/// Parse a `vuma.pkg` TOML string into a `PackageManifest`.
///
/// This is a convenience wrapper around [`PackageManifest::from_toml`].
pub fn parse_manifest(toml_str: &str) -> Result<PackageManifest, PackageError> {
    PackageManifest::from_toml(toml_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_manifest() {
        let manifest = PackageManifest {
            name: "test-pkg".to_string(),
            version: "0.1.0".to_string(),
            description: Some("A test package".to_string()),
            dependencies: vec![Dependency {
                name: "vuma-std".to_string(),
                version: "0.1".to_string(),
                registry: None,
            }],
            targets: vec![PackageTarget {
                name: "test-pkg".to_string(),
                kind: TargetKind::Bin,
                src: "src/main.vuma".to_string(),
            }],
        };

        let toml_str = manifest.to_toml().unwrap();
        let parsed = PackageManifest::from_toml(&toml_str).unwrap();
        assert_eq!(manifest, parsed);
    }

    #[test]
    fn test_parse_minimal_manifest() {
        let toml_str = r#"
[package]
name = "hello"
version = "0.1.0"

[dependencies]

[[target]]
name = "hello"
kind = "bin"
src = "src/main.vuma"
"#;
        let manifest = PackageManifest::from_toml(toml_str).unwrap();
        assert_eq!(manifest.name, "hello");
        assert_eq!(manifest.version, "0.1.0");
        assert!(manifest.dependencies.is_empty());
        assert_eq!(manifest.targets.len(), 1);
        assert_eq!(manifest.targets[0].kind, TargetKind::Bin);
    }
}
