//! Conversions between ordered tool results and MCP wire content.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use jp_tool::{
    ContentBlock, ToolResult,
    content::{
        Annotations, ErrorDetails, ImageContent, Resource, ResourceContent, ResourceLink,
        ToolStatus,
    },
};
use rmcp::model::{
    AnnotateAble as _, Meta, RawAudioContent, RawContent, RawTextContent, ResourceContents,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Error as JsonError, Map, Value};

use crate::{CallToolResult, Content};

const ERROR_METADATA: &str = "computer.jp/error";

/// A result cannot be represented at the MCP boundary.
#[derive(Debug, thiserror::Error)]
pub enum ResultError {
    /// Typed protocol metadata is malformed or incompatible.
    #[error("Invalid tool result metadata: {0}")]
    Metadata(#[from] JsonError),
    /// Questions must be answered before a final MCP result is delivered.
    #[error("Tool result still requires input")]
    UnansweredQuestion,
}

fn convert<T: Serialize, U: DeserializeOwned>(value: T) -> Result<U, JsonError> {
    serde_json::from_value(serde_json::to_value(value)?)
}

/// Decode a native result without projecting away non-text content.
pub fn from_mcp(result: CallToolResult) -> Result<ToolResult, JsonError> {
    let metadata = result.meta.map(|meta| meta.0);
    let status = match result.is_error {
        None => ToolStatus::Unspecified,
        Some(false) => ToolStatus::Success,
        Some(true) => ToolStatus::Error(
            match metadata.as_ref().and_then(|meta| meta.get(ERROR_METADATA)) {
                Some(value) => serde_json::from_value(value.clone())?,
                None => ErrorDetails::default(),
            },
        ),
    };
    let content = result
        .content
        .into_iter()
        .map(from_content)
        .collect::<Result<_, _>>()?;
    Ok(ToolResult {
        content,
        status,
        structured_content: result.structured_content,
        metadata,
    })
}

fn from_content(content: Content) -> Result<ContentBlock, JsonError> {
    let annotations: Option<Annotations> = content.annotations.map(convert).transpose()?;
    Ok(match content.raw {
        RawContent::Text(text) => ContentBlock::Text {
            text: text.text,
            mime_type: None,
            annotations,
            metadata: text.meta.map(|meta| meta.0),
        },
        RawContent::Image(image) => ContentBlock::Image(ImageContent {
            data: image.data,
            mime_type: image.mime_type,
            annotations,
            metadata: image.meta.map(|meta| meta.0),
        }),
        RawContent::Audio(audio) => ContentBlock::Audio {
            data: audio.data,
            mime_type: audio.mime_type,
            annotations,
        },
        RawContent::Resource(embedded) => {
            let (uri, mime_type, content, content_metadata) = match embedded.resource {
                ResourceContents::TextResourceContents {
                    uri,
                    mime_type,
                    text,
                    meta,
                } => (uri, mime_type, ResourceContent::Text(text), meta),
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    meta,
                } => (uri, mime_type, ResourceContent::EncodedBlob(blob), meta),
            };
            ContentBlock::Resource(Resource {
                uri,
                content,
                mime_type,
                annotations,
                metadata: embedded.meta.map(|meta| meta.0),
                content_metadata: content_metadata.map(|meta| meta.0),
                name: None,
                title: None,
                description: None,
                formatted: None,
            })
        }
        RawContent::ResourceLink(link) => {
            let mut link: ResourceLink = convert(link)?;
            link.annotations = annotations;
            ContentBlock::ResourceLink(link)
        }
    })
}

/// Encode the final result; an unresolved question is a protocol error.
pub fn to_mcp(result: ToolResult) -> Result<CallToolResult, ResultError> {
    let ToolResult {
        content,
        status,
        structured_content,
        mut metadata,
    } = result;
    let is_error = match status {
        ToolStatus::Unspecified => None,
        ToolStatus::Success => Some(false),
        ToolStatus::Error(error) => {
            if error != ErrorDetails::default()
                || metadata
                    .as_ref()
                    .is_some_and(|meta| meta.contains_key(ERROR_METADATA))
            {
                let metadata = metadata.get_or_insert_with(Map::new);
                let mut details: Map<String, Value> = match metadata.remove(ERROR_METADATA) {
                    Some(value) => serde_json::from_value(value)?,
                    None => Map::new(),
                };
                let encoded: Map<String, Value> = convert(error)?;
                details.extend(encoded);
                metadata.insert(ERROR_METADATA.into(), Value::Object(details));
            }
            Some(true)
        }
    };
    let mut result = CallToolResult::success(
        content
            .into_iter()
            .map(to_content)
            .collect::<Result<_, _>>()?,
    );
    result.structured_content = structured_content;
    result.is_error = is_error;
    result.meta = metadata.map(Meta);
    Ok(result)
}

fn to_content(block: ContentBlock) -> Result<Content, ResultError> {
    let (mut content, annotations) = match block {
        ContentBlock::Text {
            text,
            annotations,
            metadata,
            ..
        } => (
            RawContent::Text(RawTextContent {
                text,
                meta: metadata.map(Meta),
            })
            .no_annotation(),
            annotations,
        ),
        ContentBlock::Image(media) => {
            let mut content = Content::image(media.data, media.mime_type);
            if let RawContent::Image(image) = &mut content.raw {
                image.meta = media.metadata.map(Meta);
            }
            (content, media.annotations)
        }
        ContentBlock::Audio {
            data,
            mime_type,
            annotations,
        } => {
            let raw = RawAudioContent { data, mime_type };
            (RawContent::Audio(raw).no_annotation(), annotations)
        }
        ContentBlock::Resource(resource) => {
            let embedded = match resource.content {
                ResourceContent::Text(text) => ResourceContents::TextResourceContents {
                    uri: resource.uri,
                    mime_type: resource.mime_type,
                    text,
                    meta: resource.content_metadata.map(Meta),
                },
                ResourceContent::EncodedBlob(blob) => ResourceContents::BlobResourceContents {
                    uri: resource.uri,
                    mime_type: resource.mime_type,
                    blob,
                    meta: resource.content_metadata.map(Meta),
                },
                ResourceContent::Blob(bytes) => ResourceContents::BlobResourceContents {
                    uri: resource.uri,
                    mime_type: resource.mime_type,
                    blob: STANDARD.encode(bytes),
                    meta: resource.content_metadata.map(Meta),
                },
            };
            let mut content = Content::resource(embedded);
            if let RawContent::Resource(embedded) = &mut content.raw {
                embedded.meta = resource.metadata.map(Meta);
            }
            (content, resource.annotations)
        }
        ContentBlock::ResourceLink(mut link) => {
            let annotations = link.annotations.take();
            (Content::resource_link(convert(link)?), annotations)
        }
        ContentBlock::Question(_) => return Err(ResultError::UnansweredQuestion),
    };
    content.annotations = annotations.map(convert).transpose()?;
    Ok(content)
}

/// Existing conversation-format projection, applied by the MCP Host only.
/// Image, audio, and links contribute no text; embedded blobs retain their
/// base64 form.
pub fn to_legacy(result: &ToolResult) -> Result<String, String> {
    let text = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            ContentBlock::Resource(resource) => Some(match &resource.content {
                ResourceContent::Text(text) | ResourceContent::EncodedBlob(text) => text.clone(),
                ResourceContent::Blob(bytes) => STANDARD.encode(bytes),
            }),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if result.is_error() {
        Err(text)
    } else {
        Ok(text)
    }
}

#[cfg(test)]
#[path = "result_tests.rs"]
mod tests;
