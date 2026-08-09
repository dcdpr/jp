//! Conversation detail page: renders a single conversation's chat history.

use maud::{Markup, PreEscaped, html};

use crate::{render::RenderedEvent, views::layout};

/// Render the conversation detail page.
pub(crate) fn render(title: &str, events: &[RenderedEvent]) -> Markup {
    layout::page(title, html! {
        header class="page-header" {
            a href="/conversations" class="back" { "← Conversations" }
            h1 { (title) }
        }
        main class="conversation-detail" {
            @for event in events {
                @match event {
                    RenderedEvent::TurnSeparator => {
                        hr class="turn-separator";
                    }
                    RenderedEvent::UserMessage { html } => {
                        div class="message user" {
                            div class="role" { "You" }
                            div class="content" { (PreEscaped(html)) }
                        }
                    }
                    RenderedEvent::AssistantMessage { html } => {
                        div class="message assistant" {
                            div class="role" { "Assistant" }
                            div class="content" { (PreEscaped(html)) }
                        }
                    }
                    RenderedEvent::Reasoning { html } => {
                        details class="reasoning" {
                            summary { "Reasoning" }
                            div class="content" { (PreEscaped(html)) }
                        }
                    }
                    RenderedEvent::Structured { json } => {
                        div class="message assistant structured" {
                            div class="role" { "Assistant (structured)" }
                            pre class="content" { code { (json) } }
                        }
                    }
                    RenderedEvent::ToolCall { name, arguments, result } => {
                        details class="tool-call" {
                            summary { "Tool: " (name) }
                            @if !arguments.is_empty() {
                                div class="tool-args" {
                                    h4 { "Arguments" }
                                    pre { code { (arguments) } }
                                }
                            }
                            @if let Some(result) = result {
                                div class="tool-result" {
                                    h4 { "Result" }
                                    pre { code { (result) } }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}
