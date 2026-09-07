//! A compact, line-oriented rendering of a [`Schema`] tree.
//!
//! [`render`] is the entry point.
//! It turns a schema into an indented outline where each line is one field and
//! the shape it accepts, so a change to what the config claims to accept shows
//! up as a small diff in a snapshot instead of a few lines buried in several
//! thousand of `Debug` output.
//!
//! Scalar shapes are rendered inline (`"a" | "b" | int`); structs, maps, arrays
//! and unions open an indented block whose children are labelled by their
//! position: a struct field by its name, a map value by `*`, an array item by
//! `[]`, and a union variant by `|`.
//! A trailing `?` on a label marks a field the schema reports as optional.
//!
//! A named type that encloses itself renders as `@Name` rather than being
//! expanded again, matching how the schema builder represents the same cycle.

use std::fmt::Write as _;

use schematic::{
    Schema, SchemaType,
    schema::{EnumType, LiteralValue, StructType, UnionType},
};

/// Render a schema as an indented outline, one line per field.
///
/// The output ends with a newline and is stable across runs: struct fields come
/// from a `BTreeMap` and every other collection keeps declaration order.
#[must_use]
pub fn render(schema: &Schema) -> String {
    let mut out = String::new();
    let mut enclosing = Vec::new();

    if is_inline(schema) {
        let _ = writeln!(out, "{}", inline(schema));
    } else {
        write_body(&mut out, schema, 0, &mut enclosing);
    }

    out
}

/// Whether a schema renders as a single inline token rather than a block.
fn is_inline(schema: &Schema) -> bool {
    match &schema.ty {
        SchemaType::Struct(_) | SchemaType::Object(_) => false,
        SchemaType::Array(array) => is_inline(&array.items_type),
        SchemaType::Union(union) => union.variants_types.iter().all(|v| is_inline(v)),
        _ => true,
    }
}

/// Render a schema that fits on one line.
fn inline(schema: &Schema) -> String {
    match &schema.ty {
        SchemaType::Null => "null".to_owned(),
        SchemaType::Unknown => "unknown".to_owned(),
        SchemaType::Boolean(_) => "bool".to_owned(),
        SchemaType::Integer(_) => "int".to_owned(),
        SchemaType::Float(_) => "float".to_owned(),
        SchemaType::String(string) => string
            .pattern
            .as_ref()
            .map_or_else(|| "string".to_owned(), |p| format!("string(/{p}/)")),
        SchemaType::Literal(literal) => literal.value.to_string(),
        SchemaType::Enum(enum_type) => inline_enum(enum_type),
        SchemaType::Reference(reference) => format!("@{}", reference.name),
        SchemaType::Array(array) => format!("[{}]", inline(&array.items_type)),
        SchemaType::Tuple(tuple) => {
            let items: Vec<_> = tuple.items_types.iter().map(|i| inline(i)).collect();
            format!("({})", items.join(", "))
        }
        SchemaType::Union(union) => {
            let variants: Vec<_> = union.variants_types.iter().map(|v| inline(v)).collect();
            variants.join(" | ")
        }
        // `is_inline` sends both of these to `write_body` instead.
        SchemaType::Struct(_) => "struct".to_owned(),
        SchemaType::Object(_) => "map".to_owned(),
    }
}

/// Render an enum's accepted values, with each variant's aliases beside it.
///
/// `variants` is the richer of the two representations an [`EnumType`] carries:
/// it holds every variant, including one whose shape is not a literal (a
/// catch-all accepting any string, say), which `values` alone cannot express.
fn inline_enum(enum_type: &EnumType) -> String {
    let Some(variants) = &enum_type.variants else {
        let values: Vec<_> = enum_type
            .values
            .iter()
            .map(LiteralValue::to_string)
            .collect();
        return values.join(" | ");
    };

    let rendered: Vec<_> = variants
        .values()
        .map(|field| {
            let value = inline(&field.schema);
            if field.aliases.is_empty() {
                value
            } else {
                format!("{value} (aka {})", field.aliases.join(", "))
            }
        })
        .collect();

    rendered.join(" | ")
}

/// Write the children of a block schema, each at `depth`.
fn write_body(out: &mut String, schema: &Schema, depth: usize, enclosing: &mut Vec<String>) {
    match &schema.ty {
        SchemaType::Struct(struct_type) => write_struct_body(out, struct_type, depth, enclosing),
        SchemaType::Object(object) => {
            write_labelled(out, "*", &object.value_type, depth, enclosing);
        }
        SchemaType::Array(array) => {
            write_labelled(out, "[]", &array.items_type, depth, enclosing);
        }
        SchemaType::Union(union) => write_union_body(out, union, depth, enclosing),
        _ => {
            let _ = writeln!(out, "{}{}", "  ".repeat(depth), inline(schema));
        }
    }
}

/// Write a struct's fields, one label per field, sorted by field name.
fn write_struct_body(
    out: &mut String,
    struct_type: &StructType,
    depth: usize,
    enclosing: &mut Vec<String>,
) {
    if struct_type.fields.is_empty() {
        let _ = writeln!(out, "{}(no fields)", "  ".repeat(depth));
        return;
    }

    for (name, field) in &struct_type.fields {
        let mut label = name.clone();

        if field.optional {
            label.push('?');
        }
        if field.flatten {
            label.push_str(" (flattened)");
        }
        if field.hidden {
            label.push_str(" (hidden)");
        }
        if !field.aliases.is_empty() {
            let _ = write!(label, " (aka {})", field.aliases.join(", "));
        }

        write_labelled(out, &label, &field.schema, depth, enclosing);
    }
}

/// Write a union's variants, marking the one the others are shorthand for.
fn write_union_body(
    out: &mut String,
    union: &UnionType,
    depth: usize,
    enclosing: &mut Vec<String>,
) {
    for (index, variant) in union.variants_types.iter().enumerate() {
        let label = if Some(index) == union.expanded_index {
            "| (expanded)"
        } else {
            "|"
        };

        write_labelled(out, label, variant, depth, enclosing);
    }
}

/// Write `label: <shape>` on one line, or `label:` followed by an indented
/// block.
fn write_labelled(
    out: &mut String,
    label: &str,
    schema: &Schema,
    depth: usize,
    enclosing: &mut Vec<String>,
) {
    let pad = "  ".repeat(depth);

    if is_inline(schema) {
        let _ = writeln!(out, "{pad}{label}: {}", inline(schema));
        return;
    }

    if let Some(name) = &schema.name {
        if enclosing.contains(name) {
            let _ = writeln!(out, "{pad}{label}: @{name}");
            return;
        }

        let _ = writeln!(out, "{pad}{label}: {name}");
        enclosing.push(name.clone());
        write_body(out, schema, depth + 1, enclosing);
        enclosing.pop();
        return;
    }

    let _ = writeln!(out, "{pad}{label}:");
    write_body(out, schema, depth + 1, enclosing);
}

#[cfg(test)]
#[path = "schema_shape_tests.rs"]
mod tests;
