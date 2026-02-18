mod client;
mod error;
mod handlers;
pub mod models;
mod page;
pub mod params;

pub use client::{Octocrab, OctocrabBuilder, initialise, instance};
pub use error::{Error, GitHubError, Result, StatusCode};
pub use page::Page;

#[cfg(test)]
mod tests;
