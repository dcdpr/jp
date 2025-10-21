//! TTS provider configurations.

pub mod deepgram;
pub mod owhisper;

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    providers::tts::{
        deepgram::{DeepgramConfig, PartialDeepgramConfig},
        owhisper::{OwhisperConfig, PartialOwhisperConfig},
    },
};

/// Provider configuration.
///
/// For more providers, see:
/// <https://docs.hyprnote.com/owhisper/configuration/providers>
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct TtsProviderConfig {
    /// Deepgram API configuration.
    ///
    /// see: <https://docs.rs/deepgram/latest/deepgram/>
    #[setting(nested)]
    pub deepgram: DeepgramConfig,

    /// Owhisper API configuration.
    #[setting(nested)]
    pub owhisper: OwhisperConfig,
}

impl AssignKeyValue for PartialTtsProviderConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ if kv.p("owhisper") => self.owhisper.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
