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

use serde::{Deserialize, Serialize};
use std::fmt;

/// Helper to provide a default empty TOML table.
fn toml_default_table() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

/// Helper to provide a default empty TOML array.
fn toml_default_array() -> toml::Value {
    toml::Value::Array(Vec::new())
}

// ---------------------------------------------------------------------------
// PackageManifest
// ---------------------------------------------------------------------------

/// The parsed representation of a `vuma.pkg` manifest file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackageManifest {
    /// Package name (must be a valid identifier: lowercase, hyphens allowed).
    pub name: String,
    /// Semantic version string (e.g. "0.1.0").
    pub version: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// List of package dependencies.
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    /// Build targets (binaries, libraries, tests).
    #[serde(default)]
    pub targets: Vec<PackageTarget>,
}

impl PackageManifest {
    /// Parse a manifest from a TOML string.
    pub fn from_toml(toml_str: &str) -> Result<Self, toml::de::Error> {
        #[derive(Deserialize)]
        struct RawManifest {
            package: RawPackage,
            #[serde(default = "toml_default_table")]
            dependencies: toml::Value,
            #[serde(default = "toml_default_array")]
            target: toml::Value,
        }

        #[derive(Deserialize)]
        struct RawPackage {
            name: String,
            version: String,
            #[serde(default)]
            description: Option<String>,
        }

        let raw: RawManifest = toml::from_str(toml_str)?;

        // Parse dependencies from the [dependencies] section
        let dependencies = Self::parse_dependencies(&raw.dependencies)?;

        // Parse targets from [[target]] array
        let targets = Self::parse_targets(&raw.target)?;

        Ok(PackageManifest {
            name: raw.package.name,
            version: raw.package.version,
            description: raw.package.description,
            dependencies,
            targets,
        })
    }

    /// Serialize the manifest to a TOML string.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        #[derive(Serialize)]
        struct RawManifest {
            package: RawPackage,
            dependencies: toml::Value,
            target: Vec<RawTarget>,
        }

        #[derive(Serialize)]
        struct RawPackage {
            name: String,
            version: String,
            description: Option<String>,
        }

        #[derive(Serialize)]
        struct RawTarget {
            name: String,
            kind: String,
            src: String,
        }

        let package = RawPackage {
            name: self.name.clone(),
            version: self.version.clone(),
            description: self.description.clone(),
        };

        // Build dependencies as a TOML table
        let mut dep_table = toml::map::Map::new();
        for dep in &self.dependencies {
            if let Some(ref registry) = dep.registry {
                let mut dep_table_inner = toml::map::Map::new();
                dep_table_inner.insert(
                    "version".to_string(),
                    toml::Value::String(dep.version.clone()),
                );
                dep_table_inner.insert(
                    "registry".to_string(),
                    toml::Value::String(registry.clone()),
                );
                dep_table.insert(
                    dep.name.clone(),
                    toml::Value::Table(dep_table_inner),
                );
            } else {
                dep_table.insert(
                    dep.name.clone(),
                    toml::Value::String(dep.version.clone()),
                );
            }
        }

        let targets: Vec<RawTarget> = self
            .targets
            .iter()
            .map(|t| RawTarget {
                name: t.name.clone(),
                kind: t.kind.to_string(),
                src: t.src.clone(),
            })
            .collect();

        let raw = RawManifest {
            package,
            dependencies: toml::Value::Table(dep_table),
            target: targets,
        };

        toml::to_string_pretty(&raw)
    }

    /// Parse the `[dependencies]` section.
    fn parse_dependencies(value: &toml::Value) -> Result<Vec<Dependency>, toml::de::Error> {
        let mut deps = Vec::new();

        match value {
            toml::Value::Table(table) => {
                for (name, val) in table {
                    match val {
                        toml::Value::String(version) => {
                            deps.push(Dependency {
                                name: name.clone(),
                                version: version.clone(),
                                registry: None,
                            });
                        }
                        toml::Value::Table(inner) => {
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
            _ => {}
        }

        Ok(deps)
    }

    /// Parse the `[[target]]` array.
    fn parse_targets(value: &toml::Value) -> Result<Vec<PackageTarget>, toml::de::Error> {
        let mut targets = Vec::new();

        match value {
            toml::Value::Array(arr) => {
                for item in arr {
                    if let toml::Value::Table(table) = item {
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
            _ => {}
        }

        Ok(targets)
    }
}

// ---------------------------------------------------------------------------
// Dependency
// ---------------------------------------------------------------------------

/// A single package dependency.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackageTarget {
    /// Target name (used as the output binary name).
    pub name: String,
    /// Kind of target (binary, library, test, example).
    pub kind: TargetKind,
    /// Source file path relative to the package root.
    pub src: String,
}

/// The kind of build target.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
pub fn parse_manifest(toml_str: &str) -> Result<PackageManifest, toml::de::Error> {
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
            dependencies: vec![
                Dependency {
                    name: "vuma-std".to_string(),
                    version: "0.1".to_string(),
                    registry: None,
                },
            ],
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
