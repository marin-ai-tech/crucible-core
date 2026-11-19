//! Compact YAML node tree for loading map files.
//!
//! SS14 maps are much larger than ordinary prototypes. Earlier map parse trees
//! used 0.8–1.3 GB. Here each node takes 24 bytes, mapping keys and common short
//! scalars are interned as `u32`, long scalars use `Box<str>`, and children use
//! tightly sized `Box<[_]>`. Locations retain only line numbers. As in `crucible-proto`,
//! scalars retain their raw text, mapping keys must be scalar, duplicate keys are errors,
//! and the only supported tag prefix is `!type:`.

use std::collections::HashMap;
use std::rc::Rc;

use crucible_proto::{Node, Pos, Scalar, Value};
use saphyr_parser::{Event, Parser, ScalarStyle};

const TYPE_TAG_PREFIX: &str = "!type:";
/// Maximum length of a scalar eligible for interning, such as a position or chunk index.
const SHORT_SCALAR: usize = 8;

/// Table of interned strings.
#[derive(Default)]
pub(crate) struct Atoms {
    by_text: HashMap<Rc<str>, u32>,
    texts: Vec<Rc<str>>,
}

impl Atoms {
    fn intern(&mut self, text: &str) -> u32 {
        if let Some(&id) = self.by_text.get(text) {
            return id;
        }
        let id = self.texts.len() as u32;
        let rc: Rc<str> = Rc::from(text);
        self.texts.push(rc.clone());
        self.by_text.insert(rc, id);
        id
    }

    pub(crate) fn find(&self, text: &str) -> Option<u32> {
        self.by_text.get(text).copied()
    }

    pub(crate) fn text(&self, id: u32) -> &str {
        &self.texts[id as usize]
    }
}

/// Node with `tag` set to zero or the `!type:` name index plus one. Scalar
/// `quoted` indicates a style other than plain.
#[derive(Clone, Debug)]
pub(crate) enum CNode {
    Text {
        tag: u16,
        quoted: bool,
        line: u32,
        text: Box<str>,
    },
    Atom {
        tag: u16,
        quoted: bool,
        line: u32,
        atom: u32,
    },
    Seq {
        tag: u16,
        line: u32,
        items: Box<[CNode]>,
    },
    Map {
        tag: u16,
        line: u32,
        entries: Box<[(u32, CNode)]>,
    },
}

impl CNode {
    pub(crate) fn line(&self) -> u32 {
        match self {
            CNode::Text { line, .. }
            | CNode::Atom { line, .. }
            | CNode::Seq { line, .. }
            | CNode::Map { line, .. } => *line,
        }
    }

    fn tag(&self) -> u16 {
        match self {
            CNode::Text { tag, .. }
            | CNode::Atom { tag, .. }
            | CNode::Seq { tag, .. }
            | CNode::Map { tag, .. } => *tag,
        }
    }

    pub(crate) fn as_str<'a>(&'a self, tree: &'a MapTree) -> Option<&'a str> {
        match self {
            CNode::Text { text, .. } => Some(text),
            CNode::Atom { atom, .. } => Some(tree.atoms.text(*atom)),
            _ => None,
        }
    }

    pub(crate) fn as_seq(&self) -> Option<&[CNode]> {
        match self {
            CNode::Seq { items, .. } => Some(items),
            _ => None,
        }
    }

    pub(crate) fn as_map(&self) -> Option<&[(u32, CNode)]> {
        match self {
            CNode::Map { entries, .. } => Some(entries),
            _ => None,
        }
    }

    /// Value by mapping key, using a linear search over interned key IDs.
    pub(crate) fn get(&self, tree: &MapTree, key: &str) -> Option<&CNode> {
        let id = tree.atoms.find(key)?;
        self.as_map()?
            .iter()
            .find(|(k, _)| *k == id)
            .map(|(_, v)| v)
    }

    /// Copies the subtree into a `crucible-proto` node with column 0 for unknown columns.
    pub(crate) fn to_proto(&self, tree: &MapTree) -> Node {
        let pos = Pos {
            line: self.line(),
            col: 0,
        };
        let value = match self {
            CNode::Text { quoted, text, .. } => Value::Scalar(Scalar {
                text: text.clone(),
                plain: !quoted,
            }),
            CNode::Atom { quoted, atom, .. } => Value::Scalar(Scalar {
                text: tree.atoms.text(*atom).into(),
                plain: !quoted,
            }),
            CNode::Seq { items, .. } => {
                Value::Seq(items.iter().map(|n| n.to_proto(tree)).collect())
            }
            CNode::Map { entries, .. } => Value::Map(
                entries
                    .iter()
                    .map(|(k, v)| {
                        (
                            Node::scalar(
                                Pos {
                                    line: v.line(),
                                    col: 0,
                                },
                                tree.atoms.text(*k),
                            ),
                            v.to_proto(tree),
                        )
                    })
                    .collect(),
            ),
        };
        let tag = self.tag();
        Node {
            type_tag: (tag != 0).then(|| tree.tags[usize::from(tag) - 1].clone()),
            pos,
            value,
        }
    }
}

/// A parsed map file.
pub(crate) struct MapTree {
    pub(crate) atoms: Atoms,
    tags: Vec<Box<str>>,
    pub(crate) docs: Vec<CNode>,
}

/// Parse error with a one-based line number and message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TreeError {
    pub line: u32,
    pub message: String,
}

enum Frame {
    Seq {
        tag: u16,
        line: u32,
        anchor: usize,
        items: Vec<CNode>,
    },
    Map {
        tag: u16,
        line: u32,
        anchor: usize,
        entries: Vec<(u32, CNode)>,
        pending: Option<(u32, u32)>,
    },
}

/// Parses map YAML into a compact tree. Syntax errors stop parsing. Other errors
/// such as duplicate keys, unsupported tags, compound keys, and `<<` accumulate.
pub(crate) fn parse(src: &str) -> Result<MapTree, Vec<TreeError>> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut b = Builder {
        tree: MapTree {
            atoms: Atoms::default(),
            tags: Vec::new(),
            docs: Vec::new(),
        },
        stack: Vec::new(),
        anchors: HashMap::new(),
        errors: Vec::new(),
        tag_ids: HashMap::new(),
    };
    for item in Parser::new_from_str(src) {
        let (event, span) = match item {
            Ok(x) => x,
            Err(e) => {
                b.errors.push(TreeError {
                    line: e.marker().line() as u32,
                    message: format!("YAML syntax: {}", e.info()),
                });
                return Err(b.errors);
            }
        };
        b.event(event, span.start.line() as u32);
    }
    if b.errors.is_empty() {
        Ok(b.tree)
    } else {
        Err(b.errors)
    }
}

struct Builder {
    tree: MapTree,
    stack: Vec<Frame>,
    anchors: HashMap<usize, CNode>,
    errors: Vec<TreeError>,
    tag_ids: HashMap<Box<str>, u16>,
}

impl Builder {
    fn err(&mut self, line: u32, message: String) {
        self.errors.push(TreeError { line, message });
    }

    fn tag(&mut self, tag: Option<std::borrow::Cow<'_, saphyr_parser::Tag>>, line: u32) -> u16 {
        let Some(tag) = tag else { return 0 };
        let full = format!("{}{}", tag.handle, tag.suffix);
        match full.strip_prefix(TYPE_TAG_PREFIX) {
            Some(name) if !name.is_empty() => {
                if let Some(&id) = self.tag_ids.get(name) {
                    return id;
                }
                if self.tree.tags.len() >= usize::from(u16::MAX) {
                    self.err(line, "too many distinct `!type:` tags".to_owned());
                    return 0;
                }
                self.tree.tags.push(name.into());
                let id = self.tree.tags.len() as u16;
                self.tag_ids.insert(name.into(), id);
                id
            }
            _ => {
                self.err(
                    line,
                    format!("tag `{full}` is not supported; only `!type:` tags are allowed"),
                );
                0
            }
        }
    }

    /// Key for the next node, used to decide whether to intern the value.
    fn awaiting_value_of(&self) -> Option<&str> {
        match self.stack.last() {
            Some(Frame::Map {
                pending: Some((k, _)),
                ..
            }) => Some(self.tree.atoms.text(*k)),
            _ => None,
        }
    }

    fn event(&mut self, event: Event<'_>, line: u32) {
        match event {
            Event::Scalar(text, style, anchor, tag) => {
                let tag = self.tag(tag, line);
                let quoted = style != ScalarStyle::Plain;
                let is_key = matches!(self.stack.last(), Some(Frame::Map { pending: None, .. }));
                let intern = is_key
                    || text.len() <= SHORT_SCALAR
                    || matches!(self.awaiting_value_of(), Some("type" | "proto"));
                let node = if intern {
                    CNode::Atom {
                        tag,
                        quoted,
                        line,
                        atom: self.tree.atoms.intern(&text),
                    }
                } else {
                    CNode::Text {
                        tag,
                        quoted,
                        line,
                        text: text.as_ref().into(),
                    }
                };
                self.complete(node, anchor);
            }
            Event::SequenceStart(anchor, tag) => {
                let tag = self.tag(tag, line);
                self.stack.push(Frame::Seq {
                    tag,
                    line,
                    anchor,
                    items: Vec::new(),
                });
            }
            Event::MappingStart(anchor, tag) => {
                let tag = self.tag(tag, line);
                self.stack.push(Frame::Map {
                    tag,
                    line,
                    anchor,
                    entries: Vec::new(),
                    pending: None,
                });
            }
            Event::SequenceEnd | Event::MappingEnd => match self.stack.pop() {
                Some(Frame::Seq {
                    tag,
                    line,
                    anchor,
                    items,
                }) => self.complete(
                    CNode::Seq {
                        tag,
                        line,
                        items: items.into_boxed_slice(),
                    },
                    anchor,
                ),
                Some(Frame::Map {
                    tag,
                    line,
                    anchor,
                    entries,
                    ..
                }) => {
                    self.check_keys(&entries);
                    self.complete(
                        CNode::Map {
                            tag,
                            line,
                            entries: entries.into_boxed_slice(),
                        },
                        anchor,
                    )
                }
                None => {}
            },
            Event::Alias(id) => match self.anchors.get(&id) {
                Some(node) => {
                    let node = node.clone();
                    self.complete(node, 0);
                }
                None => self.err(line, "alias refers to an unknown anchor".to_owned()),
            },
            _ => {}
        }
    }

    fn complete(&mut self, node: CNode, anchor: usize) {
        if anchor > 0 {
            self.anchors.insert(anchor, node.clone());
        }
        let line = node.line();
        let mut bad_key = false;
        match self.stack.last_mut() {
            None => self.tree.docs.push(node),
            Some(Frame::Seq { items, .. }) => items.push(node),
            Some(Frame::Map {
                entries, pending, ..
            }) => match pending.take() {
                Some((key, _)) => entries.push((key, node)),
                None => match node {
                    CNode::Atom { atom, .. } => *pending = Some((atom, line)),
                    _ => {
                        bad_key = true;
                        // Keep a placeholder so the key-value pairs remain aligned.
                        *pending = Some((u32::MAX, line));
                    }
                },
            },
        }
        if bad_key {
            self.err(line, "mapping keys must be scalars".to_owned());
        }
        // Discard the value paired with the placeholder key.
        if let Some(Frame::Map { entries, .. }) = self.stack.last_mut()
            && entries.last().is_some_and(|(k, _)| *k == u32::MAX)
        {
            entries.pop();
        }
    }

    fn check_keys(&mut self, entries: &[(u32, CNode)]) {
        let mut found = Vec::new();
        let mut seen: HashMap<u32, u32> = HashMap::new();
        for (key, value) in entries {
            let text = self.tree.atoms.text(*key);
            if text == "<<" {
                found.push((value.line(), "merge key `<<` is not supported".to_owned()));
                continue;
            }
            if let Some(first) = seen.insert(*key, value.line()) {
                found.push((
                    value.line(),
                    format!("duplicate key `{text}` (first occurrence on line {first})"),
                ));
            }
        }
        for (line, message) in found {
            self.err(line, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_is_24_bytes() {
        assert_eq!(std::mem::size_of::<CNode>(), 24);
    }

    #[test]
    fn parses_keys_tags_and_duplicates() {
        let tree = parse("a: 1\nb: !type:Foo\n  x: long text value here\nc: [1, 2]\n").unwrap();
        let root = &tree.docs[0];
        assert_eq!(root.get(&tree, "a").unwrap().as_str(&tree), Some("1"));
        let b = root.get(&tree, "b").unwrap().to_proto(&tree);
        assert_eq!(b.type_tag.as_deref(), Some("Foo"));
        assert_eq!(b.get("x").unwrap().as_str(), Some("long text value here"));
        assert_eq!(root.get(&tree, "c").unwrap().as_seq().unwrap().len(), 2);

        let errors = parse("a: 1\na: 2\n").err().unwrap();
        assert!(errors[0].message.contains("duplicate key"), "{errors:?}");
        let errors = parse("a: !foo 1\n").err().unwrap();
        assert!(errors[0].message.contains("not supported"), "{errors:?}");
    }
}
