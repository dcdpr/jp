use core::fmt;
use std::{collections::HashMap, hash::Hash, marker::PhantomData};

use serde::{
    de::{DeserializeOwned, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};

#[derive(Debug)]
pub struct ConfigMap<K: DeserializeOwned + Eq + Hash, V: confique::Config> {
    pub inner: HashMap<K, V>,
}

impl<K: DeserializeOwned + Eq + Hash, V: confique::Config> std::ops::Deref for ConfigMap<K, V> {
    type Target = HashMap<K, V>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<K: DeserializeOwned + Eq + Hash, V: confique::Config> std::ops::DerefMut for ConfigMap<K, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct ConfigMapPartial<K: DeserializeOwned + Eq + Hash, V: confique::Partial> {
    pub inner: HashMap<K, V>,
}

impl<K: DeserializeOwned + Eq + Hash + Clone, V: confique::Config + Clone> Clone
    for ConfigMap<K, V>
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<K: DeserializeOwned + Eq + Hash, V: confique::Config> Default for ConfigMap<K, V> {
    fn default() -> Self {
        Self {
            inner: HashMap::default(),
        }
    }
}

impl<K: DeserializeOwned + Eq + Hash + PartialEq, V: confique::Config + PartialEq> PartialEq
    for ConfigMap<K, V>
{
    fn eq(&self, other: &Self) -> bool {
        self.inner.eq(&other.inner)
    }
}

impl<K: DeserializeOwned + Eq + Hash + Serialize, V: confique::Config + Serialize> Serialize
    for ConfigMap<K, V>
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.inner.serialize(serializer)
    }
}

struct ConfigMapVisitor<K: DeserializeOwned + Eq + Hash, V: confique::Config> {
    marker: PhantomData<fn() -> ConfigMap<K, V>>,
}

impl<K: DeserializeOwned + Eq + Hash, V: confique::Config> ConfigMapVisitor<K, V> {
    fn new() -> Self {
        ConfigMapVisitor {
            marker: PhantomData,
        }
    }
}

impl<'de, K: DeserializeOwned + Eq + Hash, V: confique::Config> Visitor<'de>
    for ConfigMapVisitor<K, V>
where
    V: Deserialize<'de>,
{
    type Value = ConfigMap<K, V>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a config map")
    }

    // Deserialize MyMap from an abstract "map" provided by the
    // Deserializer. The MapAccess input is a callback provided by
    // the Deserializer to let us see each entry in the map.
    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut map = ConfigMap::default();
        while let Some((key, value)) = access.next_entry()? {
            map.inner.insert(key, value);
        }

        Ok(map)
    }
}

// This is the trait that informs Serde how to deserialize MyMap.
impl<'de, K: DeserializeOwned + Eq + Hash, V: confique::Config> Deserialize<'de> for ConfigMap<K, V>
where
    V: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Instantiate our Visitor and ask the Deserializer to drive
        // it over the input data, resulting in an instance of MyMap.
        deserializer.deserialize_map(ConfigMapVisitor::new())
    }
}

impl<K: DeserializeOwned + Eq + Hash, V: confique::Config> confique::Config for ConfigMap<K, V> {
    type Partial = ConfigMapPartial<K, V::Partial>;

    // TODO
    const META: confique::meta::Meta = confique::meta::Meta {
        name: "",
        doc: &[],
        fields: &[],
    };

    fn from_partial(partial: Self::Partial) -> Result<Self, confique::Error> {
        // TODO: this needs to use `confique::internal::map_err_prefix_path` to give the correct path in errors
        let inner: Result<_, confique::Error> = partial
            .inner
            .into_iter()
            .map(|(k, v)| Ok((k, V::from_partial(v)?)))
            .collect();
        Ok(Self { inner: inner? })
    }
}

impl<'de, K: DeserializeOwned + Eq + Hash, V: confique::Partial> serde::Deserialize<'de>
    for ConfigMapPartial<K, V>
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        Ok(Self {
            inner: HashMap::deserialize(deserializer)?,
        })
    }
}

impl<K: DeserializeOwned + Eq + Hash, V: confique::Partial> confique::Partial
    for ConfigMapPartial<K, V>
{
    fn empty() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    fn default_values() -> Self {
        Self::empty()
    }

    fn from_env() -> Result<Self, confique::Error> {
        Ok(Self::empty())
    }

    fn with_fallback(mut self, fallback: Self) -> Self {
        for (k, v) in fallback.inner {
            let v = match self.inner.remove(&k) {
                Some(value) => value.with_fallback(v),
                None => v,
            };
            self.inner.insert(k, v);
        }
        self
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn is_complete(&self) -> bool {
        self.inner.values().all(confique::Partial::is_complete)
    }
}

impl<K: DeserializeOwned + Eq + Hash + Clone, V: confique::Partial + Clone> Clone
    for ConfigMapPartial<K, V>
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
