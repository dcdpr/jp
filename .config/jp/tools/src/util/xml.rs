use std::fmt::Write as _;

use serde::Serialize;
use serde_json::Value;

use crate::Result;

const INDENT_WIDTH: usize = 2;

/// Serializes any Serializable type into a pretty-printed, LLM-friendly XML format.
pub fn to_simple_xml_with_root<T: Serialize>(data: &T, root: &str) -> Result<String> {
    let value = serde_json::to_value(data)?;

    // Start with <root> and a newline
    let mut output = format!("<{root}>\n");

    match value {
        Value::Array(vec) => {
            // Case 1: Top-level Vector
            let tag_name = infer_array_tag_name::<T>();
            for item in vec {
                write_xml_node(&mut output, &tag_name, &item, 1)?;
            }
        }
        Value::Object(map) => {
            // Case 2: Top-level Struct
            // We write the struct fields directly as children of <root>
            for (key, val) in map {
                write_xml_node(&mut output, &key, &val, 1)?;
            }
        }
        _ => {
            // Case 3: Top-level Primitive
            // We just write the value indented once
            write_indent(&mut output, 1);
            write_content(&mut output, &value)?;
            output.push('\n');
        }
    }

    output.push_str(&format!("</{root}>"));
    Ok(output)
}

/// Recursive function to write XML nodes with indentation.
fn write_xml_node(out: &mut String, key: &str, value: &Value, depth: usize) -> std::fmt::Result {
    match value {
        Value::Null => Ok(()), // Skip nulls

        Value::Array(vec) => {
            // Flattening: We do NOT indent or write a tag for the array itself.
            // We iterate and write the children at the CURRENT depth.
            for item in vec {
                write_xml_node(out, key, item, depth)?;
            }
            Ok(())
        }

        Value::Object(map) => {
            // 1. Indent + Open Tag
            write_indent(out, depth);
            writeln!(out, "<{key}>")?;

            // 2. Write Children (depth + 1)
            for (child_key, child_val) in map {
                write_xml_node(out, child_key, child_val, depth + 1)?;
            }

            // 3. Indent + Close Tag
            write_indent(out, depth);
            writeln!(out, "</{key}>")?;
            Ok(())
        }

        _ => {
            // Leaf node (Primitive): Written inline on a single line
            // <key>value</key>
            write_indent(out, depth);
            write!(out, "<{key}>")?;
            write_content(out, value)?;
            writeln!(out, "</{key}>")?;
            Ok(())
        }
    }
}

/// Helper to write indentation spaces
fn write_indent(out: &mut String, depth: usize) {
    for _ in 0..(depth * INDENT_WIDTH) {
        out.push(' ');
    }
}

/// Writes the raw content of a primitive value, applying CDATA if needed.
fn write_content(out: &mut String, value: &Value) -> std::fmt::Result {
    match value {
        Value::String(s) => {
            if s.contains('<') || s.contains('>') {
                write!(out, "<![CDATA[\n{s}\n]]>")
            } else {
                write!(out, "{s}")
            }
        }
        Value::Bool(b) => write!(out, "{b}"),
        Value::Number(n) => write!(out, "{n}"),
        _ => Ok(()),
    }
}

/// Helper to guess a tag name from a type string
fn infer_array_tag_name<T: ?Sized>() -> String {
    let type_name = std::any::type_name::<T>();
    let inner = if let Some(start) = type_name.find('<') {
        if let Some(end) = type_name.rfind('>') {
            &type_name[start + 1..end]
        } else {
            type_name
        }
    } else {
        type_name
    };
    let clean_name = inner.split("::").last().unwrap_or("item");
    let tag = clean_name.to_lowercase();
    match tag.as_str() {
        "string" | "str" | "i32" | "i64" | "u32" | "u64" | "f64" | "bool" => "item".to_string(),
        _ => tag,
    }
}
