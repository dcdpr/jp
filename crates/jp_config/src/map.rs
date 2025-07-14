use std::{
    fmt,
    hash::Hash,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use confique::{
    internal::map_err_prefix_path,
    meta::{Field, FieldKind, Meta},
    Config, Error, Partial,
};
use indexmap::IndexMap;
use serde::{
    de::{Deserializer, MapAccess, Visitor},
    ser::SerializeMap as _,
    Deserialize, Serialize, Serializer,
};

#[derive(Serialize)]
pub struct ConfigMap<K, V: Config>(IndexMap<K, V>);

pub struct ConfigMapPartial<K, V: Partial>(IndexMap<K, V>);

impl<K, V> Serialize for ConfigMapPartial<K, V>
where
    K: Serialize + std::fmt::Debug,
    V: Serialize + Partial + PartialEq + std::fmt::Debug,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.iter() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<K, V: Config> Deref for ConfigMap<K, V> {
    type Target = IndexMap<K, V>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K, V: Partial> Deref for ConfigMapPartial<K, V> {
    type Target = IndexMap<K, V>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K, V: Config> DerefMut for ConfigMap<K, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<K, V: Partial> DerefMut for ConfigMapPartial<K, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<K, V> fmt::Debug for ConfigMap<K, V>
where
    K: fmt::Debug,
    V: fmt::Debug + Config,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V> fmt::Debug for ConfigMapPartial<K, V>
where
    K: fmt::Debug,
    V: fmt::Debug + Partial,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V> PartialEq for ConfigMap<K, V>
where
    K: Eq + Hash,
    V: PartialEq + Config,
{
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl<K, V> PartialEq for ConfigMapPartial<K, V>
where
    K: Eq + Hash,
    V: PartialEq + Partial,
{
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl<K, V: Config> Default for ConfigMap<K, V> {
    fn default() -> Self {
        Self(IndexMap::default())
    }
}

impl<K, V: Partial> Default for ConfigMapPartial<K, V> {
    fn default() -> Self {
        Self(IndexMap::default())
    }
}

impl<K, V> Clone for ConfigMap<K, V>
where
    K: Clone,
    V: Config + Clone,
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K, V> Clone for ConfigMapPartial<K, V>
where
    K: Clone,
    V: Clone + Partial,
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

// See: <https://serde.rs/deserialize-map.html>
struct ConfigMapVisitor<K, V: Config> {
    marker: PhantomData<fn() -> ConfigMap<K, V>>,
}

impl<K, V: Config> ConfigMapVisitor<K, V> {
    fn new() -> Self {
        ConfigMapVisitor {
            marker: PhantomData,
        }
    }
}

impl<'de, K, V> Visitor<'de> for ConfigMapVisitor<K, V>
where
    K: Deserialize<'de> + Eq + Hash,
    V: Deserialize<'de> + Config,
{
    type Value = ConfigMap<K, V>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a config map")
    }

    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut map = ConfigMap::default();
        while let Some((key, value)) = access.next_entry()? {
            map.insert(key, value);
        }

        Ok(map)
    }
}

impl<'de, K, V> Deserialize<'de> for ConfigMap<K, V>
where
    K: Deserialize<'de> + Eq + Hash,
    V: Deserialize<'de> + Config,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ConfigMapVisitor::new())
    }
}

impl<'de, K, V> Deserialize<'de> for ConfigMapPartial<K, V>
where
    K: Deserialize<'de> + Eq + Hash,
    V: Partial,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self(IndexMap::deserialize(deserializer)?))
    }
}

pub(crate) trait ConfigKey: for<'de> Deserialize<'de> + Eq + Hash {
    const KIND: &'static str;
}

impl<K, V> Config for ConfigMap<K, V>
where
    K: ConfigKey,
    V: Config,
{
    type Partial = ConfigMapPartial<K, V::Partial>;

    const META: Meta = Meta {
        name: std::any::type_name::<Self>(),
        doc: &["A config map of key-value pairs."],
        fields: &[Field {
            name: K::KIND,
            doc: &[],
            kind: FieldKind::Nested { meta: &V::META },
        }],
    };

    fn from_partial(partial: Self::Partial) -> Result<Self, Error> {
        let map = partial
            .0
            .into_iter()
            .map(|(k, v)| {
                map_err_prefix_path(V::from_partial(v), Self::META.fields[0].name).map(|v| (k, v))
            })
            .collect::<Result<_, Error>>()?;

        Ok(Self(map))
    }
}

impl<K, V> Partial for ConfigMapPartial<K, V>
where
    K: for<'de> Deserialize<'de> + Eq + Hash,
    V: Partial,
{
    fn empty() -> Self {
        Self(IndexMap::new())
    }

    fn default_values() -> Self {
        Self::empty()
    }

    fn from_env() -> Result<Self, Error> {
        Ok(Self::empty())
    }

    fn with_fallback(mut self, fallback: Self) -> Self {
        for (k, v) in fallback.0 {
            let v = match self.shift_remove(&k) {
                Some(value) => value.with_fallback(v),
                None => v,
            };
            self.insert(k, v);
        }
        self
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn is_complete(&self) -> bool {
        self.values().all(Partial::is_complete)
    }
}
