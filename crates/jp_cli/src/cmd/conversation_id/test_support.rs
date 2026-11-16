//! Test constructors for building [`PositionalIds`] and [`FlagIds`] directly
//! without CLI parsing.

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
