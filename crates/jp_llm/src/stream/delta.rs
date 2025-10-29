use crate::{
    Error,
    stream::{accumulator::Accumulator, event::StreamEvent},
};

#[derive(Debug, Default)]
pub struct Delta {
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_call_name: Option<String>,
    pub tool_call_arguments: Option<String>,
    pub tool_call_finished: bool,
}

impl Delta {
    pub fn into_stream_events(
        self,
        accumulator: &mut Accumulator,
    ) -> Result<Vec<StreamEvent>, Error> {
        accumulator.delta_step(&self)
    }

    pub fn content(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            ..Default::default()
        }
    }

    pub fn reasoning(reasoning: impl Into<String>) -> Self {
        Self {
            reasoning: Some(reasoning.into()),
            ..Default::default()
        }
    }

    pub fn tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        let id: String = id.into();
        let name: String = name.into();
        let arguments: String = arguments.into();

        Self {
            tool_call_id: (!id.is_empty()).then_some(id),
            tool_call_name: (!name.is_empty()).then_some(name),
            tool_call_arguments: (!arguments.is_empty()).then_some(arguments),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn finished(mut self) -> Self {
        self.tool_call_finished = true;
        self
    }

    pub fn tool_call_finished() -> Self {
        Self {
            tool_call_finished: true,
            ..Default::default()
        }
    }
}
