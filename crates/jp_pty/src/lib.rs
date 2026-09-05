//! A terminal harness for tests that assert on what a terminal renders.
//!
//! [`Terminal`] is the entry point.
//! It hands out a [`Writer`] for the code under test to draw into, models what
//! a terminal makes of those bytes, and answers questions about the result
//! through [`Screen`]: what row `N` holds, where the cursor is, whether a row
//! wrapped.
//!
//! ```no_run
//! use std::io::Write as _;
//!
//! use jp_pty::{Size, Terminal};
//!
//! let term = Terminal::open(Size::new(8, 40));
//! let mut tty = term.writer().unwrap();
//! write!(tty, "hello\n").unwrap();
//!
//! let screen = term
//!     .wait_for("the greeting", |s| s.row(0) == "hello")
//!     .unwrap();
//! assert_eq!(screen.cursor(), (1, 0));
//! ```
//!
//! # Backends
//!
//! A terminal is backed either by a real pty or by the screen model alone, and
//! the same test body works against both.
//!
//! - [`Terminal::pty`] opens a real pty.
//!   Output passes through the kernel's line discipline on its way to the
//!   model, the writer is a tty, and a child process can be spawned into it and
//!   resized under it.
//! - [`Terminal::modelled`] feeds the model directly, translating `\n` to
//!   `\r\n` the way a tty's `ONLCR` does.
//!   No child can be spawned into it.
//! - [`Terminal::open`] takes the pty where the platform provides one and falls
//!   back to the model where it does not, which is Windows and anywhere
//!   `openpty` fails.
//!   [`Terminal::backend`] reports which one it got.
//!
//! # Waiting
//!
//! Bytes reach a pty-backed model on a reader thread, so a screen read straight
//! after a write can catch a half-drawn frame.
//! [`Terminal::wait_for`] blocks until the screen satisfies a predicate and
//! returns the [`Screen`] that did, so the assertions that follow are made
//! against exactly that snapshot.
//! It is woken by arriving bytes rather than by the clock; nothing here sleeps
//! for a fixed interval.

#![warn(
    clippy::all,
    clippy::allow_attributes,
    clippy::missing_docs_in_private_items,
    clippy::nursery,
    clippy::pedantic,
    clippy::renamed_function_params,
    clippy::tests_outside_test_module,
    clippy::todo,
    clippy::try_err,
    clippy::unimplemented,
    clippy::unneeded_field_pattern,
    clippy::unseparated_literal_suffix,
    clippy::unused_result_ok,
    clippy::unused_trait_names,
    clippy::use_debug,
    clippy::unwrap_used,
    missing_docs,
    rustdoc::all,
    unused_doc_comments
)]
#![allow(
    rustdoc::private_intra_doc_links,
    reason = "we don't host the docs, and use them mainly for LSP integration"
)]

mod error;
mod screen;
mod terminal;

pub use error::Error;
pub use portable_pty::CommandBuilder;
pub use screen::Screen;
pub use terminal::{Backend, Child, Size, Terminal, Writer};
