use std::str::FromStr;

use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    serde::is_default,
    Error,
};

pub(super) type ChecksumPartial = <Checksum as Confique>::Partial;

/// The checksum for the MCP server binary.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Checksum {
    /// The algorithm to use for the checksum.
    #[config(
        default = "sha256",
        partial_attr(serde(default, skip_serializing_if = "is_default"))
    )]
    pub algorithm: Algorithm,

    /// The checksum value.
    pub value: String,
}

impl AssignKeyValue for <Checksum as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<(), Error> {
        let k = kv.key().as_str().to_owned();

        match k.as_str() {
            "algorithm" => self.algorithm = Some(kv.try_into_string()?.parse()?),
            "value" => self.value = Some(kv.try_into_string()?),

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    #[default]
    Sha256,
    Sha1,
}

impl FromStr for Algorithm {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "sha256" => Ok(Self::Sha256),
            "sha1" => Ok(Self::Sha1),
            _ => Err(Error::InvalidConfigValueType {
                key: s.to_string(),
                value: s.to_string(),
                need: vec!["sha256".to_string(), "sha1".to_string()],
            }),
        }
    }
}
