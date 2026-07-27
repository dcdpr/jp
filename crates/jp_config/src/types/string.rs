//! String types.

use std::{convert::Infallible, ops::Deref, str::FromStr};

use schematic::{Config, ConfigEnum, PartialConfig as _};
use serde::{Deserialize, Deserializer, Serialize};
use serde_untagged::UntaggedEnumVisitor;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::ToPartial,
    types::{Dedup, deserialize_dedup},
};

/// String value, either defaulting to a merge strategy of `replace`, or
/// defining a specific merge strategy.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(serde(untagged), no_deserialize_derive)]
pub enum MergeableString {
    /// A string that is merged using the [`schematic::merge::replace`]
    String(String),

    /// A string that is merged using the specified merge strategy.
    #[setting(nested, empty)]
    Merged(MergedString),
}

impl PartialMergeableString {
    /// Returns `true` if the `MergeableString` is the default value.
    #[must_use]
    pub fn discard_when_merged(&self) -> bool {
        matches!(self, Self::Merged(v) if v.discard_when_merged.is_some_and(|v| v))
    }
}

impl From<&str> for PartialMergeableString {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl FromStr for PartialMergeableString {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::String(s.to_owned()))
    }
}

impl From<MergeableString> for String {
    fn from(value: MergeableString) -> Self {
        match value {
            MergeableString::String(v) => v,
            MergeableString::Merged(v) => v.value,
        }
    }
}

impl AsRef<str> for PartialMergeableString {
    fn as_ref(&self) -> &str {
        match self {
            Self::String(v) => v,
            Self::Merged(v) => v.value.as_deref().unwrap_or_default(),
        }
    }
}

impl Deref for PartialMergeableString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl AssignKeyValue for PartialMergeableString {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        if kv.key_string().is_empty() {
            *self = kv.try_object_or_from_str()?;
            return Ok(());
        }

        // A nested key addresses the merge metadata, which the plain-string
        // form has nowhere to put, so promote it. The strategy is pinned to
        // `replace` because that is what a plain string means — leaving it
        // unstated would resolve to `append`.
        if let Self::String(value) = self {
            let value = std::mem::take(value);
            *self = Self::Merged(PartialMergedString {
                value: Some(value),
                strategy: Some(MergedStringStrategy::Replace),
                ..PartialMergedString::default()
            });
        }

        let Self::Merged(config) = self else {
            return missing_key(&kv);
        };

        config.assign(kv)
    }
}

impl PartialConfigDelta for PartialMergeableString {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Merged(prev), Self::Merged(next)) => Self::Merged(prev.delta(next)),
            (Self::String(prev), Self::String(next)) if prev == &next => Self::empty(),
            (_, next) => next,
        }
    }
}

impl FillDefaults for PartialMergeableString {
    /// Fill gaps from `defaults`, keeping every value this side states.
    ///
    /// A metadata-only `Merged` (`{ dedup = false }`, say) has no value of its
    /// own, so it takes the default's — without this, stating metadata alone
    /// would suppress the default value entirely.
    /// A plain string states a complete value and has no gaps to fill.
    fn fill_from(self, defaults: Self) -> Self {
        match (self, defaults) {
            (Self::Merged(v), Self::Merged(d)) => Self::Merged(v.fill_from(d)),
            (Self::Merged(v), Self::String(d)) => Self::Merged(PartialMergedString {
                value: v.value.or(Some(d)),
                ..v
            }),
            (v, _) => v,
        }
    }
}

impl ToPartial for MergeableString {
    fn to_partial(&self) -> Self::Partial {
        // Always flatten to `String` variant. The finalized value already
        // reflects all prior merges (append/prepend), so preserving the
        // `Merged` variant would cause `string_with_strategy` to re-apply
        // the strategy when the partial is merged again (e.g. in
        // `apply_conversation_config`), doubling the value.
        match self {
            Self::String(v) | Self::Merged(MergedString { value: v, .. }) => {
                Self::Partial::String(v.clone())
            }
        }
    }
}

impl<'de> Deserialize<'de> for PartialMergeableString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        UntaggedEnumVisitor::new()
            .string(|v| Ok(Self::String(v.to_owned())))
            .map(|map| map.deserialize().map(Self::Merged))
            .deserialize(deserializer)
    }
}

/// Strings that are merged using the specified merge strategy.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct MergedString {
    /// The string value.
    #[setting(default)]
    pub value: String,

    /// The merge strategy.
    ///
    /// - `append`: Append this string to the existing string (default).
    /// - `prepend`: Prepend this string to the existing string.
    /// - `replace`: Replace the existing string with this one.
    #[setting(default)]
    pub strategy: MergedStringStrategy,

    /// The separator to use between the previous value and the new value.
    ///
    /// - `none`: No separator (default).
    /// - `space`: Single space separator.
    /// - `line`: New line separator.
    /// - `paragraph`: Paragraph separator (two new lines).
    #[setting(default)]
    pub separator: MergedStringSeparator,

    /// Whether the value is discarded when another value is merged in,
    /// regardless of the merge strategy of the other value.
    ///
    /// This is useful for "default" values that should only be used when no
    /// other value is set.
    #[setting(default)]
    pub discard_when_merged: bool,

    /// Whether to skip an `append` or `prepend` whose value is already present.
    ///
    /// Defaults to `true`.
    /// Set to `false` to append the value unconditionally.
    /// Accepts `true`, `false`, or `"inherit"`.
    ///
    /// A value counts as present when it appears in the existing string as a
    /// whole `separator`-delimited block.
    /// Partial matches inside a block do not count.
    /// With `separator = "none"` there are no block boundaries to match
    /// against, so only an exact match of the whole string counts.
    ///
    /// This flag is "sticky": once a config in the merge chain sets it
    /// explicitly, subsequent merges for this field use that value — unless a
    /// later config states a different one.
    ///
    /// `"inherit"` (or omitting the field) means "no opinion" — inherit from
    /// the previous merge, falling back to `true`.
    #[setting(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_dedup"
    )]
    pub dedup: Option<bool>,
}

impl AssignKeyValue for PartialMergedString {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "value" => self.value = kv.try_some_string()?,
            "strategy" => self.strategy = kv.try_some_from_str()?,
            "separator" => self.separator = kv.try_some_from_str()?,
            "discard_when_merged" => self.discard_when_merged = kv.try_some_bool()?,
            // Tri-state. `inherit` states no opinion, which in a config file
            // leaves an earlier layer's choice standing — so it is a no-op here
            // too. Assignments mutate the accumulated partial in place, so
            // writing `None` would instead erase what a lower layer set.
            // Returning to the default takes an explicit `dedup=true`.
            "dedup" => match kv.try_some_bool_or_from_str::<Dedup, _>()? {
                Some(Dedup::Inherit) => {}
                Some(opinion) => self.dedup = opinion.opinion(),
                None => self.dedup = None,
            },
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl ToPartial for MergedString {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            value: Some(self.value.clone()),
            strategy: Some(self.strategy),
            separator: Some(self.separator),
            discard_when_merged: Some(self.discard_when_merged),
            dedup: self.dedup,
        }
    }
}

impl FillDefaults for PartialMergedString {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            value: self.value.or(defaults.value),
            strategy: self.strategy.or(defaults.strategy),
            separator: self.separator.or(defaults.separator),
            discard_when_merged: self.discard_when_merged.or(defaults.discard_when_merged),
            dedup: self.dedup.or(defaults.dedup),
        }
    }
}

impl PartialConfigDelta for PartialMergedString {
    fn delta(&self, next: Self) -> Self {
        Self {
            value: delta_opt(self.value.as_ref(), next.value),
            strategy: delta_opt(self.strategy.as_ref(), next.strategy),
            separator: delta_opt(self.separator.as_ref(), next.separator),
            discard_when_merged: delta_opt(
                self.discard_when_merged.as_ref(),
                next.discard_when_merged,
            ),
            dedup: delta_opt(self.dedup.as_ref(), next.dedup),
        }
    }
}

/// Merge strategy for `MergeableString`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MergedStringStrategy {
    /// Append this string to the existing string, using the `separator` value.
    #[default]
    Append,

    /// Prepend this string to the existing string, using the `separator` value.
    Prepend,

    /// Replace the existing string with this one.
    ///
    /// See [`schematic::merge::replace`].
    Replace,
}

/// Merge strategy for `VecWithStrategy`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MergedStringSeparator {
    /// No separator.
    #[default]
    None,

    /// Single space separator.
    Space,

    /// New line separator.
    Line,

    /// Paragraph separator.
    Paragraph,
}

impl MergedStringSeparator {
    /// Returns the separator as a string.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::None => "",
            Self::Space => " ",
            Self::Line => "\n",
            Self::Paragraph => "\n\n",
        }
    }
}
