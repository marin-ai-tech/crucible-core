//! Prototype loader for indexing, overrides, prefixes, inheritance, and validation.
//!
//! Add roots in core, module, then active extension order. `add_source` parses and indexes
//! a file. `finish` resolves inheritance, validates all records, and returns
//! the resolved prototype set and the complete diagnostic report.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use crucible_loc::LocKeys;

use crate::diag::{Diagnostic, DiagnosticKind, ProtoRef, Report};
use crate::merge::{Merged, merge, own_parts};
use crate::node::{Node, Pos, parse_bool};
use crate::parse::parse_str;
use crate::schema::Registry;
use crate::set::{Prototype, PrototypeSet};
use crate::validate::{Ctx, validate_entry};

/// Data root owned by the core, a module, or an extension.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataRoot {
    pub name: String,
    /// ID prefix from the extension manifest. IDs without it generate a warning.
    pub id_prefix: Option<String>,
    /// Explicit overrides that fully replace prototypes from earlier roots,
    /// identified by (kind, ID).
    pub overrides: Vec<(String, String)>,
}

impl DataRoot {
    pub fn new(name: &str) -> Self {
        DataRoot {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    pub fn with_id_prefix(mut self, prefix: &str) -> Self {
        self.id_prefix = Some(prefix.to_owned());
        self
    }

    pub fn with_override(mut self, kind: &str, id: &str) -> Self {
        self.overrides.push((kind.to_owned(), id.to_owned()));
        self
    }
}

/// Data root index within the loader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootId(usize);

/// Prototype record before inheritance is resolved.
pub(crate) struct Entry {
    pub kind: Arc<str>,
    pub id: Arc<str>,
    pub file: Arc<str>,
    pub pos: Pos,
    pub root: RootId,
    pub is_abstract: bool,
    pub parents: Vec<(String, Pos)>,
    /// The complete record mapping.
    pub body: Node,
}

impl Entry {
    fn proto_ref(&self) -> ProtoRef {
        ProtoRef {
            kind: self.kind.clone(),
            id: self.id.clone(),
        }
    }

    fn diag(&self, pos: Pos, kind: DiagnosticKind) -> Diagnostic {
        Diagnostic {
            file: Some(self.file.clone()),
            pos: Some(pos),
            proto: Some(self.proto_ref()),
            kind,
        }
    }
}

type Index = HashMap<Arc<str>, HashMap<Arc<str>, usize>>;

/// The resolved prototype set and full diagnostics.
#[derive(Debug)]
pub struct LoadResult {
    pub set: PrototypeSet,
    pub report: Report,
}

impl LoadResult {
    /// Returns the set if no errors occurred. A server must not open its lobby on `Err`.
    pub fn into_result(self) -> Result<(PrototypeSet, Report), Report> {
        if self.report.has_errors() {
            Err(self.report)
        } else {
            Ok((self.set, self.report))
        }
    }
}

pub struct PrototypeLoader<'r> {
    registry: &'r Registry,
    roots: Vec<DataRoot>,
    entries: Vec<Entry>,
    index: Index,
    report: Report,
}

impl<'r> PrototypeLoader<'r> {
    pub fn new(registry: &'r Registry) -> Self {
        PrototypeLoader {
            registry,
            roots: Vec::new(),
            entries: Vec::new(),
            index: HashMap::new(),
            report: Report::default(),
        }
    }

    /// Adds a data root. Call order determines load order and override precedence.
    pub fn add_root(&mut self, root: DataRoot) -> RootId {
        self.roots.push(root);
        RootId(self.roots.len() - 1)
    }

    /// Recursively parses `*.yml` and `*.yaml` files in path order.
    pub fn add_dir(&mut self, root: RootId, dir: &Path) -> std::io::Result<()> {
        let mut files = Vec::new();
        collect_yaml(dir, &mut files)?;
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path)?;
            self.add_source(root, &path.to_string_lossy().replace('\\', "/"), &text);
        }
        Ok(())
    }

    /// Parses and indexes a source file, appending errors to the report.
    pub fn add_source(&mut self, root: RootId, file: &str, text: &str) {
        let file: Arc<str> = file.into();
        let docs = match parse_str(text) {
            Ok(docs) => docs,
            Err(errors) => {
                for e in errors {
                    self.report.push(Diagnostic {
                        file: Some(file.clone()),
                        pos: Some(e.pos),
                        proto: None,
                        kind: DiagnosticKind::Parse(e.kind),
                    });
                }
                return;
            }
        };
        for doc in docs {
            if doc.is_null() {
                continue;
            }
            let Some(items) = doc.as_seq() else {
                self.malformed(&file, doc.pos, "the top level must be a list of records");
                continue;
            };
            for item in items {
                self.add_record(root, &file, item.clone());
            }
        }
    }

    fn malformed(&mut self, file: &Arc<str>, pos: Pos, what: &str) {
        self.report.push(Diagnostic {
            file: Some(file.clone()),
            pos: Some(pos),
            proto: None,
            kind: DiagnosticKind::MalformedRecord(what.to_owned()),
        });
    }

    fn add_record(&mut self, root: RootId, file: &Arc<str>, body: Node) {
        if body.as_map().is_none() {
            self.malformed(file, body.pos, "a record must be a mapping");
            return;
        }
        let Some(kind_node) = body
            .get("type")
            .filter(|n| n.as_str().is_some_and(|s| !s.is_empty()))
        else {
            self.malformed(file, body.pos, "record is missing its `type` field");
            return;
        };
        let Some(id_node) = body
            .get("id")
            .filter(|n| n.as_str().is_some_and(|s| !s.is_empty()))
        else {
            self.malformed(file, body.pos, "record is missing its `id` field");
            return;
        };
        let kind: Arc<str> = kind_node.as_str().unwrap_or_default().into();
        let id: Arc<str> = id_node.as_str().unwrap_or_default().into();
        let pos = body.pos;
        let id_pos = id_node.pos;
        let kind_pos = kind_node.pos;
        let proto = ProtoRef {
            kind: kind.clone(),
            id: id.clone(),
        };

        if self.registry.kind(&kind).is_none() {
            self.report.push(Diagnostic {
                file: Some(file.clone()),
                pos: Some(kind_pos),
                proto: Some(proto),
                kind: DiagnosticKind::UnknownPrototypeKind {
                    kind: kind.to_string(),
                },
            });
            return;
        }

        let root_info = &self.roots[root.0];
        if let Some(prefix) = &root_info.id_prefix
            && !id.starts_with(prefix.as_str())
        {
            self.report.push(Diagnostic {
                file: Some(file.clone()),
                pos: Some(id_pos),
                proto: Some(proto.clone()),
                kind: DiagnosticKind::MissingIdPrefix {
                    prefix: prefix.clone(),
                },
            });
        }
        let is_override = root_info
            .overrides
            .iter()
            .any(|(k, i)| **k == *kind && **i == *id);

        let is_abstract = body
            .get("abstract")
            .and_then(Node::as_str)
            .and_then(parse_bool)
            .unwrap_or(false);
        let parents = match body.get("parent") {
            Some(p) => match (p.as_str(), p.as_seq()) {
                (Some(s), _) => vec![(s.to_owned(), p.pos)],
                (None, Some(items)) => items
                    .iter()
                    .filter_map(|n| n.as_str().map(|s| (s.to_owned(), n.pos)))
                    .collect(),
                _ => Vec::new(),
            },
            None => Vec::new(),
        };
        let entry = Entry {
            kind: kind.clone(),
            id: id.clone(),
            file: file.clone(),
            pos,
            root,
            is_abstract,
            parents,
            body,
        };

        match self.index.get(&kind).and_then(|m| m.get(&id)).copied() {
            None => {
                self.index
                    .entry(kind)
                    .or_default()
                    .insert(id, self.entries.len());
                self.entries.push(entry);
            }
            Some(existing) => {
                let first = &self.entries[existing];
                if is_override && first.root != root {
                    self.entries[existing] = entry;
                } else {
                    let (first_file, first_pos) = (first.file.clone(), first.pos);
                    self.report.push(entry.diag(
                        id_pos,
                        DiagnosticKind::DuplicateId {
                            first_file,
                            first_pos,
                        },
                    ));
                }
            }
        }
    }

    /// Resolves inheritance, validates all records, and builds the prototype set.
    ///
    /// `loc` checks keys against the default language. Pass no provider to skip key validation.
    pub fn finish(self, loc: Option<&dyn LocKeys>) -> LoadResult {
        let PrototypeLoader {
            registry,
            entries,
            index,
            mut report,
            ..
        } = self;

        let mut resolver = Resolver {
            entries: &entries,
            index: &index,
            memo: vec![Memo::New; entries.len()],
            stack: Vec::new(),
        };
        for i in 0..entries.len() {
            resolver.resolve(i, &mut report);
        }
        let memo = resolver.memo;

        let exists = |kind: &str, id: &str| index.get(kind).is_some_and(|m| m.contains_key(id));
        let mut not_implemented: BTreeSet<String> = BTreeSet::new();
        let mut not_implemented_protos = 0usize;
        for entry in &entries {
            let mut ctx = Ctx {
                registry,
                exists: &exists,
                loc,
                report: &mut report,
                file: entry.file.clone(),
                proto: entry.proto_ref(),
                not_implemented: BTreeSet::new(),
            };
            validate_entry(entry, &mut ctx);
            if !ctx.not_implemented.is_empty() {
                not_implemented_protos += 1;
                not_implemented.extend(ctx.not_implemented);
            }
        }
        if !not_implemented.is_empty() {
            report.push(Diagnostic {
                file: None,
                pos: None,
                proto: None,
                kind: DiagnosticKind::NotImplementedComponents {
                    components: not_implemented.into_iter().collect(),
                    prototypes: not_implemented_protos,
                },
            });
        }

        let mut set = PrototypeSet::default();
        for (entry, memo) in entries.iter().zip(memo) {
            let Memo::Done(Some(merged)) = memo else {
                continue;
            };
            let components = merged
                .components
                .iter()
                .filter(|c| registry.component(&c.name).is_some())
                .cloned()
                .collect();
            let proto = Prototype {
                kind: entry.kind.clone(),
                id: entry.id.clone(),
                is_abstract: entry.is_abstract,
                file: entry.file.clone(),
                pos: entry.pos,
                parents: entry.parents.iter().map(|(p, _)| p.clone()).collect(),
                fields: merged.fields.clone(),
                components,
            };
            set.by_kind
                .entry(entry.kind.clone())
                .or_default()
                .insert(entry.id.clone(), Arc::new(proto));
        }
        LoadResult { set, report }
    }
}

fn collect_yaml(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    for item in std::fs::read_dir(dir)? {
        let path = item?.path();
        if path.is_dir() {
            collect_yaml(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
            out.push(path);
        }
    }
    Ok(())
}

#[derive(Clone)]
enum Memo {
    New,
    Visiting,
    Done(Option<Rc<Merged>>),
}

struct Resolver<'a> {
    entries: &'a [Entry],
    index: &'a Index,
    memo: Vec<Memo>,
    stack: Vec<usize>,
}

impl Resolver<'_> {
    /// Resolved fields and components, or `None` for a missing parent or inheritance cycle.
    fn resolve(&mut self, idx: usize, report: &mut Report) -> Option<Rc<Merged>> {
        match &self.memo[idx] {
            Memo::Done(r) => return r.clone(),
            Memo::Visiting => return None,
            Memo::New => {}
        }
        self.memo[idx] = Memo::Visiting;
        self.stack.push(idx);

        let entries = self.entries;
        let entry = &entries[idx];
        let mut acc = Merged::default();
        let mut ok = true;
        for (name, pos) in &entry.parents {
            let parent = self
                .index
                .get(&entry.kind)
                .and_then(|m| m.get(name.as_str()))
                .copied();
            let Some(pidx) = parent else {
                report.push(entry.diag(
                    *pos,
                    DiagnosticKind::UnknownParent {
                        parent: name.clone(),
                    },
                ));
                ok = false;
                continue;
            };
            if matches!(self.memo[pidx], Memo::Visiting) {
                let start = self.stack.iter().position(|&i| i == pidx).unwrap_or(0);
                let mut cycle: Vec<String> = self.stack[start..]
                    .iter()
                    .map(|&i| entries[i].id.to_string())
                    .collect();
                cycle.push(entries[pidx].id.to_string());
                report.push(entry.diag(*pos, DiagnosticKind::InheritanceCycle { cycle }));
                ok = false;
                continue;
            }
            match self.resolve(pidx, report) {
                Some(m) => merge(&mut acc, &m),
                None => ok = false,
            }
        }
        self.stack.pop();

        let result = ok.then(|| {
            merge(&mut acc, &own_parts(&entry.body));
            Rc::new(acc)
        });
        self.memo[idx] = Memo::Done(result.clone());
        result
    }
}
