//! Tests for Fluent message keys.

use crucible_loc::{FluentArgs, LocError, LocKeys, Localization, MissingKey};

const FTL: &str = "\
ent-Lamp = Lamp
    .desc = A plain lamp.
greeting = Hello, { $name }!
";

fn loc() -> Localization {
    let mut l = Localization::new("en-US").unwrap();
    l.add_resource("en-US", "entities.ftl", FTL).unwrap();
    l
}

/// Scenario: a missing key is reported.
#[test]
fn missing_key_is_reported_with_owner_and_key() {
    let missing = loc().check_keys([
        ("Lamp", "ent-Lamp"),
        ("Lamp", "ent-Lamp.desc"),
        ("Lamp", "ent-Lamp.suffix"),
        ("Crate", "ent-Crate"),
    ]);
    assert_eq!(
        missing,
        [
            MissingKey {
                owner: "Lamp".into(),
                key: "ent-Lamp.suffix".into()
            },
            MissingKey {
                owner: "Crate".into(),
                key: "ent-Crate".into()
            },
        ]
    );
}

/// Key validation uses the default language, regardless of the requested language.
#[test]
fn key_check_uses_default_language() {
    let mut l = loc();
    l.add_resource("en-GB", "en-GB.ftl", "ent-Crate = Crate 📦\n")
        .unwrap();
    assert!(!l.has_key("ent-Crate"));
    assert!(l.has_key_for("en-GB", "ent-Crate"));
}

/// Formatting substitutes arguments and falls back to the default language.
#[test]
fn format_substitutes_arguments_and_falls_back_to_default_language() {
    let l = loc();
    let mut args = FluentArgs::new();
    args.set("name", "world");
    assert_eq!(
        l.format("en-US", "greeting", Some(&args)).as_deref(),
        Some("Hello, world!")
    );
    assert_eq!(
        l.format("en-GB", "ent-Lamp.desc", None).as_deref(),
        Some("A plain lamp.")
    );
    assert_eq!(l.format("en-GB", "no-such-key", None), None);
}

/// A syntax error in an `.ftl` file reports its line number.
#[test]
fn syntax_error_in_resource_reports_line() {
    let mut l = Localization::new("en-US").unwrap();
    let err = l
        .add_resource("en-US", "bad.ftl", "ok = fine\n= broken\n")
        .unwrap_err();
    match err {
        LocError::Syntax { resource, errors } => {
            assert_eq!(resource, "bad.ftl");
            assert_eq!(errors[0].0, 2);
        }
        other => panic!("expected Syntax, got {other:?}"),
    }
}

/// A duplicate message within the same language is an error.
#[test]
fn duplicate_message_in_language_is_an_error() {
    let mut l = loc();
    let err = l
        .add_resource("en-US", "more.ftl", "ent-Lamp = Again\n")
        .unwrap_err();
    assert!(matches!(err, LocError::DuplicateMessage { .. }));
}
