//! Scenarios for loading, identifiers and prefixes, inheritance,
//! ignored components, and validation with a complete report.

mod common;

use common::{errors, fixture, load, load_with, registry};
use crucible_loc::Localization;
use crucible_proto::{DataRoot, DiagnosticKind, InstantiateError, Pos, PrototypeLoader, Severity};

/// Scenario: an unchanged SS14-style file.
#[test]
fn ss14_file_loads_without_transformation() {
    let reg = registry();
    let result = load(&reg, &[("ss14_style.yml", &fixture("ss14_style.yml"))]);
    assert!(errors(&result).is_empty(), "{}", result.report);

    let crate_proto = result.set.get("entity", "Crate").unwrap();
    assert_eq!(crate_proto.parents, ["BaseThing", "BaseBreakable"]);
    // Includes components from both parents and the child. The client-only Sprite is discarded.
    let names: Vec<&str> = crate_proto.components.iter().map(|c| &*c.name).collect();
    assert_eq!(names, ["PointLight", "Health"]);
    // Polymorphic tags remain in component data for entity creation.
    let health = crate_proto.component("Health").unwrap();
    let behaviors = health.data.get("behaviors").unwrap().as_seq().unwrap();
    let tags: Vec<_> = behaviors
        .iter()
        .map(|b| b.type_tag.as_deref().unwrap())
        .collect();
    assert_eq!(
        tags,
        ["DoActsBehavior", "SpawnEntitiesBehavior", "DoActsBehavior"]
    );
}

/// Scenario: unknown prototype kind.
#[test]
fn prototype_of_unknown_kind_is_reported_with_file_line_and_kind() {
    let reg = registry();
    let result = load(&reg, &[("unknown_kind.yml", &fixture("unknown_kind.yml"))]);
    let errs = errors(&result);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].file.as_deref(), Some("unknown_kind.yml"));
    assert_eq!(errs[0].pos, Some(Pos { line: 4, col: 9 }));
    assert_eq!(
        errs[0].kind,
        DiagnosticKind::UnknownPrototypeKind {
            kind: "widget".into()
        }
    );
}

/// A duplicate key in a loaded file reports the filename and position of the second key.
#[test]
fn duplicate_key_is_reported_by_loader_with_file() {
    let reg = registry();
    let result = load(
        &reg,
        &[("duplicate_key.yml", &fixture("duplicate_key.yml"))],
    );
    let errs = errors(&result);
    assert_eq!(errs[0].file.as_deref(), Some("duplicate_key.yml"));
    assert_eq!(errs[0].pos, Some(Pos { line: 7, col: 5 }));
}

/// Scenario: duplicate identifier without an override.
#[test]
fn implicit_duplicate_across_roots_reports_conflict_with_both_files() {
    let reg = registry();
    let mut loader = PrototypeLoader::new(&reg);
    let core = loader.add_root(DataRoot::new("core"));
    let ext = loader.add_root(DataRoot::new("ext"));
    loader.add_source(core, "core/tags.yml", "- type: tag\n  id: Metal\n");
    loader.add_source(ext, "ext/tags.yml", "- type: tag\n  id: Metal\n");
    let result = loader.finish(None);
    let errs = errors(&result);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].file.as_deref(), Some("ext/tags.yml"));
    assert_eq!(errs[0].pos, Some(Pos { line: 2, col: 7 }));
    match &errs[0].kind {
        DiagnosticKind::DuplicateId {
            first_file,
            first_pos,
        } => {
            assert_eq!(&**first_file, "core/tags.yml");
            assert_eq!(*first_pos, Pos { line: 1, col: 3 });
        }
        other => panic!("expected DuplicateId, got {other:?}"),
    }
}

/// A duplicate within a data root cannot be an override.
#[test]
fn duplicate_within_one_root_is_an_error_even_with_override() {
    let reg = registry();
    let mut loader = PrototypeLoader::new(&reg);
    let root = loader.add_root(DataRoot::new("core").with_override("tag", "Metal"));
    loader.add_source(root, "a.yml", "- type: tag\n  id: Metal\n");
    loader.add_source(root, "b.yml", "- type: tag\n  id: Metal\n");
    assert_eq!(errors(&loader.finish(None)).len(), 1);
}

/// An explicit override from a later data root is accepted.
#[test]
fn explicit_override_replaces_earlier_prototype() {
    let reg = registry();
    let mut loader = PrototypeLoader::new(&reg);
    let core = loader.add_root(DataRoot::new("core"));
    let ext = loader.add_root(DataRoot::new("ext").with_override("entity", "Lamp"));
    loader.add_source(
        core,
        "core/lamp.yml",
        "- type: entity\n  id: Lamp\n  name: old\n",
    );
    loader.add_source(
        ext,
        "ext/lamp.yml",
        "- type: entity\n  id: Lamp\n  name: new\n",
    );
    let result = loader.finish(None);
    assert!(errors(&result).is_empty(), "{}", result.report);
    let lamp = result.set.get("entity", "Lamp").unwrap();
    assert_eq!(lamp.field("name").unwrap().as_str(), Some("new"));
    assert_eq!(&*lamp.file, "ext/lamp.yml");
}

/// Scenario: prototype without the extension's prefix.
#[test]
fn prototype_without_extension_prefix_gives_warning() {
    let reg = registry();
    let mut loader = PrototypeLoader::new(&reg);
    let ext = loader.add_root(DataRoot::new("ext").with_id_prefix("Ex"));
    loader.add_source(ext, "ext/prefixed.yml", &fixture("prefixed.yml"));
    let result = loader.finish(None);
    assert!(errors(&result).is_empty());
    let warnings: Vec<_> = result.report.warnings().collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].file.as_deref(), Some("ext/prefixed.yml"));
    assert_eq!(warnings[0].pos, Some(Pos { line: 2, col: 7 }));
    assert_eq!(warnings[0].proto.as_ref().unwrap().id.as_ref(), "Torch");
    assert_eq!(
        warnings[0].kind,
        DiagnosticKind::MissingIdPrefix {
            prefix: "Ex".into()
        }
    );
    // The prefixed prototype produces no warning. Both prototypes load.
    assert!(result.set.get("entity", "Torch").is_some());
}

/// Scenario: overriding a component field.
#[test]
fn child_component_field_overrides_parent_field() {
    let reg = registry();
    let yaml = "\
- type: entity
  id: Parent
  abstract: true
  components:
  - type: PointLight
    radius: 5
    color: white
- type: entity
  id: Child
  parent: Parent
  components:
  - type: PointLight
    radius: 8
";
    let result = load(&reg, &[("a.yml", yaml)]);
    assert!(errors(&result).is_empty(), "{}", result.report);
    let light = result
        .set
        .get("entity", "Child")
        .unwrap()
        .component("PointLight")
        .unwrap();
    assert_eq!(light.data.get("radius").unwrap().as_str(), Some("8"));
    assert_eq!(light.data.get("color").unwrap().as_str(), Some("white"));
}

/// With multiple parents, a later parent overrides an earlier one.
#[test]
fn later_parent_overrides_earlier_parent() {
    let reg = registry();
    let yaml = "\
- type: entity
  id: First
  abstract: true
  components:
  - type: PointLight
    radius: 1
    color: red
- type: entity
  id: Second
  abstract: true
  components:
  - type: PointLight
    radius: 2
- type: entity
  id: Child
  parent: [ First, Second ]
";
    let result = load(&reg, &[("a.yml", yaml)]);
    let light = result
        .set
        .get("entity", "Child")
        .unwrap()
        .component("PointLight")
        .unwrap();
    assert_eq!(light.data.get("radius").unwrap().as_str(), Some("2"));
    assert_eq!(light.data.get("color").unwrap().as_str(), Some("red"));
}

/// Scenario: inheritance cycle.
#[test]
fn inheritance_cycle_is_reported_with_the_cycle_members() {
    let reg = registry();
    let result = load(&reg, &[("cycle.yml", &fixture("cycle.yml"))]);
    let errs = errors(&result);
    assert_eq!(errs.len(), 1, "{}", result.report);
    assert_eq!(errs[0].file.as_deref(), Some("cycle.yml"));
    match &errs[0].kind {
        DiagnosticKind::InheritanceCycle { cycle } => assert_eq!(cycle, &["A", "B", "A"]),
        other => panic!("expected InheritanceCycle, got {other:?}"),
    }
    assert!(result.set.get("entity", "A").is_none());
}

/// A missing parent produces a positioned error and neither panics nor goes unnoticed.
#[test]
fn missing_parent_is_reported() {
    let reg = registry();
    let result = load(
        &reg,
        &[("a.yml", "- type: entity\n  id: Child\n  parent: Nowhere\n")],
    );
    let errs = errors(&result);
    assert_eq!(
        errs[0].kind,
        DiagnosticKind::UnknownParent {
            parent: "Nowhere".into()
        }
    );
    assert_eq!(errs[0].pos, Some(Pos { line: 3, col: 11 }));
}

/// Scenario: client-only component.
#[test]
fn client_only_component_is_dropped_without_error_or_warning() {
    let reg = registry();
    let result = load(
        &reg,
        &[(
            "a.yml",
            "- type: entity\n  id: Lamp\n  components:\n  - type: Sprite\n    sprite: a.rsi\n  - type: Physics\n",
        )],
    );
    assert!(result.report.diagnostics.is_empty(), "{}", result.report);
    let lamp = result.set.get("entity", "Lamp").unwrap();
    assert!(lamp.component("Sprite").is_none());
    assert!(lamp.component("Physics").is_some());
}

/// An unimplemented component produces a single aggregate warning.
#[test]
fn not_implemented_components_give_one_summary_warning() {
    let reg = registry();
    let result = load(&reg, &[("a.yml", &fixture("ignored_components.yml"))]);
    assert!(errors(&result).is_empty(), "{}", result.report);
    let warnings: Vec<_> = result.report.warnings().collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].kind,
        DiagnosticKind::NotImplementedComponents {
            components: vec!["Fixtures".into()],
            prototypes: 2
        }
    );
    assert!(
        result
            .set
            .get("entity", "Lamp")
            .unwrap()
            .component("Fixtures")
            .is_none()
    );
}

/// Scenario: unrecognized component.
#[test]
fn unknown_component_is_reported_with_file_line_prototype_and_name() {
    let reg = registry();
    let result = load(
        &reg,
        &[("unknown_component.yml", &fixture("unknown_component.yml"))],
    );
    let errs = errors(&result);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].file.as_deref(), Some("unknown_component.yml"));
    assert_eq!(errs[0].pos, Some(Pos { line: 6, col: 11 }));
    assert_eq!(errs[0].proto.as_ref().unwrap().id.as_ref(), "Lamp");
    assert_eq!(
        errs[0].kind,
        DiagnosticKind::UnknownComponent {
            component: "Mystery".into()
        }
    );
}

/// Scenario: multiple errors across files.
#[test]
fn several_errors_in_different_files_are_all_reported() {
    let reg = registry();
    let result = load(
        &reg,
        &[
            ("errors_a.yml", &fixture("errors_a.yml")),
            ("errors_b.yml", &fixture("errors_b.yml")),
            ("tags.yml", "- type: tag\n  id: Metal\n"),
        ],
    );
    let errs = errors(&result);
    assert_eq!(errs.len(), 2, "{}", result.report);
    assert_eq!(errs[0].file.as_deref(), Some("errors_a.yml"));
    assert_eq!(errs[0].pos, Some(Pos { line: 5, col: 5 }));
    assert!(matches!(&errs[0].kind, DiagnosticKind::UnknownField { field, .. } if field == "glow"));
    assert_eq!(errs[1].file.as_deref(), Some("errors_b.yml"));
    assert_eq!(errs[1].pos, Some(Pos { line: 5, col: 13 }));
    assert_eq!(errs[1].proto.as_ref().unwrap().id.as_ref(), "Shelf");
    assert!(
        matches!(&errs[1].kind, DiagnosticKind::UnknownReference { id, .. } if id == "Missing")
    );
    // The server cannot open the lobby: `into_result` returns a report without a prototype set.
    assert!(result.into_result().is_err());
}

/// Reports multiple errors in field types and polymorphic values.
#[test]
fn value_types_and_polymorphic_values_are_validated() {
    let reg = registry();
    let yaml = "\
- type: entity
  id: Bad
  components:
  - type: PointLight
    radius: wide
    enabled: maybe
  - type: Health
    max: 1.5
    behaviors:
    - !type:Unknown
    - {}
    - !type:DoActsBehavior
      acts: [ a, b ]
";
    let result = load(&reg, &[("bad.yml", yaml)]);
    let errs = errors(&result);
    assert_eq!(errs.len(), 5, "{}", result.report);
    assert!(matches!(errs[0].kind, DiagnosticKind::TypeMismatch { .. }));
    assert!(matches!(errs[1].kind, DiagnosticKind::TypeMismatch { .. }));
    assert!(matches!(errs[2].kind, DiagnosticKind::TypeMismatch { .. }));
    assert!(matches!(
        errs[3].kind,
        DiagnosticKind::UnknownPolymorphicType { .. }
    ));
    assert!(matches!(
        errs[4].kind,
        DiagnosticKind::MissingPolymorphicTag { .. }
    ));
}

/// String fields retain the source text in the resolved prototype.
#[test]
fn string_fields_keep_source_text_in_resolved_prototype() {
    let reg = registry();
    let result = load(&reg, &[("scalars.yml", &fixture("scalars.yml"))]);
    assert!(errors(&result).is_empty(), "{}", result.report);
    let lamp = result.set.get("entity", "Lamp").unwrap();
    assert_eq!(lamp.field("name").unwrap().as_str(), Some("1.10"));
    assert_eq!(
        lamp.component("Label")
            .unwrap()
            .data
            .get("text")
            .unwrap()
            .as_str(),
        Some("yes")
    );
}

/// Scenario: missing localization key.
#[test]
fn missing_loc_key_gives_warning_with_prototype_and_key() {
    let reg = registry();
    let mut loc = Localization::new("en-US").unwrap();
    loc.add_resource("en-US", "entities.ftl", "ent-Lamp = Lamp\n")
        .unwrap();
    let result = load_with(
        &reg,
        &[("localized.yml", &fixture("localized.yml"))],
        Some(&loc),
    );
    assert!(errors(&result).is_empty());
    let warnings: Vec<_> = result.report.warnings().collect();
    assert_eq!(warnings.len(), 1, "{}", result.report);
    assert_eq!(warnings[0].severity(), Severity::Warning);
    assert_eq!(warnings[0].file.as_deref(), Some("localized.yml"));
    assert_eq!(warnings[0].pos, Some(Pos { line: 4, col: 9 }));
    assert_eq!(warnings[0].proto.as_ref().unwrap().id.as_ref(), "Lamp");
    assert_eq!(
        warnings[0].kind,
        DiagnosticKind::MissingLocKey {
            key: "ent-Lamp.desc".into()
        }
    );
}

/// Abstract and missing prototypes cannot be instantiated.
#[test]
fn abstract_and_missing_prototypes_are_not_instantiable() {
    let reg = registry();
    let result = load(&reg, &[("ss14_style.yml", &fixture("ss14_style.yml"))]);
    assert!(matches!(
        result.set.instantiable("entity", "BaseThing"),
        Err(InstantiateError::Abstract { .. })
    ));
    assert!(matches!(
        result.set.instantiable("entity", "Nope"),
        Err(InstantiateError::NotFound { .. })
    ));
    assert!(result.set.instantiable("entity", "Crate").is_ok());
}
