//! Constructors that build the conversation ID argument types without parsing a
//! command line, for the tests of the commands that hold them.

use super::{FlagIds, PositionalIds};
use crate::cmd::target::ConversationTarget;

impl<const SESSION: bool, const MULTI: bool> PositionalIds<SESSION, MULTI> {
    pub fn from_targets(ids: Vec<ConversationTarget>) -> Self {
        Self { ids }
    }
}

impl<const SESSION: bool, const MULTI: bool, const GLOBAL: bool> FlagIds<SESSION, MULTI, GLOBAL> {
    pub fn from_targets(ids: Vec<ConversationTarget>) -> Self {
        Self { ids }
    }
}
