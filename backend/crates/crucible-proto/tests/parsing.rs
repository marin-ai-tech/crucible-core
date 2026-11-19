//! SS14-style YAML parsing scenarios and node tree fixtures.

mod common;

use common::fixture;
use crucible_proto::{ParseErrorKind, Pos, Value, parse_str};

/// Scenario: duplicate keys.
#[test]
fn duplicate_key_reports_position_of_second_key() {
    let errors = parse_str(&fixture("duplicate_key.yml")).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].pos, Pos { line: 7, col: 5 });
    assert_eq!(
        errors[0].kind,
        ParseErrorKind::DuplicateKey {
            key: "radius".into(),
            first: Pos { line: 5, col: 5 }
        }
    );
}

/// Scenario: the field determines the value type.
#[test]
fn scalar_type_is_decided_by_field_not_by_yaml() {
    let docs = parse_str(&fixture("scalars.yml")).unwrap();
    let record = &docs[0].as_seq().unwrap()[0];
    assert_eq!(record.get("name").unwrap().as_str(), Some("1.10"));
    let component = &record.get("components").unwrap().as_seq().unwrap()[0];
    assert_eq!(component.get("text").unwrap().as_str(), Some("yes"));
}

/// Scenario: unsupported tag.
#[test]
fn unsupported_tag_reports_position_of_tag() {
    let errors = parse_str(&fixture("unsupported_tag.yml")).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].pos, Pos { line: 5, col: 12 });
    assert_eq!(
        errors[0].kind,
        ParseErrorKind::UnsupportedTag("!include".into())
    );
}

/// A merge key `<<` is an error with a source position.
#[test]
fn merge_key_is_an_error_with_position() {
    let errors = parse_str(&fixture("merge_key.yml")).unwrap_err();
    assert_eq!(errors[0].pos, Pos { line: 5, col: 5 });
    assert_eq!(errors[0].kind, ParseErrorKind::MergeKey);
}

/// An unchanged SS14-style file preserves tag positions, empty tagged nodes, anchors, and BOM handling.
#[test]
fn ss14_file_parses_with_tags_anchors_and_bom() {
    let text = format!("\u{feff}{}", fixture("ss14_style.yml"));
    let docs = parse_str(&text).unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].count_type_tags(), 3);

    let health = &docs[0].as_seq().unwrap()[1]
        .get("components")
        .unwrap()
        .as_seq()
        .unwrap()[0];
    let behaviors = health.get("behaviors").unwrap().as_seq().unwrap();
    // A tagged node points at the tag itself (line 20, column 7 in the fixture).
    assert_eq!(behaviors[0].type_tag.as_deref(), Some("DoActsBehavior"));
    assert_eq!(behaviors[0].pos, Pos { line: 20, col: 7 });
    // A tagged mapping and a tagged empty node.
    assert!(matches!(behaviors[1].value, Value::Map(_)));
    assert_eq!(behaviors[2].type_tag.as_deref(), Some("DoActsBehavior"));
    assert!(behaviors[2].is_empty());
}

/// An alias expands into a copy of the anchored node.
#[test]
fn alias_expands_to_copy_of_anchored_node() {
    let docs = parse_str("- &a { x: 1 }\n- *a\n").unwrap();
    let items = docs[0].as_seq().unwrap();
    assert_eq!(items[0].get("x"), items[1].get("x"));
    assert_eq!(items[1].get("x").unwrap().as_str(), Some("1"));
}

/// Parsing continues after a recoverable error.
#[test]
fn non_fatal_parse_errors_are_accumulated() {
    let errors = parse_str("- { a: 1, a: 2 }\n- !x 3\n- { b: 1, b: 2 }\n").unwrap_err();
    assert_eq!(errors.len(), 3);
}

/// A file containing only comments produces zero documents.
#[test]
fn comment_only_file_has_no_documents() {
    assert!(parse_str("# only a comment\n").unwrap().is_empty());
}

/// A YAML syntax error reports its line and column.
#[test]
fn syntax_error_has_position() {
    let errors = parse_str("- a: [1, 2\n- b\n").unwrap_err();
    assert!(matches!(errors[0].kind, ParseErrorKind::Syntax(_)));
    assert!(errors[0].pos.line >= 1);
}
