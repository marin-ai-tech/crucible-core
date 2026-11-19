//! Turns YAML into node trees using the SS14 loader's parsing rules.
//!
//! The parser consumes `saphyr-parser` events with a custom tree builder
//! to preserve both tags and source positions. It accumulates recoverable
//! errors such as duplicate keys, merge keys, unsupported tags, and
//! nonscalar keys while continuing to read the file.
//! A syntax error stops parsing that file.

use std::collections::HashMap;
use std::fmt;

use saphyr_parser::{Event, Parser, ScalarStyle, Tag};

use crate::node::{Node, Pos, Scalar, Value};

/// Tag prefix for polymorphic values.
const TYPE_TAG_PREFIX: &str = "!type:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// YAML syntax error.
    Syntax(String),
    /// A key appears twice in a mapping. The error points to the second key.
    DuplicateKey { key: String, first: Pos },
    /// A merge key (`<<`).
    MergeKey,
    /// A tag other than `!type:Name`.
    UnsupportedTag(String),
    /// A mapping key is not a scalar.
    NonScalarKey,
    /// An alias points to an unknown anchor. This indicates an internal parser error.
    UnknownAlias,
}

/// A parse error with a one-based line and column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub pos: Pos,
    pub kind: ParseErrorKind,
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseErrorKind::Syntax(m) => write!(f, "YAML syntax error: {m}"),
            ParseErrorKind::DuplicateKey { key, first } => {
                write!(f, "duplicate key `{key}` (first occurrence at {first})")
            }
            ParseErrorKind::MergeKey => f.write_str("merge key `<<` is unsupported"),
            ParseErrorKind::UnsupportedTag(t) => {
                write!(f, "unsupported tag `{t}`; expected `!type:`")
            }
            ParseErrorKind::NonScalarKey => f.write_str("a mapping key must be a scalar"),
            ParseErrorKind::UnknownAlias => f.write_str("alias refers to an unknown anchor"),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.pos, self.kind)
    }
}

/// Parses YAML documents. Input containing only comments produces an empty list.
///
/// A leading BOM is removed.
pub fn parse_str(src: &str) -> Result<Vec<Node>, Vec<ParseError>> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut builder = Builder::default();
    for item in Parser::new_from_str(src) {
        let (event, span) = match item {
            Ok(x) => x,
            Err(e) => {
                let m = e.marker();
                builder.errors.push(ParseError {
                    pos: Pos {
                        line: m.line() as u32,
                        col: m.col() as u32 + 1,
                    },
                    kind: ParseErrorKind::Syntax(e.info().to_owned()),
                });
                return Err(builder.errors);
            }
        };
        let pos = Pos {
            line: span.start.line() as u32,
            col: span.start.col() as u32 + 1,
        };
        builder.event(event, pos);
    }
    if builder.errors.is_empty() {
        Ok(builder.docs)
    } else {
        Err(builder.errors)
    }
}

struct Frame {
    node: Node,
    anchor: usize,
    /// A key waiting for a value (mappings only).
    pending_key: Option<Node>,
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    anchors: HashMap<usize, Node>,
    docs: Vec<Node>,
    errors: Vec<ParseError>,
}

impl Builder {
    fn err(&mut self, pos: Pos, kind: ParseErrorKind) {
        self.errors.push(ParseError { pos, kind });
    }

    /// Extracts `Name` from `!type:Name`. Other tags report an error at the tag position.
    fn type_tag(&mut self, tag: Option<std::borrow::Cow<'_, Tag>>, pos: Pos) -> Option<Box<str>> {
        let tag = tag?;
        let full = format!("{}{}", tag.handle, tag.suffix);
        match full.strip_prefix(TYPE_TAG_PREFIX) {
            Some(name) if !name.is_empty() => Some(name.into()),
            _ => {
                self.err(pos, ParseErrorKind::UnsupportedTag(full));
                None
            }
        }
    }

    fn event(&mut self, event: Event<'_>, pos: Pos) {
        match event {
            Event::Scalar(text, style, anchor, tag) => {
                let type_tag = self.type_tag(tag, pos);
                let scalar = Scalar {
                    text: text.as_ref().into(),
                    plain: style == ScalarStyle::Plain,
                };
                self.complete(
                    Node {
                        type_tag,
                        pos,
                        value: Value::Scalar(scalar),
                    },
                    anchor,
                );
            }
            Event::SequenceStart(anchor, tag) => {
                let type_tag = self.type_tag(tag, pos);
                self.stack.push(Frame {
                    node: Node {
                        type_tag,
                        pos,
                        value: Value::Seq(Vec::new()),
                    },
                    anchor,
                    pending_key: None,
                });
            }
            Event::MappingStart(anchor, tag) => {
                let type_tag = self.type_tag(tag, pos);
                self.stack.push(Frame {
                    node: Node {
                        type_tag,
                        pos,
                        value: Value::Map(Vec::new()),
                    },
                    anchor,
                    pending_key: None,
                });
            }
            Event::SequenceEnd | Event::MappingEnd => {
                if let Some(frame) = self.stack.pop() {
                    if let Value::Map(entries) = &frame.node.value {
                        self.check_keys(entries);
                    }
                    self.complete(frame.node, frame.anchor);
                }
            }
            Event::Alias(id) => match self.anchors.get(&id) {
                Some(anchor) => {
                    // Copy the anchored node. Use the alias position for the root and retain nested positions.
                    let mut copy = anchor.clone();
                    copy.pos = pos;
                    self.complete(copy, 0);
                }
                None => self.err(pos, ParseErrorKind::UnknownAlias),
            },
            _ => {}
        }
    }

    fn complete(&mut self, node: Node, anchor: usize) {
        if anchor > 0 {
            self.anchors.insert(anchor, node.clone());
        }
        let Some(frame) = self.stack.last_mut() else {
            self.docs.push(node);
            return;
        };
        match &mut frame.node.value {
            Value::Seq(items) => items.push(node),
            Value::Map(entries) => match frame.pending_key.take() {
                None => frame.pending_key = Some(node),
                Some(key) => entries.push((key, node)),
            },
            Value::Scalar(_) => {}
        }
    }

    /// Keys must be unique scalars and may not be `<<`. A duplicate points to the second key.
    fn check_keys(&mut self, entries: &[(Node, Node)]) {
        let mut found = Vec::new();
        // For mappings of up to 16 entries, compare pairs to avoid allocations.
        let mut seen: HashMap<&str, Pos> = HashMap::new();
        for (i, (key, _)) in entries.iter().enumerate() {
            let Value::Scalar(s) = &key.value else {
                found.push(ParseError {
                    pos: key.pos,
                    kind: ParseErrorKind::NonScalarKey,
                });
                continue;
            };
            if s.plain && &*s.text == "<<" {
                found.push(ParseError {
                    pos: key.pos,
                    kind: ParseErrorKind::MergeKey,
                });
                continue;
            }
            let first = if entries.len() <= 16 {
                entries[..i]
                    .iter()
                    .find(|(k, _)| k.as_str() == Some(&s.text))
                    .map(|(k, _)| k.pos)
            } else {
                match seen.entry(&s.text) {
                    std::collections::hash_map::Entry::Occupied(o) => Some(*o.get()),
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert(key.pos);
                        None
                    }
                }
            };
            if let Some(first) = first {
                found.push(ParseError {
                    pos: key.pos,
                    kind: ParseErrorKind::DuplicateKey {
                        key: s.text.to_string(),
                        first,
                    },
                });
            }
        }
        self.errors.extend(found);
    }
}
