use std::fmt;

use crate::Schema;

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum UnionOperator {
    #[default]
    AnyOf,
    OneOf,
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct UnionType {
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub default_index: Option<usize>,

    /// Index of the variant the other variants are shorthand spellings of.
    ///
    /// Set when a union describes one value written several ways, so a consumer
    /// can find the form that names the value's parts: a tool's `enable`
    /// accepts `true` or `{ state, allow_toggle }`, and the table is the
    /// expanded form of the bool.
    ///
    /// Left unset when the variants are genuinely different values rather than
    /// spellings of one.
    /// A model id is either an id or an alias resolved through a lookup, and
    /// neither expands into the other.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub expanded_index: Option<usize>,

    pub partial: bool,

    pub operator: UnionOperator,

    pub variants_types: Vec<Box<Schema>>,
}

impl UnionType {
    /// Create an "any of" union schema.
    pub fn new_any<I, V>(variants_types: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: Into<Schema>,
    {
        UnionType {
            variants_types: variants_types
                .into_iter()
                .map(|inner| Box::new(inner.into()))
                .collect(),
            ..UnionType::default()
        }
    }

    /// Create a "one of" union schema.
    pub fn new_one<I, V>(variants_types: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: Into<Schema>,
    {
        UnionType {
            operator: UnionOperator::OneOf,
            variants_types: variants_types
                .into_iter()
                .map(|inner| Box::new(inner.into()))
                .collect(),
            ..UnionType::default()
        }
    }

    /// Mark which variant the others are shorthand spellings of.
    ///
    /// See [`Self::expanded_index`].
    #[must_use]
    pub fn with_expanded_index(mut self, index: usize) -> Self {
        self.expanded_index = Some(index);
        self
    }

    /// The variant the others are shorthand spellings of, if any.
    #[must_use]
    pub fn expanded_variant(&self) -> Option<&Schema> {
        self.expanded_index
            .and_then(|index| self.variants_types.get(index))
            .map(AsRef::as_ref)
    }

    #[must_use]
    pub fn has_null(&self) -> bool {
        self.variants_types.iter().any(|schema| schema.ty.is_null())
    }

    #[doc(hidden)]
    pub fn from_schemas<I, V>(variants_types: I, default_index: Option<usize>) -> Self
    where
        I: IntoIterator<Item = V>,
        V: Into<Schema>,
    {
        UnionType {
            default_index,
            variants_types: variants_types
                .into_iter()
                .map(|inner| Box::new(inner.into()))
                .collect(),
            ..UnionType::default()
        }
    }
}

impl fmt::Display for UnionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            self.variants_types
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(" | ")
        )
    }
}
