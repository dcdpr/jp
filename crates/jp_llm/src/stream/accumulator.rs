use jp_conversation::message::ToolCallRequest;

use crate::{stream::delta::Delta, CompletionChunk, Error, StreamEvent};

#[derive(Debug, Default)]
pub struct Accumulator {
    content: TextAccumulator,
    reasoning: TextAccumulator,
    tool_call: ToolCallAccumulator,
}

impl Accumulator {
    pub fn new(max_line_length: usize) -> Self {
        Self {
            content: TextAccumulator::new(max_line_length),
            reasoning: TextAccumulator::new(max_line_length),
            tool_call: ToolCallAccumulator::default(),
        }
    }

    pub fn is_accumulating_function_call(&self) -> bool {
        self.tool_call.is_accumulating()
    }

    pub fn delta_step(&mut self, delta: &Delta) -> Result<Vec<StreamEvent>, Error> {
        let mut events = Vec::new();

        if let Some(text) = delta
            .reasoning
            .as_deref()
            .and_then(|text| self.reasoning.delta_step(text))
        {
            events.push(StreamEvent::ChatChunk(CompletionChunk::Reasoning(text)));
        }

        if let Some(text) = delta
            .content
            .as_deref()
            .and_then(|text| self.content.delta_step(text))
        {
            events.push(StreamEvent::ChatChunk(CompletionChunk::Content(text)));
        }

        if let Some(call) = self.tool_call.delta_step(
            delta.tool_call_id.as_deref(),
            delta.tool_call_name.as_deref(),
            delta.tool_call_arguments.as_deref(),
            delta.tool_call_finished,
        )? {
            events.push(StreamEvent::ToolCall(call));
        }

        Ok(events)
    }

    /// Mark the accumulator as finished, so that the next `delta_step` call
    /// will drain all buffers and close any open accumulators.
    pub fn finalize(&mut self) {
        self.tool_call.finalize();
        self.reasoning.finalize();
        self.content.finalize();
    }
}

// State for accumulating function calls.
#[derive(Default, Debug)]
pub enum ToolCallAccumulator {
    #[default]
    Idle,
    Accumulating {
        id: String,
        name: String,
        arguments_buffer: String,
        finished: bool,
    },
}

impl ToolCallAccumulator {
    pub fn is_accumulating(&self) -> bool {
        matches!(self, Self::Accumulating { .. })
    }

    fn delta_step(
        &mut self,
        id: Option<&str>,
        name: Option<&str>,
        arguments: Option<&str>,
        force_finished: bool,
    ) -> Result<Option<ToolCallRequest>, Error> {
        match self {
            Self::Idle => match name {
                Some(name) => {
                    *self = ToolCallAccumulator::Accumulating {
                        id: id.map(str::to_owned).unwrap_or_default(),
                        name: name.to_owned(),
                        arguments_buffer: arguments.map(str::to_owned).unwrap_or_default(),
                        finished: false,
                    };

                    Ok(None)
                }
                None if arguments.is_some() => Err(Error::InvalidResponse(
                    "Received function call arguments without a function name.".into(),
                )),
                _ => Ok(None),
            },
            Self::Accumulating {
                id,
                name,
                arguments_buffer,
                finished,
            } => {
                if let Some(args_chunk) = arguments {
                    arguments_buffer.push_str(args_chunk);
                }

                if !*finished && !force_finished {
                    return Ok(None);
                }

                let id = id.clone();
                let name = name.clone();
                let arguments = if arguments_buffer.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    match serde_json::from_str(arguments_buffer) {
                        Ok(arguments) => arguments,
                        Err(e) => {
                            return Err(Error::InvalidResponse(format!(
                                "Failed to parse function call arguments: {e}. Buffer was: \
                                 '{arguments_buffer}'"
                            )));
                        }
                    }
                };

                *self = Self::default();
                Ok(Some(ToolCallRequest {
                    id,
                    name,
                    arguments,
                }))
            }
        }
    }

    fn finalize(&mut self) {
        match self {
            ToolCallAccumulator::Idle => {}
            ToolCallAccumulator::Accumulating { finished, .. } => *finished = true,
        }
    }
}

// State for accumulating content.
#[derive(Debug, Default)]
pub struct TextAccumulator {
    buffer: String,

    /// The text accumulator tries to accumulate per line, but if a line is
    /// longer than `line_length`, it tries to find the nearest sentence
    /// terminator and accumulates until that point.
    max_line_length: Option<usize>,

    /// If set to `true`, the next `delta_step` call will drain the entire
    /// buffer.
    drain: bool,
}

impl TextAccumulator {
    pub fn new(max_line_length: usize) -> Self {
        Self {
            buffer: String::new(),
            max_line_length: Some(max_line_length),
            drain: false,
        }
    }

    pub fn delta_step(&mut self, text: &str) -> Option<String> {
        self.buffer.push_str(text);

        if self.drain && !self.buffer.is_empty() {
            return Some(self.buffer.drain(..).collect());
        }

        let max = self.max_line_length.unwrap_or(usize::MAX);
        if self.buffer.len() < max {
            return None;
        }

        let pos = self
            .buffer
            .chars()
            .take(max)
            .collect::<String>()
            .rfind(['\n', '.', '!', '?', ',', ';', ':'])
            .unwrap_or(self.buffer.len() - 1);

        Some(self.buffer.drain(..=pos).collect())
    }

    /// Mark the accumulator as finished, so that the next `delta_step` call
    /// will drain the entire buffer.
    pub fn finalize(&mut self) {
        self.drain = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[rustfmt::skip]
    fn test_text_accumulator() {
        let mut accumulator = TextAccumulator {
            max_line_length: Some(20),
            ..Default::default()
        };

        assert_eq!(accumulator.delta_step("Hello"), None);
        assert_eq!(accumulator.delta_step(" world!"), None);
        assert_eq!(accumulator.delta_step(" How are you?"), Some("Hello world!".to_owned()));
        assert_eq!(accumulator.delta_step("I'm fine, thanks."), Some(" How are you?".to_owned()));
        assert_eq!(accumulator.delta_step("Ah yes, me too\nAnd you?"), Some("I'm fine, thanks.".to_owned()));
        assert_eq!(accumulator.delta_step(" Uh Good!"), Some("Ah yes, me too\n".to_owned()));
        assert_eq!(accumulator.delta_step("\n\nGreat!"), Some("And you? Uh Good!\n\n".to_owned()));
        assert_eq!(accumulator.delta_step("!!"), None);
        accumulator.finalize();
        assert_eq!(accumulator.delta_step(""), Some("Great!!!".to_owned()));

    }
}
