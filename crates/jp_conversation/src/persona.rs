use std::{fmt, path::PathBuf, str::FromStr};

use jp_id::{
    parts::{GlobalId, TargetId, Variant},
    Id,
};
use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    model::Parameters,
    ModelId,
};

/// Configuration specifying how the LLM should behave.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Persona {
    /// A unique identifier for the persona.
    pub name: String,

    /// The system prompt to use for the persona.
    pub system_prompt: String,

    /// A list of instructions for the persona.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<Instructions>,

    /// The model ID to use for the persona.
    ///
    /// If not set, either the default configured model is used, or one has to
    /// be specified on a per-conversation basis by the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,

    /// Whether to inherit the model parameters from the global config.
    #[serde(default = "inherit_parameters_default")]
    pub inherit_parameters: bool,

    /// A list of model parameters to set.
    #[serde(default)]
    pub parameters: Parameters,
}

fn inherit_parameters_default() -> bool {
    true
}

impl Persona {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }
}

impl Default for Persona {
    fn default() -> Self {
        Self {
            name: "Default".to_string(),
            system_prompt: "You are a helpful assistant.".to_string(),
            instructions: Vec::new(),
            model: None,
            inherit_parameters: true,
            parameters: Parameters::default(),
        }
    }
}

/// A list of instructions for a persona.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Instructions {
    /// The title of the instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// An optional description of the instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The list of instructions.
    #[serde(default)]
    pub items: Vec<String>,

    /// A list of examples to go with the instructions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

impl Instructions {
    pub fn try_to_xml(&self) -> Result<String> {
        let mut buffer = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
        serializer.indent(' ', 2);
        self.serialize(serializer)?;
        Ok(buffer)
    }

    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub fn with_item(mut self, item: impl Into<String>) -> Self {
        self.items.push(item.into());
        self
    }
}

/// ID wrapper for Persona
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PersonaId(String);

impl PersonaId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn to_path_buf(&self) -> PathBuf {
        format!("{}.json", self.target_id()).into()
    }

    pub fn from_filename(filename: &str) -> Result<Self> {
        filename
            .strip_suffix(".json")
            .ok_or_else(|| Error::InvalidIdFormat(format!("Invalid persona filename: {filename}")))
            .and_then(Self::try_from)
    }
}

impl Default for PersonaId {
    fn default() -> Self {
        Self("default".to_owned())
    }
}

impl Id for PersonaId {
    fn variant() -> Variant {
        'p'.into()
    }

    fn target_id(&self) -> TargetId {
        self.0.clone().into()
    }

    fn global_id(&self) -> GlobalId {
        jp_id::global::get().into()
    }

    fn is_valid(&self) -> bool {
        Self::variant().is_valid() && self.global_id().is_valid()
    }
}

impl fmt::Display for PersonaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<&str> for PersonaId {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self> {
        Self::try_from(s.to_owned())
    }
}

impl TryFrom<&String> for PersonaId {
    type Error = Error;

    fn try_from(s: &String) -> Result<Self> {
        Self::try_from(s.as_str())
    }
}

impl TryFrom<String> for PersonaId {
    type Error = Error;

    fn try_from(s: String) -> Result<Self> {
        if s.chars().any(|c| {
            !(c.is_numeric()
                || (c.is_ascii_alphabetic() && c.is_ascii_lowercase())
                || c == '-'
                || c == '_')
        }) {
            return Err(Error::InvalidIdFormat(
                "Persona ID must be [a-z0-9_-]".to_string(),
            ));
        }

        Ok(Self(s))
    }
}

impl FromStr for PersonaId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        jp_id::parse::<Self>(s)
            .map(|p| Self(p.target_id.to_string()))
            .map_err(Into::into)
    }
}
