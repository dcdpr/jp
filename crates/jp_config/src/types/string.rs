//! String types.

use std::{convert::Infallible, ops::Deref, str::FromStr};

use schematic::{Config, ConfigEnum, PartialConfig as _};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{Error as DeError, Visitor},
};
use serde_untagged::UntaggedEnumVisitor;

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::ToPartial,
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
    /// - `paragraph`: Blank line between the two values (default).
    /// - `line`: Single newline.
    /// - `space`: Single space.
    /// - `none`: Values are joined with nothing in between.
    ///
    /// A value merged with `space` or `none` leaves no line break around it,
    /// which is what `dedup` matches on, so such a value is only recognized as
    /// already present when it equals the whole accumulated string.
    #[setting(default)]
    pub separator: MergedStringSeparator,

    /// Whether the value is discarded when another value is merged in,
    /// regardless of the merge strategy of the other value.
    ///
    /// This is useful for "default" values that should only be used when no
    /// other value is set.
    #[setting(default)]
    pub discard_when_merged: bool,

    /// How an `append` or `prepend` recognizes a value it has already merged,
    /// and skips it.
    ///
    /// - `block`: Skip a value that appears as a whole block of the existing
    ///   string, bounded on each side by a line break or by the string's start
    ///   or end (default).
    /// - `contains`: Skip a value that appears anywhere in the existing string,
    ///   including inside a line.
    /// - `exact`: Skip a value only when it equals the whole existing string.
    /// - `off`: Merge the value however often it is supplied.
    ///
    /// `true` and `false` are accepted as shorthand for `block` and `off`.
    ///
    /// `block` and `exact` need a line break around the value to recognize it,
    /// which a value merged with `separator = "space"` or `"none"` does not
    /// have; `contains` is the mode that still recognizes those.
    /// Its trade-off is short values: a value that occurs as part of a longer
    /// sentence somewhere in the string counts as present, and is dropped.
    ///
    /// This setting is "sticky": once a config in the merge chain states one,
    /// subsequent merges for this field use it — unless a later config states
    /// a different one.
    ///
    /// `"inherit"` (or omitting the field) means "no opinion" — inherit from
    /// the previous merge, falling back to `block`.
    #[setting(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_string_dedup"
    )]
    pub dedup: Option<StringDedup>,
}

impl AssignKeyValue for PartialMergedString {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "value" => self.value = kv.try_some_string()?,
            "strategy" => self.strategy = kv.try_some_from_str()?,
            "separator" => self.separator = kv.try_some_from_str()?,
            "discard_when_merged" => self.discard_when_merged = kv.try_some_bool()?,
            // `inherit` states no opinion, which in a config file leaves an
            // earlier layer's choice standing — so it is a no-op here too.
            // Assignments mutate the accumulated partial in place, so writing
            // `None` would instead erase what a lower layer set. Returning to
            // the default takes an explicit `dedup=block`.
            "dedup" => match kv.try_some_bool_or_from_str::<StringDedupSetting, _>()? {
                Some(StringDedupSetting::Inherit) => {}
                Some(setting) => self.dedup = setting.opinion(),
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

/// How a merged string recognizes a value it has already merged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum StringDedup {
    /// Merge the value however often it is supplied.
    Off,

    /// Skip a value equal to the whole accumulated string.
    Exact,

    /// Skip a value that appears as a whole block of the accumulated string,
    /// bounded on each side by a line break or by the string's start or end.
    #[default]
    Block,

    /// Skip a value that appears anywhere in the accumulated string, including
    /// inside a line.
    Contains,
}

/// The forms a `dedup` setting can be written in.
///
/// [`StringDedupSetting::Inherit`] carries no opinion, leaving the merge to
/// take one from the previous layer.
/// Used to parse `dedup` from config files and from key-value assignments
/// (`--cfg …dedup=contains`), which accept the same set of values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StringDedupSetting {
    /// A stated mode, written by name or as one of the `true` / `false`
    /// shorthands.
    Mode(StringDedup),

    /// `"inherit"`.
    Inherit,
}

impl StringDedupSetting {
    /// The opinion this form carries, if any.
    pub(crate) const fn opinion(self) -> Option<StringDedup> {
        match self {
            Self::Mode(mode) => Some(mode),
            Self::Inherit => None,
        }
    }
}

impl From<bool> for StringDedupSetting {
    fn from(v: bool) -> Self {
        Self::Mode(if v {
            StringDedup::Block
        } else {
            StringDedup::Off
        })
    }
}

impl FromStr for StringDedupSetting {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "true" => Ok(Self::from(true)),
            "false" => Ok(Self::from(false)),
            "inherit" => Ok(Self::Inherit),
            _ => s.parse().map(Self::Mode).map_err(|_| {
                format!(
                    "expected `off`, `exact`, `block`, `contains`, `true`, `false` or `inherit`, \
                     got `{s}`"
                )
                .into()
            }),
        }
    }
}

/// Deserialize a `dedup` field from a mode name, a boolean, or `"inherit"`.
///
/// `"inherit"` and an absent field both produce `None`, which the merge
/// strategies read as "no opinion" and inherit from the previous layer.
fn deserialize_string_dedup<'de, D>(deserializer: D) -> Result<Option<StringDedup>, D::Error>
where
    D: Deserializer<'de>,
{
    struct DedupVisitor;

    impl Visitor<'_> for DedupVisitor {
        type Value = Option<StringDedup>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a dedup mode, a boolean, or \"inherit\"")
        }

        fn visit_bool<E: DeError>(self, v: bool) -> Result<Self::Value, E> {
            Ok(StringDedupSetting::from(v).opinion())
        }

        fn visit_str<E: DeError>(self, v: &str) -> Result<Self::Value, E> {
            v.parse::<StringDedupSetting>()
                .map(StringDedupSetting::opinion)
                .map_err(DeError::custom)
        }

        fn visit_none<E: DeError>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: DeError>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(DedupVisitor)
}

/// Separator placed between two merged string values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MergedStringSeparator {
    /// No separator.
    None,

    /// Single space separator.
    Space,

    /// New line separator.
    Line,

    /// Paragraph separator.
    #[default]
    Paragraph,
}

impl MergedStringSeparator {
    /// Returns the separator as a string.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::None => "",
            Self::Space => " ",
            Self::Line => "\n",
            Self::Paragraph => "\n\n",
        }
    }
}
