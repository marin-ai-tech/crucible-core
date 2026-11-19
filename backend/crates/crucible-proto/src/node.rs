//! Node trees produced by the YAML parser.
//!
//! The tree retains `!type:` tags, one-based source lines and columns,
//! and mapping key order. It stores the original scalar text because the
//! destination field determines a value's type.
//! For example, `1.10` and `yes` retain their original spelling.

/// One-based line and column in a source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

impl std::fmt::Display for Pos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// A scalar's original text and whether it uses plain style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scalar {
    pub text: Box<str>,
    /// `false` for quoted or block scalars.
    pub plain: bool,
}

/// Contents of a node.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Scalar(Scalar),
    Seq(Vec<Node>),
    /// Mapping entries in document order with scalar keys.
    Map(Vec<(Node, Node)>),
}

/// A node in a YAML tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Name from a `!type:Name` tag, without the prefix. Other tags are unsupported.
    pub type_tag: Option<Box<str>>,
    /// Source position of the node, pointing to the tag if one is present.
    pub pos: Pos,
    pub value: Value,
}

impl Node {
    pub fn new(pos: Pos, value: Value) -> Self {
        Node {
            type_tag: None,
            pos,
            value,
        }
    }

    /// An untagged plain scalar for tests and manually constructed trees.
    pub fn scalar(pos: Pos, text: &str) -> Self {
        Node::new(
            pos,
            Value::Scalar(Scalar {
                text: text.into(),
                plain: true,
            }),
        )
    }

    /// The scalar's original text.
    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::Scalar(s) => Some(&s.text),
            _ => None,
        }
    }

    pub fn as_seq(&self) -> Option<&[Node]> {
        match &self.value {
            Value::Seq(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[(Node, Node)]> {
        match &self.value {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Returns the value for a scalar key. Prototype mappings are small, so lookup is linear.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.as_map()?
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .map(|(_, v)| v)
    }

    /// An empty node is a plain scalar with no text, as in `key:` with no value.
    pub fn is_empty(&self) -> bool {
        matches!(&self.value, Value::Scalar(s) if s.plain && s.text.is_empty())
    }

    /// An empty node, `null`, or `~`.
    pub fn is_null(&self) -> bool {
        matches!(&self.value, Value::Scalar(s)
            if s.plain && matches!(&*s.text, "" | "~" | "null" | "Null" | "NULL"))
    }

    /// Counts the nodes in the subtree, including keys.
    pub fn count_nodes(&self) -> usize {
        1 + match &self.value {
            Value::Scalar(_) => 0,
            Value::Seq(v) => v.iter().map(Node::count_nodes).sum(),
            Value::Map(m) => m
                .iter()
                .map(|(k, v)| k.count_nodes() + v.count_nodes())
                .sum(),
        }
    }

    /// Counts `!type:`-tagged nodes in the subtree, including keys.
    pub fn count_type_tags(&self) -> usize {
        usize::from(self.type_tag.is_some())
            + match &self.value {
                Value::Scalar(_) => 0,
                Value::Seq(v) => v.iter().map(Node::count_type_tags).sum(),
                Value::Map(m) => m
                    .iter()
                    .map(|(k, v)| k.count_type_tags() + v.count_type_tags())
                    .sum(),
            }
    }
}

/// Interprets a scalar as a case-insensitive `true` or `false`, following SS14's loader rules.
pub fn parse_bool(text: &str) -> Option<bool> {
    if text.eq_ignore_ascii_case("true") {
        Some(true)
    } else if text.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}
