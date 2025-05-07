use std::{
    fs,
    path::{Path, PathBuf},
};

use httpmock::{MockServer, RecordingRuleBuilder};
use saphyr::{LoadableYamlNode, Yaml, YamlEmitter};

/// A recorder for HTTP requests.
pub struct Vcr {
    forward_to: String,
    fixtures: PathBuf,
    recording: bool,
}

impl Vcr {
    /// Create a new recorder.
    pub fn new(forward_to: impl Into<String>, fixtures: impl Into<PathBuf>) -> Self {
        Self {
            forward_to: forward_to.into(),
            fixtures: fixtures.into(),
            recording: false,
        }
    }

    /// Enable recording for the current test.
    pub fn record(&mut self) {
        self.recording = true;
    }

    /// Enable playback for the current test.
    pub fn playback(&mut self) {
        self.recording = false;
    }

    /// Set recording mode for the current test.
    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
    }

    /// Record a test cassette.
    pub async fn cassette<R, F, Fut>(
        &self,
        name: &str,
        rule_builder: R,
        test_fn: F,
    ) -> Result<(), Box<dyn std::error::Error>>
    where
        R: FnOnce(RecordingRuleBuilder),
        F: FnOnce(bool, String) -> Fut,
        Fut: Future<Output = ()>,
    {
        let fixture = self.fixtures.join(format!("{name}.yml"));
        let server = MockServer::start_async().await;

        if self.recording {
            server
                .forward_to_async(&self.forward_to, |rule| {
                    rule.filter(|when| {
                        when.any_request();
                    });
                })
                .await;

            let recording = server.record_async(rule_builder).await;

            test_fn(true, server.base_url()).await;

            let temp_path = recording.save_to_async(&self.fixtures, name).await?;

            if temp_path != fixture && temp_path.exists() {
                fs::rename(temp_path, &fixture)?;
            }

            modify_fixture(&fixture)?;
        } else if fixture.exists() {
            server.playback_async(&fixture).await;
            test_fn(false, server.base_url()).await;
        } else {
            return Err(format!("Recording not found at {}", fixture.display()).into());
        }

        Ok(())
    }
}

fn modify_fixture(fixture: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(fixture)?;
    let mut document = Yaml::load_from_str(&contents)?;
    for yaml in &mut document {
        let Some(header) = yaml
            .as_mapping_get_mut("then")
            .and_then(|then| then.as_mapping_get_mut("header"))
            .and_then(|header| header.as_sequence_mut())
        else {
            return Ok(());
        };

        header.retain(|header| {
            header.as_mapping_get("name").and_then(|v| v.as_str()) == Some("content-type")
        });
    }

    let mut buf = String::new();
    for yaml in &document {
        YamlEmitter::new(&mut buf).dump(yaml)?;
        buf.push('\n');
    }
    fs::write(fixture, buf.trim_start_matches("---\n"))?;

    Ok(())
}
