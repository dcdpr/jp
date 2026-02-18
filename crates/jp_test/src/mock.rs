use std::{
    env, fs,
    io::BufRead as _,
    path::{Path, PathBuf},
};

use httpmock::RecordingRuleBuilder;
pub use httpmock::{
    MockServer,
    prelude::{GET, POST},
};
use saphyr::{LoadableYamlNode, Yaml, YamlEmitter};

/// A recorder for HTTP requests.
pub struct Vcr {
    forward_to: String,
    fixtures: PathBuf,
    recording: bool,
}

pub enum Snap {
    Debug(Box<dyn std::fmt::Debug>),
    Json(serde_json::Value),
}
impl Snap {
    /// Helper to easily create a JSON variant from any serializable type.
    ///
    /// # Panics
    ///
    /// Panics if the serialization fails.
    pub fn json<T: serde::Serialize>(v: T) -> Self {
        Snap::Json(serde_json::to_value(v).unwrap())
    }

    /// Helper to create a Debug variant.
    pub fn debug<T: std::fmt::Debug + 'static>(v: T) -> Self {
        Snap::Debug(Box::new(v))
    }
}

impl Vcr {
    /// Create a new recorder.
    pub fn new(forward_to: impl Into<String>, manifest_dir: &'static str) -> Self {
        let fixtures = PathBuf::from(manifest_dir).join("tests/fixtures");

        Self {
            forward_to: forward_to.into(),
            fixtures,
            recording: env::var("RECORD").is_ok(),
        }
    }

    /// Set the fixture suffix, e.g. to add a directory to the base fixture
    /// path.
    #[must_use]
    pub fn with_fixture_suffix(mut self, suffix: &dyn AsRef<str>) -> Self {
        self.fixtures = self.fixtures.join(suffix.as_ref());
        self
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
        Fut: Future<Output = Vec<(&'static str, Snap)>>,
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

            let out = test_fn(true, server.base_url()).await;
            self.verify(name, out);

            let temp_path = recording.save_to_async(&self.fixtures, name).await?;

            if temp_path != fixture && temp_path.exists() {
                fs::rename(temp_path, &fixture)?;
            }

            modify_fixture(&fixture)?;
        } else if fixture.exists() {
            server.playback_async(&fixture).await;
            let out = test_fn(false, server.base_url()).await;
            self.verify(name, out);
        } else {
            return Err(format!("Recording not found at {}", fixture.display()).into());
        }

        Ok(())
    }

    fn verify(&self, name: &str, exprs: Vec<(&'static str, Snap)>) {
        for (suffix, snap) in exprs {
            let name = if suffix.is_empty() {
                name.to_owned()
            } else {
                format!("{name}__{suffix}")
            };

            insta::with_settings!({ snapshot_path => &self.fixtures, prepend_module_to_snapshot => false }, {
                match snap {
                    Snap::Debug(v) => insta::assert_debug_snapshot!(name, v),
                    Snap::Json(v) => insta::assert_json_snapshot!(name, v),
                }
            });
        }
    }
}

fn modify_fixture(fixture: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(fixture)?;
    let mut document = Yaml::load_from_str(&contents)?;
    for yaml in &mut document {
        if let Some(params) = yaml
            .as_mapping_get_mut("when")
            .and_then(|then| then.as_mapping_get_mut("query_param"))
            .and_then(|then| then.as_sequence_mut())
        {
            params.retain(|param| {
                param.as_mapping_get("name").and_then(|v| v.as_str()) != Some("key")
            });
        }

        // TODO: `saphyr` does not support `ScalarStyle::Folded` formatting yet.
        //
        // See: <https://github.com/saphyr-rs/saphyr/blob/001b7092d6ac21509aacacbd5810219132e8d4f7/saphyr/src/emitter.rs#L232>
        //
        // For now this is hacked below with `starts_with` on the YAML string.
        if let Some(body) = yaml
            .as_mapping_get_mut("when")
            .and_then(|then| then.as_mapping_get_mut("body"))
            .and_then(|body| body.as_cow_mut())
        {
            let s = body.to_mut();
            *s = s.to_owned();

            // let _value = body.as_str().unwrap_or_default().to_owned();
            // *body = Yaml::Representation(Cow::Owned(value), saphyr::ScalarStyle::Folded, None);
        }

        if let Some(header) = yaml
            .as_mapping_get_mut("then")
            .and_then(|then| then.as_mapping_get_mut("header"))
            .and_then(|header| header.as_sequence_mut())
        {
            header.retain(|header| {
                header.as_mapping_get("name").and_then(|v| v.as_str()) == Some("content-type")
            });
        }

        if let Some(body) = yaml
            .as_mapping_get_mut("then")
            .and_then(|then| then.as_mapping_get_mut("body"))
            .and_then(|body| body.as_cow_mut())
        {
            let s = body.to_mut();
            *s = s.to_owned();
        }
    }

    let mut buf = String::new();
    for yaml in &document {
        let mut emitter = YamlEmitter::new(&mut buf);
        // pretty-print streaming responses with multiline strings
        emitter.multiline_strings(true);
        emitter.dump(yaml)?;
        buf.push('\n');
    }

    let buf = std::io::Cursor::new(buf)
        .lines()
        .filter_map(|line| {
            let line = line.ok()?;
            // Hack to pretty-print JSON bodies in fixture files, until `saphyr`
            // supports folded scalars, and `httpmock` supports stringified JSON
            // bodies (not sure if they would want this upstreamed, it's
            // somewhat niche).
            if line.starts_with("  body: \"{") {
                // First unescape the top-level JSON.
                //
                // Be careful with removing whitespace at the end of the line,
                // because we might be dealing with server-sent events, which
                // require two newlines at the end of the event/line.
                let line = line.trim_end()[9..line.len() - 1].replace("\\\"", "\"");

                // Then unescape backslashes inside the JSON payload.
                let line = line.replace("\\\\", "\\");
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                    let mut lines = vec!["  json_body_str: >-".to_owned()];
                    let fmt = serde_json::to_string_pretty(&value).unwrap();
                    for line in fmt.lines() {
                        lines.push(format!("    {line}"));
                    }

                    return Some(lines.join("\n"));
                }
            }

            Some(line)
        })
        .collect::<Vec<_>>()
        .join("\n");

    fs::write(fixture, format!("{}\n", buf.trim_start_matches("---\n")))?;

    Ok(())
}
