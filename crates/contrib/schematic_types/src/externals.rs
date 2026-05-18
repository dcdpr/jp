#![allow(deprecated, unused_imports, unused_macros)]

use crate::{
    ArrayType, IntegerKind, IntegerType, ObjectType, Schema, SchemaBuilder, Schematic, StringType,
};

macro_rules! impl_unknown {
    ($type:ty) => {
        impl Schematic for $type {}
    };
}

macro_rules! impl_set {
    ($type:ty) => {
        impl<T: Schematic, S> Schematic for $type {
            fn build_schema(mut schema: SchemaBuilder) -> Schema {
                schema.array(ArrayType::new(schema.infer::<T>()))
            }
        }
    };
}

macro_rules! impl_map {
    ($type:ty) => {
        impl<K: Schematic, V: Schematic, S> Schematic for $type {
            fn build_schema(mut schema: SchemaBuilder) -> Schema {
                schema.object(ObjectType::new(schema.infer::<K>(), schema.infer::<V>()))
            }
        }
    };
}

macro_rules! impl_string {
    ($type:ty) => {
        impl Schematic for $type {
            fn build_schema(mut schema: SchemaBuilder) -> Schema {
                schema.string_default()
            }
        }
    };
}

macro_rules! impl_string_format {
    ($type:ty, $format:expr_2021) => {
        impl Schematic for $type {
            fn build_schema(mut schema: SchemaBuilder) -> Schema {
                schema.string(StringType {
                    format: Some($format.into()),
                    ..StringType::default()
                })
            }
        }
    };
}

#[cfg(feature = "indexmap")]
mod indexmap_feature {
    use super::{ArrayType, ObjectType, Schema, SchemaBuilder, Schematic};

    impl_map!(indexmap::IndexMap<K, V, S>);
    impl_set!(indexmap::IndexSet<T, S>);
}

#[cfg(feature = "relative_path")]
mod relative_path_feature {
    use super::{Schema, SchemaBuilder, Schematic, StringType};

    impl_string_format!(&relative_path::RelativePath, "path");
    impl_string_format!(relative_path::RelativePath, "path");
    impl_string_format!(relative_path::RelativePathBuf, "path");
}

#[cfg(feature = "serde_json")]
mod serde_json_feature {
    use super::{IntegerKind, IntegerType, ObjectType, Schema, SchemaBuilder, Schematic};

    impl_unknown!(serde_json::Value);

    // This isn't accurate since we can't access the `N` enum
    impl Schematic for serde_json::Number {
        fn build_schema(mut schema: SchemaBuilder) -> Schema {
            schema.integer(IntegerType::new_kind(IntegerKind::I64))
        }
    }

    impl<K: Schematic, V: Schematic> Schematic for serde_json::Map<K, V> {
        fn build_schema(mut schema: SchemaBuilder) -> Schema {
            schema.object(ObjectType::new(schema.infer::<K>(), schema.infer::<V>()))
        }
    }
}

#[cfg(feature = "serde_toml")]
mod serde_toml_feature {
    use super::{ObjectType, Schema, SchemaBuilder, Schematic};

    impl_unknown!(toml::Value);

    impl<K: Schematic, V: Schematic> Schematic for toml::map::Map<K, V> {
        fn build_schema(mut schema: SchemaBuilder) -> Schema {
            schema.object(ObjectType::new(schema.infer::<K>(), schema.infer::<V>()))
        }
    }
}

#[cfg(feature = "serde_yaml")]
mod serde_yaml_feature {
    use super::{IntegerKind, IntegerType, ObjectType, Schema, SchemaBuilder, Schematic};

    impl_unknown!(serde_yaml::Value);

    // This isn't accurate since we can't access the `N` enum
    impl Schematic for serde_yaml::Number {
        fn build_schema(mut schema: SchemaBuilder) -> Schema {
            schema.integer(IntegerType::new_kind(IntegerKind::I64))
        }
    }

    impl Schematic for serde_yaml::Mapping {
        fn build_schema(mut schema: SchemaBuilder) -> Schema {
            schema.object(ObjectType::new(
                schema.infer::<serde_yaml::Value>(),
                schema.infer::<serde_yaml::Value>(),
            ))
        }
    }
}

#[cfg(feature = "url")]
mod url_feature {
    use super::{Schema, SchemaBuilder, Schematic, StringType};

    impl_string_format!(url::Url, "uri");
}
