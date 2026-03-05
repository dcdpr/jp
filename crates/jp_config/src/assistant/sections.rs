//! System prompt sections.
//!
//! Sections are the building blocks of the system prompt. Each section has
//! content and optional `tag` / `title` fields that control how it is rendered:
//!
//! | `tag` | `title` | Output                           |
//! |-------|---------|----------------------------------|
//! | Some  | Some    | `<tag title="...">content</tag>` |
//! | Some  | None    | `<tag>content</tag>`             |
//! | None  | Some    | `# title\n\ncontent`             |
//! | None  | None    | `content`                        |

use std::{borrow::Cow, str::FromStr};

use schematic::Config;
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt, partial_opts},
};

/// Command configuration, either as a string or a complete configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case", serde(untagged))]
#[serde(untagged)]
pub enum SectionConfigOrString {
    /// A single string, which is interpreted as the full content of the
    /// section.
    String(String),

    /// A complete section configuration.
    #[setting(nested)]
    Config(SectionConfig),
}

impl AssignKeyValue for PartialSectionConfigOrString {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => match self {
                Self::String(_) => return missing_key(&kv),
                Self::Config(config) => config.assign(kv)?,
            },
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialSectionConfigOrString {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Config(prev), Self::Config(next)) => Self::Config(prev.delta(next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for SectionConfigOrString {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::String(v) => Self::Partial::String(v.to_owned()),
            Self::Config(v) => Self::Partial::Config(v.to_partial()),
        }
    }
}

impl FromStr for PartialSectionConfigOrString {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::String(s.to_owned()))
    }
}

/// A section of the system prompt.
///
/// Sections are rendered according to their `tag` and `title` fields.
/// See the [module-level documentation](self) for details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(default, rename_all = "snake_case")]
pub struct SectionConfig {
    /// The content of the section.
    pub content: String,

    /// Optional XML tag surrounding the section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,

    /// Optional title for the section.
    ///
    /// When a `tag` is set, the title is rendered as an XML attribute. When
    /// only a title is set (no tag), it is rendered as a markdown heading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The position of the section.
    ///
    /// A lower position will be shown first. This is useful when merging
    /// multiple sections, and you want to make sure the most important sections
    /// are shown first.
    ///
    /// Defaults to `0`.
    #[setting(default = 0)]
    pub position: i32,
}

impl SectionConfig {
    /// Add a tag to the section.
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// Add a title to the section.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Add content to the section.
    #[must_use]
    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }

    /// Render the section to its final string representation.
    ///
    /// The output format depends on the combination of `tag` and `title`. See
    /// the [module-level documentation](self) for the rendering rules.
    #[must_use]
    pub fn render(&self) -> String {
        match (&self.tag, &self.title) {
            (Some(tag), Some(title)) => {
                let content = wrap_cdata_if_needed(&self.content);
                format!(
                    "<{tag} title=\"{}\">\n{}\n</{tag}>",
                    escape_attr(title),
                    content.trim()
                )
            }
            (Some(tag), None) => {
                let content = wrap_cdata_if_needed(&self.content);
                format!("<{tag}>\n{}\n</{tag}>", content.trim())
            }
            (None, Some(title)) => {
                format!("# {title}\n\n{}", self.content)
            }
            (None, None) => self.content.trim().to_owned(),
        }
    }
}

impl AssignKeyValue for PartialSectionConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            "tag" => self.tag = kv.try_some_string()?,
            "title" => self.title = kv.try_some_string()?,
            "content" => self.content = kv.try_some_string()?,
            "position" => self.position = kv.try_some_i32()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl ToPartial for SectionConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            tag: partial_opts(self.tag.as_ref(), defaults.tag),
            title: partial_opts(self.title.as_ref(), defaults.title),
            content: partial_opt(&self.content, defaults.content),
            position: partial_opt(&self.position, defaults.position),
        }
    }
}

impl PartialConfigDelta for PartialSectionConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            tag: delta_opt(self.tag.as_ref(), next.tag),
            title: delta_opt(self.title.as_ref(), next.title),
            content: delta_opt(self.content.as_ref(), next.content),
            position: delta_opt(self.position.as_ref(), next.position),
        }
    }
}

/// Escape characters that are not allowed in XML attribute values.
fn escape_attr(s: &str) -> Cow<'_, str> {
    if s.contains(['&', '"', '<', '>']) {
        Cow::Owned(
            s.replace('&', "&amp;")
                .replace('"', "&quot;")
                .replace('<', "&lt;")
                .replace('>', "&gt;"),
        )
    } else {
        Cow::Borrowed(s)
    }
}

/// Wraps content in `<![CDATA[...]]>` if it contains characters that
/// would break the surrounding XML structure.
///
/// If the content itself contains the CDATA end marker `]]>`, it is
/// split into multiple CDATA sections.
fn wrap_cdata_if_needed(content: &str) -> Cow<'_, str> {
    if !content.contains(['<', '>', '&']) {
        return Cow::Borrowed(content);
    }

    let mut buf = String::with_capacity(content.len() + 24);
    buf.push_str("<![CDATA[\n");

    // `]]>` inside CDATA must be split: close the current CDATA section,
    // emit `]]>` literally via a new CDATA section, then continue.
    let mut rest = content;
    while let Some(pos) = rest.find("]]>") {
        buf.push_str(&rest[..pos]);
        buf.push_str("]]]]><![CDATA[>");
        rest = &rest[pos + 3..];
    }
    buf.push_str(rest);

    buf.push_str("\n]]>");
    Cow::Owned(buf)
}

impl FromStr for PartialSectionConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            content: Some(s.to_owned()),
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[path = "sections_tests.rs"]
mod tests;
