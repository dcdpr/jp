//! Typewriter printing utilities.

use std::{iter::Peekable, str::Chars};

/// The state of the ANSI parser.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum AnsiParseState {
    /// Normal state.
    Normal,

    /// Escape sequence state.
    Escape,

    /// CSI sequence state.
    Csi,

    /// OSC sequence state.
    Osc,
}

/// An iterator that yields characters from an input string along with a boolean
/// indicating if the character is considered "visual" (i.e., not part of an
/// ANSI escape sequence or a non-ESC control character).
#[derive(Debug)]
pub struct VisibleCharsIterator<'a> {
    /// The underlying iterator.
    chars: Peekable<Chars<'a>>,

    /// The current state of the ANSI parser.
    state: AnsiParseState,
}

impl<'a> VisibleCharsIterator<'a> {
    /// Creates a new `VisibleCharsIterator` for the given input string.
    pub fn new(input: &'a str) -> Self {
        VisibleCharsIterator {
            chars: input.chars().peekable(),
            state: AnsiParseState::Normal,
        }
    }
}

impl Iterator for VisibleCharsIterator<'_> {
    type Item = (char, bool);

    fn next(&mut self) -> Option<Self::Item> {
        // Get the next character from the underlying iterator.
        // If the string is exhausted, return None.
        let c = self.chars.next()?;

        // Default to non-visual. It will be set true only in the Normal state
        // for non-control, non-escape characters.
        let mut is_visual = false;

        // Determine visual status and update state based on the state *before*
        // processing the current character 'c'.
        let current_state = self.state;

        match current_state {
            AnsiParseState::Normal => match c {
                '\x1b' => self.state = AnsiParseState::Escape,
                _ if c.is_control() => {}
                _ => is_visual = true,
            },

            AnsiParseState::Escape => match c {
                '[' => self.state = AnsiParseState::Csi,
                ']' => self.state = AnsiParseState::Osc,
                _ => self.state = AnsiParseState::Normal,
            },

            AnsiParseState::Csi => {
                match c {
                    // Parameter or Intermediate bytes
                    '\u{0020}'..='\u{003F}' => {}
                    // Anything else (Final byte, control char, unexpected) ends
                    // the sequence
                    _ => self.state = AnsiParseState::Normal,
                }
            }

            AnsiParseState::Osc => {
                match c {
                    // BEL terminates
                    '\x07' => self.state = AnsiParseState::Normal,
                    '\x1b' => {
                        // Check for String Terminator "ESC \"
                        if self.chars.peek() == Some(&'\\') {
                            // Consume the '\' as it's part of the terminator
                            // sequence
                            self.chars.next();
                        }

                        self.state = AnsiParseState::Normal;
                    }
                    _ => {}
                }
            }
        }

        Some((c, is_visual))
    }
}
