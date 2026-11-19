//! Load errors and warnings with source locations and prototype identifiers.

use std::fmt;
use std::sync::Arc;

use crate::node::Pos;
use crate::parse::ParseErrorKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// Kind and ID of the prototype associated with a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtoRef {
    pub kind: Arc<str>,
    pub id: Arc<str>,
}

impl fmt::Display for ProtoRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} `{}`", self.kind, self.id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    // ---- Errors
    Parse(ParseErrorKind),
    /// The file does not follow the `- type: …` record layout, such as a missing `type` or `id`.
    MalformedRecord(String),
    UnknownPrototypeKind {
        kind: String,
    },
    DuplicateId {
        first_file: Arc<str>,
        first_pos: Pos,
    },
    UnknownParent {
        parent: String,
    },
    InheritanceCycle {
        cycle: Vec<String>,
    },
    UnknownComponent {
        component: String,
    },
    UnknownField {
        owner: String,
        field: String,
    },
    /// The value does not match the field. `expected` describes the expected type.
    TypeMismatch {
        expected: String,
        found: String,
    },
    UnknownPolymorphicType {
        base: String,
        name: String,
    },
    MissingPolymorphicTag {
        base: String,
    },
    UnexpectedTypeTag {
        tag: String,
    },
    UnknownReference {
        kind: String,
        id: String,
    },
    /// A module-level check across prototypes, such as multiple empty tiles.
    /// These checks run after all files load and do not belong to a single record.
    ModuleCheck {
        /// Module identifier.
        module: String,
        /// Error description.
        message: String,
    },
    // ---- Warnings
    MissingIdPrefix {
        prefix: String,
    },
    /// Summary of components that are still unimplemented on the backend.
    NotImplementedComponents {
        components: Vec<String>,
        prototypes: usize,
    },
    MissingLocKey {
        key: String,
    },
}

impl DiagnosticKind {
    pub fn severity(&self) -> Severity {
        match self {
            DiagnosticKind::MissingIdPrefix { .. }
            | DiagnosticKind::NotImplementedComponents { .. }
            | DiagnosticKind::MissingLocKey { .. } => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use DiagnosticKind as K;
        match self {
            K::Parse(k) => write!(f, "{k}"),
            K::MalformedRecord(m) => write!(f, "invalid prototype record: {m}"),
            K::UnknownPrototypeKind { kind } => write!(f, "unknown prototype kind `{kind}`"),
            K::DuplicateId {
                first_file,
                first_pos,
            } => {
                write!(
                    f,
                    "ID is already used by the prototype from {first_file}:{first_pos}; declare an override"
                )
            }
            K::UnknownParent { parent } => write!(f, "parent `{parent}` was not found"),
            K::InheritanceCycle { cycle } => write!(f, "inheritance cycle: {}", cycle.join(" -> ")),
            K::UnknownComponent { component } => {
                write!(
                    f,
                    "component `{component}` is neither registered nor ignored"
                )
            }
            K::UnknownField { owner, field } => write!(f, "{owner} has no field `{field}`"),
            K::TypeMismatch { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            K::UnknownPolymorphicType { base, name } => {
                write!(f, "type `{name}` is not registered for base `{base}`")
            }
            K::MissingPolymorphicTag { base } => {
                write!(f, "a value for base `{base}` requires a `!type:Name` tag")
            }
            K::UnexpectedTypeTag { tag } => write!(f, "unexpected tag `!type:{tag}`"),
            K::UnknownReference { kind, id } => write!(f, "prototype {kind} `{id}` was not found"),
            K::ModuleCheck { module, message } => write!(f, "module `{module}`: {message}"),
            K::MissingIdPrefix { prefix } => {
                write!(f, "ID is missing extension prefix `{prefix}`")
            }
            K::NotImplementedComponents {
                components,
                prototypes,
            } => write!(
                f,
                "unimplemented backend components skipped ({} types in {prototypes} prototypes): {}",
                components.len(),
                components.join(", ")
            ),
            K::MissingLocKey { key } => write!(
                f,
                "localization key `{key}` is missing in the default language"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: Option<Arc<str>>,
    pub pos: Option<Pos>,
    pub proto: Option<ProtoRef>,
    pub kind: DiagnosticKind,
}

impl Diagnostic {
    pub fn severity(&self) -> Severity {
        self.kind.severity()
    }
}

impl fmt::Display for Diagnostic {
    /// Formats a diagnostic as `path:line:column: severity:` followed by prototype context and the message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file, self.pos) {
            (Some(file), Some(pos)) => write!(f, "{file}:{pos}: ")?,
            (Some(file), None) => write!(f, "{file}: ")?,
            _ => {}
        }
        let sev = if self.severity() == Severity::Error {
            "error"
        } else {
            "warning"
        };
        write!(f, "{sev}: ")?;
        if let Some(p) = &self.proto {
            write!(f, "{p}: ")?;
        }
        write!(f, "{}", self.kind)
    }
}

/// All load errors and warnings in discovery order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity() == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity() == Severity::Warning)
    }

    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    pub(crate) fn push(&mut self, d: Diagnostic) {
        self.diagnostics.push(d);
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in &self.diagnostics {
            writeln!(f, "{d}")?;
        }
        let e = self.errors().count();
        let w = self.warnings().count();
        write!(f, "total errors: {e}, warnings: {w}")
    }
}
