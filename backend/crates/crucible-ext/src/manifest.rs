//! The extension manifest, `extension.toml`.
//!
//! ```toml
//! [extension]
//! id        = "testbed"          # ID: lowercase ASCII letters, digits, `-`, `_`
//! version   = "0.1.0"            # extension version (SemVer)
//! core      = "^0.1"             # compatible core version range (SemVer)
//! id_prefix = "Test"             # optional prototype ID prefix
//!
//! [modules]
//! require = ["damage"]           # core modules, with dependencies resolved automatically
//!
//! [content]
//! roots = ["data"]               # data roots relative to the manifest
//!
//! [[content.overrides]]          # explicit overrides of prototypes from earlier roots
//! kind = "entity"
//! id   = "BaseItem"
//!
//! [ignore]                       # ignored components, grouped by category
//! client_only     = ["Sprite"]
//! not_implemented = ["Climbable"]
//!
//! [client]                       # adapter settings, ignored by the core
//! ```
//!
//! Unknown keys in `extension`, `modules`, `content`, or `ignore` cause manifest errors.
//! Unknown sections are also errors.

use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};

use crucible_proto::{DataRoot, IgnoreCategory};
use semver::{Version, VersionReq};
use toml::{Table, Value};

/// Manifest text and the base directory for relative paths.
#[derive(Clone, Debug)]
pub struct ManifestSource {
    /// Contents of `extension.toml`.
    pub text: Cow<'static, str>,
    /// Manifest directory.
    pub base_dir: PathBuf,
}

impl ManifestSource {
    /// Creates a source from manifest text (often `include_str!("../extension.toml")`) and the crate directory.
    pub fn new(text: impl Into<Cow<'static, str>>, base_dir: impl Into<PathBuf>) -> Self {
        ManifestSource {
            text: text.into(),
            base_dir: base_dir.into(),
        }
    }

    /// Reads a manifest source from a file.
    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok(ManifestSource::new(text, base_dir))
    }

    /// Manifest path used in diagnostics.
    pub fn display_path(&self) -> String {
        self.base_dir
            .join("extension.toml")
            .to_string_lossy()
            .replace('\\', "/")
    }
}

/// Explicit prototype override, keyed by kind and ID.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PrototypeOverride {
    /// Prototype kind.
    pub kind: String,
    /// Prototype ID.
    pub id: String,
}

/// Parsed extension manifest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExtensionManifest {
    /// Extension ID.
    pub id: String,
    /// Extension version.
    pub version: Version,
    /// Compatible core version range.
    pub core: VersionReq,
    /// Prototype ID prefix.
    pub id_prefix: Option<String>,
    /// Required core modules.
    pub modules: Vec<String>,
    /// Data roots, absolute or relative to the working directory.
    pub content_roots: Vec<PathBuf>,
    /// Explicit prototype overrides.
    pub overrides: Vec<PrototypeOverride>,
    /// Ignored components grouped by category.
    pub ignored: Vec<(String, IgnoreCategory)>,
}

/// Invalid manifest error.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ManifestError {
    /// Manifest path.
    pub path: String,
    /// Cause.
    pub message: String,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: invalid manifest: {}", self.path, self.message)
    }
}

impl std::error::Error for ManifestError {}

/// Incompatible core version error.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IncompatibleCore {
    /// Extension ID.
    pub extension: String,
    /// Required version range.
    pub required: String,
    /// Actual core version.
    pub actual: String,
}

impl fmt::Display for IncompatibleCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "extension `{}` requires core version {}, but this build has {}",
            self.extension, self.required, self.actual
        )
    }
}

impl std::error::Error for IncompatibleCore {}

impl ExtensionManifest {
    /// Parses and validates a manifest.
    pub fn parse(source: &ManifestSource) -> Result<Self, ManifestError> {
        let path = source.display_path();
        let err = |message: String| ManifestError {
            path: path.clone(),
            message,
        };
        let table: Table = toml::from_str(&source.text).map_err(|e| err(e.to_string()))?;
        for key in table.keys() {
            if !matches!(
                key.as_str(),
                "extension" | "modules" | "content" | "ignore" | "client"
            ) {
                return Err(err(format!("unknown section [{key}]")));
            }
        }

        let ext = section(&table, "extension")
            .map_err(&err)?
            .ok_or_else(|| err("missing [extension] section".into()))?;
        check_keys(ext, "extension", &["id", "version", "core", "id_prefix"]).map_err(&err)?;
        let id = required_str(ext, "extension", "id").map_err(&err)?;
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(err(format!(
                "extension.id `{id}` may contain only lowercase ASCII letters, digits, `-`, and `_`"
            )));
        }
        let version_text = required_str(ext, "extension", "version").map_err(&err)?;
        let version = Version::parse(&version_text)
            .map_err(|e| err(format!("extension.version «{version_text}»: {e}")))?;
        let core_text = required_str(ext, "extension", "core").map_err(&err)?;
        let core = VersionReq::parse(&core_text)
            .map_err(|e| err(format!("extension.core «{core_text}»: {e}")))?;
        let id_prefix = optional_str(ext, "extension", "id_prefix").map_err(&err)?;
        if id_prefix.as_deref() == Some("") {
            return Err(err("extension.id_prefix is empty".into()));
        }

        let mut modules = Vec::new();
        if let Some(m) = section(&table, "modules").map_err(&err)? {
            check_keys(m, "modules", &["require"]).map_err(&err)?;
            modules = string_list(m, "modules", "require").map_err(&err)?;
        }

        let mut content_roots = Vec::new();
        let mut overrides = Vec::new();
        if let Some(c) = section(&table, "content").map_err(&err)? {
            check_keys(c, "content", &["roots", "overrides"]).map_err(&err)?;
            content_roots = string_list(c, "content", "roots")
                .map_err(&err)?
                .into_iter()
                .map(|r| source.base_dir.join(r))
                .collect();
            if let Some(list) = c.get("overrides") {
                let list = list
                    .as_array()
                    .ok_or_else(|| err("content.overrides: expected an array of tables".into()))?;
                for item in list {
                    let t = item.as_table().ok_or_else(|| {
                        err("content.overrides: each entry must be a table with kind and id".into())
                    })?;
                    check_keys(t, "content.overrides", &["kind", "id"]).map_err(&err)?;
                    overrides.push(PrototypeOverride {
                        kind: required_str(t, "content.overrides", "kind").map_err(&err)?,
                        id: required_str(t, "content.overrides", "id").map_err(&err)?,
                    });
                }
            }
        }

        let mut ignored = Vec::new();
        if let Some(i) = section(&table, "ignore").map_err(&err)? {
            check_keys(i, "ignore", &["client_only", "not_implemented"]).map_err(&err)?;
            for name in string_list(i, "ignore", "client_only").map_err(&err)? {
                ignored.push((name, IgnoreCategory::ClientOnly));
            }
            for name in string_list(i, "ignore", "not_implemented").map_err(&err)? {
                ignored.push((name, IgnoreCategory::NotImplemented));
            }
        }

        Ok(ExtensionManifest {
            id,
            version,
            core,
            id_prefix,
            modules,
            content_roots,
            overrides,
            ignored,
        })
    }

    /// Checks whether core version `core` is compatible.
    pub fn check_core(&self, core: &Version) -> Result<(), IncompatibleCore> {
        if self.core.matches(core) {
            Ok(())
        } else {
            Err(IncompatibleCore {
                extension: self.id.clone(),
                required: self.core.to_string(),
                actual: core.to_string(),
            })
        }
    }

    /// Describes a prototype loader data root using `ext:<id>` as its name,
    /// with the configured prefix and explicit overrides.
    pub fn data_root(&self) -> DataRoot {
        let mut root = DataRoot::new(&format!("ext:{}", self.id));
        if let Some(prefix) = &self.id_prefix {
            root = root.with_id_prefix(prefix);
        }
        for o in &self.overrides {
            root = root.with_override(&o.kind, &o.id);
        }
        root
    }
}

fn section<'t>(table: &'t Table, name: &str) -> Result<Option<&'t Table>, String> {
    match table.get(name) {
        None => Ok(None),
        Some(Value::Table(t)) => Ok(Some(t)),
        Some(_) => Err(format!("[{name}] must be a table")),
    }
}

fn check_keys(table: &Table, section: &str, allowed: &[&str]) -> Result<(), String> {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "unknown key {section}.{key} (allowed: {})",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

fn required_str(table: &Table, section: &str, key: &str) -> Result<String, String> {
    optional_str(table, section, key)?
        .ok_or_else(|| format!("missing required key {section}.{key}"))
}

fn optional_str(table: &Table, section: &str, key: &str) -> Result<Option<String>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{section}.{key}: expected a string")),
    }
}

fn string_list(table: &Table, section: &str, key: &str) -> Result<Vec<String>, String> {
    match table.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{section}.{key}: expected an array of strings"))
            })
            .collect(),
        Some(_) => Err(format!("{section}.{key}: expected an array of strings")),
    }
}
