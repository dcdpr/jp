use std::{
    collections::{HashMap, VecDeque},
    mem,
};

use indexmap::IndexMap;
use schematic_types::{
    ArrayType, BooleanType, EnumType, FloatType, IntegerType, LiteralType, LiteralValue,
    ObjectType, Schema, SchemaField, SchemaType, StringType, TupleType, UnionType,
};

use crate::schema::{RenderError, RenderResult};

/// Options to control the rendered template.
pub struct TemplateOptions {
    /// Include field comments in output.
    pub comments: bool,

    /// List of field names to render but comment out.
    pub comment_fields: Vec<String>,

    /// Characters to prefix a comment line.
    pub comment_prefix: String,

    /// Custom values for each field. Supports dot notation.
    pub custom_values: HashMap<String, Schema>,

    /// List of array and object field names to expand and render a fake item.
    pub expand_fields: Vec<String>,

    /// Content to append to the bottom of the output.
    pub footer: String,

    /// Content to prepend to the top of the output.
    pub header: String,

    /// List of field names to not render.
    pub hide_fields: Vec<String>,

    /// Character(s) to use for indentation.
    pub indent_char: String,

    /// Insert an extra newline between fields.
    pub newline_between_fields: bool,

    /// List of field names to only render.
    pub only_fields: Vec<String>,

    /// Print the list of enum values for enum fields.
    pub print_enum_values: bool,

    /// List of custom environment variables to use for fields. Supports dot
    /// notation.
    pub env_vars: HashMap<String, String>,
}

impl Default for TemplateOptions {
    fn default() -> Self {
        Self {
            comments: true,
            comment_fields: vec![],
            comment_prefix: "# ".into(),
            custom_values: HashMap::new(),
            expand_fields: vec![],
            footer: String::new(),
            header: String::new(),
            hide_fields: vec![],
            indent_char: "  ".into(),
            newline_between_fields: true,
            only_fields: vec![],
            print_enum_values: true,
            env_vars: HashMap::new(),
        }
    }
}

pub fn lit_to_string(lit: &LiteralValue) -> String {
    match lit {
        LiteralValue::Bool(inner) => inner.to_string(),
        LiteralValue::F32(inner) => inner.to_string(),
        LiteralValue::F64(inner) => inner.to_string(),
        LiteralValue::Int(inner) => inner.to_string(),
        LiteralValue::UInt(inner) => inner.to_string(),
        LiteralValue::String(inner) => format!("\"{inner}\""),
    }
}

pub fn is_nested_type(schema: &SchemaType) -> bool {
    match schema {
        SchemaType::Struct(sct) => !sct.fields.is_empty(),
        SchemaType::Union(uni) if uni.has_null() && uni.variants_types.len() == 2 => uni
            .variants_types
            .iter()
            .find(|v| !v.is_null())
            .is_some_and(|v| is_nested_type(v)),
        _ => false,
    }
}

pub struct TemplateContext {
    pub depth: usize,
    pub options: TemplateOptions,

    stack: VecDeque<String>,
}

impl TemplateContext {
    pub fn new(options: TemplateOptions) -> Self {
        Self {
            depth: 0,
            options,
            stack: VecDeque::new(),
        }
    }

    pub fn indent(&self) -> String {
        if self.depth == 0 {
            String::new()
        } else {
            self.options.indent_char.repeat(self.depth)
        }
    }

    pub fn gap(&self) -> &str {
        if self.options.newline_between_fields {
            "\n\n"
        } else {
            "\n"
        }
    }

    pub fn create_field_comment(&self, field: &SchemaField) -> String {
        let key = self.get_stack_key();

        if !self.options.comments {
            return String::new();
        }

        let mut lines = vec![];
        let indent = self.indent();
        let prefix = self.get_comment_prefix();

        let mut push = |line: String| {
            lines.push(format!("{indent}{prefix}{line}"));
        };

        if let Some(comment) = &field.comment {
            comment
                .trim()
                .lines()
                .for_each(|c| push(c.trim().to_owned()));
        }

        if let Some(deprecated) = &field.deprecated {
            push(if deprecated.is_empty() {
                "@deprecated".into()
            } else {
                format!("@deprecated {deprecated}")
            });
        }

        if let Some(env_var) = &field
            .env_var
            .as_ref()
            .or_else(|| self.options.env_vars.get(&key))
            && !env_var.is_empty()
        {
            push(format!("@env {env_var}"));
        }

        if let SchemaType::Enum(enu) = &field.schema.ty
            && self.options.print_enum_values
        {
            let enum_values = render_enum_values(enu);
            if !enum_values.is_empty() {
                push(format!("@values {enum_values}"));
            }
        }

        if lines.is_empty() {
            return String::new();
        }

        let mut out = lines.join("\n");
        out.push('\n');
        out
    }

    pub fn create_field(&self, field: &SchemaField, property: &str) -> String {
        let key = self.get_stack_key();

        format!(
            "{}{}{}{property}",
            self.create_field_comment(field),
            self.indent(),
            if self.options.comment_fields.contains(&key) {
                self.get_comment_prefix()
            } else {
                ""
            },
        )
    }

    pub fn get_comment_prefix(&self) -> &str {
        &self.options.comment_prefix
    }

    pub fn get_stack_key(&self) -> String {
        let mut key = String::new();
        let last_index = self.stack.len() - 1;

        for (index, item) in self.stack.iter().enumerate() {
            key.push_str(item);

            if index != last_index {
                key.push('.');
            }
        }

        key
    }

    pub fn get_stack_value(&self) -> Option<Schema> {
        let key = self.get_stack_key();

        self.options.custom_values.get(&key).cloned()
    }

    pub fn is_expanded(&self, key: &String) -> bool {
        self.options.expand_fields.contains(key)
    }

    pub fn is_hidden(&self, field: &SchemaField) -> bool {
        let key = self.get_stack_key();

        field.hidden
            || self.options.hide_fields.contains(&key)
            || !self.options.only_fields.is_empty() && !self.options.only_fields.contains(&key)
    }

    pub fn push_stack(&mut self, name: &str) {
        self.stack.push_back(name.to_owned());
    }

    pub fn pop_stack(&mut self) {
        self.stack.pop_back();
    }

    pub fn resolve_schema(initial: &Schema, schemas: &IndexMap<String, Schema>) -> Schema {
        if let SchemaType::Reference(ty) = &initial.ty
            && let Some(schema) = schemas.get(&ty.name)
        {
            return schema.to_owned();
        }

        initial.to_owned()
    }

    pub fn validate_schema_variant<'a>(
        &self,
        custom: Option<&'a Schema>,
        fallback: &'a Schema,
    ) -> &'a Schema {
        if let Some(custom) = custom {
            if mem::discriminant(&custom.ty) == mem::discriminant(&fallback.ty) {
                return custom;
            }
            panic!(
                "Received an invalid custom value for `{}`, mismatched schema types.\n\nExpected: \
                 {:#?}\n\nReceived: {:#?}",
                self.get_stack_key(),
                fallback,
                custom
            );
        }

        fallback
    }
}

pub fn render_array(_array: &ArrayType) -> String {
    "[]".into()
}

pub fn render_boolean(boolean: &BooleanType) -> String {
    if let Some(default) = &boolean.default {
        return lit_to_string(default);
    }

    "false".into()
}

pub fn render_enum(enu: &EnumType) -> String {
    let index = enu.default_index.unwrap_or(0);

    if let Some(value) = enu.values.get(index) {
        return lit_to_string(value);
    }

    render_null()
}

pub fn render_enum_values(enu: &EnumType) -> String {
    let values: Vec<String> = match &enu.variants {
        Some(variants) => variants
            .iter()
            .filter_map(|(_, variant)| {
                if variant.hidden {
                    None
                } else if let SchemaType::Literal(lit) = &variant.schema.ty {
                    Some(lit_to_string(&lit.value))
                } else {
                    None
                }
            })
            .collect(),
        None => enu.values.iter().map(lit_to_string).collect(),
    };

    values.join(" | ")
}

pub fn render_float(float: &FloatType) -> String {
    if let Some(default) = &float.default {
        return lit_to_string(default);
    }

    "0.0".into()
}

pub fn render_integer(integer: &IntegerType) -> String {
    if let Some(default) = &integer.default {
        return lit_to_string(default);
    }

    "0".into()
}

pub fn render_literal(literal: &LiteralType) -> String {
    lit_to_string(&literal.value)
}

pub fn render_null() -> String {
    "null".into()
}

pub fn render_object(_object: &ObjectType) -> String {
    "{}".into()
}

pub fn render_reference(reference: &str) -> String {
    reference.into()
}

pub const EMPTY_STRING: &str = "\"\"";

pub fn render_string(string: &StringType) -> String {
    if let Some(default) = &string.default {
        return lit_to_string(default);
    }

    EMPTY_STRING.into()
}

pub fn render_tuple(
    tuple: &TupleType,
    mut render: impl FnMut(&Schema) -> RenderResult,
) -> RenderResult {
    let mut items = vec![];

    for item in &tuple.items_types {
        items.push(render(item)?);
    }

    Ok(format!("[{}]", items.join(", ")))
}

pub fn render_union(
    uni: &UnionType,
    mut render: impl FnMut(&Schema) -> RenderResult,
) -> RenderResult {
    if let Some(index) = &uni.default_index
        && let Some(variant) = uni.variants_types.get(*index)
    {
        return render(variant);
    }

    // We have a nullable type, so render the non-null value
    if uni.has_null()
        && let Some(variant) = uni.variants_types.iter().find(|v| !v.is_null())
    {
        return render(variant);
    }

    if let Some(variant) = uni.variants_types.first() {
        return render(variant);
    }

    Ok(render_null())
}

pub fn render_unknown() -> String {
    render_null()
}

pub fn validate_root(schemas: &IndexMap<String, Schema>) -> Result<Schema, RenderError> {
    let Some(schema) = schemas.values().last() else {
        return Err(RenderError::custom(
            "At least 1 schema is required to generate a template.",
        ));
    };

    if !schema.is_struct() {
        return Err(RenderError::custom(
            "The last registered schema must be a struct type.",
        ));
    }

    Ok(schema.to_owned())
}
