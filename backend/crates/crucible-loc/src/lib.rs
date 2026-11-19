//! Fluent localization and message key validation.
//!
//! Store player-facing text in `.ftl` files and reference it from prototypes by key.
//! A key is a message identifier (`ent-Lamp`) or a message attribute (`ent-Lamp.desc`).
//! Key validation uses the default language. A missing key is a load warning.

use std::collections::HashMap;
use std::fmt;

pub use fluent_bundle::FluentArgs;
use fluent_bundle::FluentResource;
use fluent_bundle::concurrent::FluentBundle;
use unic_langid::LanguageIdentifier;

/// Looks up keys in the default language.
pub trait LocKeys {
    fn has_key(&self, key: &str) -> bool;
}

#[derive(Debug, PartialEq, Eq)]
pub enum LocError {
    /// The language identifier could not be parsed (for example, `en-GB` or `en-US`).
    InvalidLanguage(String),
    /// Resource syntax errors as (one-based line number, message) pairs.
    Syntax {
        resource: String,
        errors: Vec<(usize, String)>,
    },
    /// A message with the same identifier already exists in this language.
    DuplicateMessage {
        lang: String,
        resource: String,
        id: String,
    },
}

impl fmt::Display for LocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LocError::InvalidLanguage(l) => write!(f, "invalid language identifier `{l}`"),
            LocError::Syntax { resource, errors } => {
                write!(f, "syntax errors in {resource}:")?;
                for (line, msg) in errors {
                    write!(f, " [{line}] {msg};")?;
                }
                Ok(())
            }
            LocError::DuplicateMessage { lang, resource, id } => {
                write!(
                    f,
                    "message `{id}` from {resource} is already defined in language {lang}"
                )
            }
        }
    }
}

impl std::error::Error for LocError {}

/// A key missing from the default language and the owner that references it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingKey {
    pub owner: String,
    pub key: String,
}

type Bundle = FluentBundle<FluentResource>;

/// Message bundles by language with one designated default.
pub struct Localization {
    default_lang: LanguageIdentifier,
    bundles: HashMap<LanguageIdentifier, Bundle>,
}

fn parse_lang(code: &str) -> Result<LanguageIdentifier, LocError> {
    code.parse()
        .map_err(|_| LocError::InvalidLanguage(code.to_owned()))
}

fn line_of(source: &str, offset: usize) -> usize {
    source
        .as_bytes()
        .iter()
        .take(offset)
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

impl Localization {
    pub fn new(default_lang: &str) -> Result<Self, LocError> {
        Ok(Localization {
            default_lang: parse_lang(default_lang)?,
            bundles: HashMap::new(),
        })
    }

    pub fn default_lang(&self) -> &LanguageIdentifier {
        &self.default_lang
    }

    /// Adds an `.ftl` file to a language. The `resource` filename appears in diagnostics.
    pub fn add_resource(
        &mut self,
        lang: &str,
        resource: &str,
        source: &str,
    ) -> Result<(), LocError> {
        let lang_id = parse_lang(lang)?;
        let res =
            FluentResource::try_new(source.to_owned()).map_err(|(_, errors)| LocError::Syntax {
                resource: resource.to_owned(),
                errors: errors
                    .iter()
                    .map(|e| (line_of(source, e.pos.start), e.kind.to_string()))
                    .collect(),
            })?;
        let bundle = self.bundles.entry(lang_id.clone()).or_insert_with(|| {
            let mut b = FluentBundle::new_concurrent(vec![lang_id.clone()]);
            b.set_use_isolating(false);
            b
        });
        bundle
            .add_resource(res)
            .map_err(|errors| LocError::DuplicateMessage {
                lang: lang.to_owned(),
                resource: resource.to_owned(),
                id: errors
                    .iter()
                    .map(|e| match e {
                        fluent_bundle::FluentError::Overriding { id, .. } => id.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            })
    }

    fn has_key_in(&self, lang: &LanguageIdentifier, key: &str) -> bool {
        let Some(bundle) = self.bundles.get(lang) else {
            return false;
        };
        let (id, attr) = match key.split_once('.') {
            Some((id, attr)) => (id, Some(attr)),
            None => (key, None),
        };
        match (bundle.get_message(id), attr) {
            (Some(m), Some(a)) => m.get_attribute(a).is_some(),
            (Some(m), None) => m.value().is_some(),
            (None, _) => false,
        }
    }

    /// Checks whether the given language contains the key.
    pub fn has_key_for(&self, lang: &str, key: &str) -> bool {
        parse_lang(lang).is_ok_and(|l| self.has_key_in(&l, key))
    }

    /// Formats the key in the requested or default language, in that order. Returns `None` if absent.
    pub fn format(&self, lang: &str, key: &str, args: Option<&FluentArgs<'_>>) -> Option<String> {
        let requested = parse_lang(lang).ok();
        let candidates = requested.iter().chain(std::iter::once(&self.default_lang));
        for l in candidates {
            if !self.has_key_in(l, key) {
                continue;
            }
            let bundle = self.bundles.get(l)?;
            let (id, attr) = match key.split_once('.') {
                Some((id, attr)) => (id, Some(attr)),
                None => (key, None),
            };
            let msg = bundle.get_message(id)?;
            let pattern = match attr {
                Some(a) => msg.get_attribute(a)?.value(),
                None => msg.value()?,
            };
            let mut errors = Vec::new();
            return Some(
                bundle
                    .format_pattern(pattern, args, &mut errors)
                    .into_owned(),
            );
        }
        None
    }

    /// Returns (owner, key) pairs for messages absent from the default language.
    pub fn check_keys<'a>(
        &self,
        refs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Vec<MissingKey> {
        refs.into_iter()
            .filter(|(_, key)| !self.has_key(key))
            .map(|(owner, key)| MissingKey {
                owner: owner.to_owned(),
                key: key.to_owned(),
            })
            .collect()
    }
}

impl LocKeys for Localization {
    fn has_key(&self, key: &str) -> bool {
        self.has_key_in(&self.default_lang, key)
    }
}
