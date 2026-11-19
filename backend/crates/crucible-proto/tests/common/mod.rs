//! Integration test helpers for a neutral registry and fixture loading.
#![allow(dead_code)]

use crucible_loc::LocKeys;
use crucible_proto::{
    ComponentDef, DataRoot, Diagnostic, DiagnosticKind, FieldDef, FieldType, IgnoreCategory,
    KindDef, LoadResult, PolymorphicDef, PrototypeLoader, Registry,
};

pub fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {path}: {e}"))
}

pub fn field(name: &str, ty: FieldType) -> FieldDef {
    FieldDef::new(name, ty)
}

/// Registry with an inheritable `entity` kind, a `tag` kind, and several test components.
pub fn registry() -> Registry {
    let mut r = Registry::new();
    r.register_kind(KindDef::with_components(
        "entity",
        vec![
            field("name", FieldType::String),
            field("title", FieldType::LocKey),
            field("note", FieldType::LocKey),
        ],
    ))
    .unwrap();
    r.register_kind(KindDef::plain("tag", vec![])).unwrap();
    r.register_component(ComponentDef {
        name: "PointLight".into(),
        fields: vec![
            field("radius", FieldType::Float),
            field("color", FieldType::String),
            field("enabled", FieldType::Bool),
        ],
    })
    .unwrap();
    r.register_component(ComponentDef {
        name: "Label".into(),
        fields: vec![field("text", FieldType::String)],
    })
    .unwrap();
    r.register_component(ComponentDef {
        name: "Tagged".into(),
        fields: vec![field(
            "tags",
            FieldType::List(Box::new(FieldType::Prototype("tag".into()))),
        )],
    })
    .unwrap();
    r.register_component(ComponentDef {
        name: "Health".into(),
        fields: vec![
            field("max", FieldType::Int),
            field(
                "behaviors",
                FieldType::List(Box::new(FieldType::Polymorphic("Behavior".into()))),
            ),
        ],
    })
    .unwrap();
    r.register_component(ComponentDef {
        name: "Physics".into(),
        fields: vec![],
    })
    .unwrap();
    r.register_polymorphic(PolymorphicDef {
        base: "Behavior".into(),
        name: "DoActsBehavior".into(),
        fields: vec![field("acts", FieldType::List(Box::new(FieldType::String)))],
    })
    .unwrap();
    r.register_polymorphic(PolymorphicDef {
        base: "Behavior".into(),
        name: "SpawnEntitiesBehavior".into(),
        fields: vec![field("spawn", FieldType::Map(Box::new(FieldType::Int)))],
    })
    .unwrap();
    r.ignore_component("Sprite", IgnoreCategory::ClientOnly)
        .unwrap();
    r.ignore_component("Fixtures", IgnoreCategory::NotImplemented)
        .unwrap();
    r
}

/// Loads files from a single `core` data root.
pub fn load(registry: &Registry, files: &[(&str, &str)]) -> LoadResult {
    load_with(registry, files, None)
}

pub fn load_with(
    registry: &Registry,
    files: &[(&str, &str)],
    loc: Option<&dyn LocKeys>,
) -> LoadResult {
    let mut loader = PrototypeLoader::new(registry);
    let root = loader.add_root(DataRoot::new("core"));
    for (name, text) in files {
        loader.add_source(root, name, text);
    }
    loader.finish(loc)
}

pub fn errors(result: &LoadResult) -> Vec<&Diagnostic> {
    result.report.errors().collect()
}

pub fn has_kind(result: &LoadResult, pred: impl Fn(&DiagnosticKind) -> bool) -> bool {
    result.report.diagnostics.iter().any(|d| pred(&d.kind))
}
