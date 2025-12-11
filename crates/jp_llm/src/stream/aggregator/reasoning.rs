#[derive(Default, Debug)]
/// A parser that segments a stream of text into 'reasoning' and 'other'
/// buckets. It handles streams with or without a `<think>` block.
pub struct ReasoningExtractor {
    pub other: String,
    pub reasoning: String,
    buffer: String,
    state: ReasoningState,
}

#[derive(Default, PartialEq, Debug)]
enum ReasoningState {
    #[default]
    /// The default state. Processing 'other' text while looking for
    /// `<think>\n`.
    Idle,
    /// Found `<think>\n`. Processing 'reasoning' text while looking for
    /// `</think>\n`.
    Accumulating,
    /// Found `</think>\n`. All subsequent text is 'other'.
    Finished,
}

impl ReasoningExtractor {
    /// Processes a chunk of the incoming text stream.
    pub fn handle(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        self.buffer.push_str(text);

        loop {
            match self.state {
                ReasoningState::Idle => {
                    if let Some(tag_start_index) = self.buffer.find("<think>\n") {
                        // Tag found. Text before it is 'other'.
                        self.other.push_str(&self.buffer[..tag_start_index]);

                        // Drain the processed 'other' text and the tag itself.
                        let tag_end_offset = tag_start_index + "<think>\n".len();
                        self.buffer.drain(..tag_end_offset);

                        // Transition state and re-process the rest of the
                        // buffer.
                        self.state = ReasoningState::Accumulating;
                    } else {
                        // No tag found. We can safely move most of the buffer
                        // to `other`, but must keep a small tail in case a tag
                        // is split across chunks.
                        let tail_len = self.buffer.len().min("<think>\n".len() - 1);
                        let mut drain_to = self.buffer.len() - tail_len;

                        if drain_to > 0 {
                            while !self.buffer.is_char_boundary(drain_to) {
                                drain_to += 1;
                            }

                            self.other.push_str(&self.buffer[..drain_to]);
                            self.buffer.drain(..drain_to);
                        }

                        // Wait for more data.
                        return;
                    }
                }
                ReasoningState::Accumulating => {
                    if let Some(tag_start_index) = self.buffer.find("</think>\n") {
                        // Closing tag found. Text before it is 'thinking'.
                        self.reasoning.push_str(&self.buffer[..tag_start_index]);

                        // Drain the 'reasoning' text and the tag.
                        let tag_end_offset = tag_start_index + "</think>\n".len();
                        self.buffer.drain(..tag_end_offset);

                        // Transition state and re-process.
                        self.state = ReasoningState::Finished;
                    } else {
                        // No closing tag found yet. Move "safe" part of the
                        // buffer to `reasoning`.
                        let tail_len = self.buffer.len().min("</think>\n".len() - 1);
                        let drain_to = self.buffer.len() - tail_len;

                        if drain_to > 0 {
                            self.reasoning.push_str(&self.buffer[..drain_to]);
                            self.buffer.drain(..drain_to);
                        }

                        // Wait for more data.
                        return;
                    }
                }
                ReasoningState::Finished => {
                    // Everything from now on is 'other'. No need for complex
                    // buffering.
                    self.other.push_str(&self.buffer);
                    self.buffer.clear();
                    return;
                }
            }
        }
    }

    /// Call this after the stream has finished to process any remaining data
    /// and fix potential unclosed thinking blocks.
    pub fn finalize(&mut self) {
        match self.state {
            ReasoningState::Accumulating => {
                let (reasoning, other) = self
                    .buffer
                    .split_once("</think>")
                    .unwrap_or((self.buffer.as_str(), ""));

                self.reasoning.push_str(reasoning);
                self.other.push_str(other);
            }
            _ => {
                self.other.push_str(&self.buffer);
            }
        }
        self.buffer.clear();
    }
}
