use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet, TryReserveError, hash_map},
    fmt::{self, Debug},
    hash::{BuildHasher, Hash, RandomState},
    ops::Index,
    ptr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use self::Entry::*;

pub struct TombMap<K, V, S = RandomState> {
    live: HashMap<K, V, S>,
    dead: HashSet<K>,
    modified: HashSet<K>,
}

impl<K, V, S> Serialize for TombMap<K, V, S>
where
    HashMap<K, V, S>: Serialize,
{
    fn serialize<Ser>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: Serializer,
    {
        self.live.serialize(ser)
    }
}

impl<'de, K, V, S> Deserialize<'de> for TombMap<K, V, S>
where
    HashMap<K, V, S>: Deserialize<'de>,
    S: BuildHasher + Default,
{
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(TombMap {
            live: HashMap::<K, V, S>::deserialize(de)?,
            ..Default::default()
        })
    }
}

impl<K, V> TombMap<K, V, RandomState> {
    /// Creates an empty `HashMap`.
    ///
    /// The hash map is initially created with a capacity of 0, so it will not allocate until it
    /// is first inserted into.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// let mut map: HashMap<&str, i32> = HashMap::new();
    /// ```
    #[inline]
    #[must_use]
    pub fn new() -> TombMap<K, V, RandomState> {
        TombMap::default()
    }

    /// Creates an empty `HashMap` with at least the specified capacity.
    ///
    /// The hash map will be able to hold at least `capacity` elements without
    /// reallocating. This method is allowed to allocate for more elements than
    /// `capacity`. If `capacity` is zero, the hash map will not allocate.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// let mut map: HashMap<&str, i32> = HashMap::with_capacity(10);
    /// ```
    #[inline]
    #[must_use]
    pub fn with_capacity(capacity: usize) -> TombMap<K, V, RandomState> {
        TombMap::with_capacity_and_hasher(capacity, RandomState::default())
    }
}

impl<K, V, S> TombMap<K, V, S> {
    /// Creates an empty `HashMap` which will use the given hash builder to hash
    /// keys.
    ///
    /// The created map has the default initial capacity.
    ///
    /// Warning: `hash_builder` is normally randomly generated, and
    /// is designed to allow `HashMaps` to be resistant to attacks that
    /// cause many collisions and very poor performance. Setting it
    /// manually using this function can expose a `DoS` attack vector.
    ///
    /// The `hash_builder` passed should implement the [`BuildHasher`] trait for
    /// the `HashMap` to be useful, see its documentation for details.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::{collections::HashMap, hash::RandomState};
    ///
    /// let s = RandomState::new();
    /// let mut map = HashMap::with_hasher(s);
    /// map.insert(1, 2);
    /// ```
    #[inline]
    pub fn with_hasher(hash_builder: S) -> TombMap<K, V, S> {
        TombMap {
            live: HashMap::with_hasher(hash_builder),
            dead: HashSet::new(),
            modified: HashSet::new(),
        }
    }

    /// Creates an empty `HashMap` with at least the specified capacity, using
    /// `hasher` to hash the keys.
    ///
    /// The hash map will be able to hold at least `capacity` elements without
    /// reallocating. This method is allowed to allocate for more elements than
    /// `capacity`. If `capacity` is zero, the hash map will not allocate.
    ///
    /// Warning: `hasher` is normally randomly generated, and
    /// is designed to allow `HashMaps` to be resistant to attacks that
    /// cause many collisions and very poor performance. Setting it
    /// manually using this function can expose a `DoS` attack vector.
    ///
    /// The `hasher` passed should implement the [`BuildHasher`] trait for
    /// the `HashMap` to be useful, see its documentation for details.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::{collections::HashMap, hash::RandomState};
    ///
    /// let s = RandomState::new();
    /// let mut map = HashMap::with_capacity_and_hasher(10, s);
    /// map.insert(1, 2);
    /// ```
    #[inline]
    pub fn with_capacity_and_hasher(capacity: usize, hasher: S) -> TombMap<K, V, S> {
        TombMap {
            live: HashMap::with_capacity_and_hasher(capacity, hasher),
            dead: HashSet::new(),
            modified: HashSet::new(),
        }
    }

    /// Returns the number of elements the map can hold without reallocating.
    ///
    /// This number is a lower bound; the `HashMap<K, V>` might be able to hold
    /// more, but is guaranteed to be able to hold at least this many.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// let map: HashMap<i32, i32> = HashMap::with_capacity(100);
    /// assert!(map.capacity() >= 100);
    /// ```
    #[inline]
    pub fn capacity(&self) -> usize {
        self.live.capacity()
    }

    /// An iterator visiting all keys in arbitrary order.
    /// The iterator element type is `&'a K`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let map = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
    ///
    /// for key in map.keys() {
    ///     println!("{key}");
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// In the current implementation, iterating over keys takes O(capacity) time
    /// instead of O(len) because it internally visits empty buckets too.
    pub fn keys(&self) -> hash_map::Keys<'_, K, V> {
        self.live.keys()
    }

    /// Creates a consuming iterator visiting all the keys in arbitrary order.
    /// The map cannot be used after calling this.
    /// The iterator element type is `K`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let map = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
    ///
    /// let mut vec: Vec<&str> = map.into_keys().collect();
    /// // The `IntoKeys` iterator produces keys in arbitrary order, so the
    /// // keys must be sorted to test them against a sorted array.
    /// vec.sort_unstable();
    /// assert_eq!(vec, ["a", "b", "c"]);
    /// ```
    ///
    /// # Performance
    ///
    /// In the current implementation, iterating over keys takes O(capacity) time
    /// instead of O(len) because it internally visits empty buckets too.
    #[inline]
    pub fn into_keys(self) -> hash_map::IntoKeys<K, V> {
        self.live.into_keys()
    }

    /// An iterator visiting all values in arbitrary order.
    /// The iterator element type is `&'a V`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let map = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
    ///
    /// for val in map.values() {
    ///     println!("{val}");
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// In the current implementation, iterating over values takes O(capacity) time
    /// instead of O(len) because it internally visits empty buckets too.
    pub fn values(&self) -> hash_map::Values<'_, K, V> {
        self.live.values()
    }

    /// An iterator visiting all values mutably in arbitrary order.
    /// The iterator element type is `&'a mut V`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
    ///
    /// for val in map.values_mut() {
    ///     *val = *val + 10;
    /// }
    ///
    /// for val in map.values() {
    ///     println!("{val}");
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// In the current implementation, iterating over values takes O(capacity) time
    /// instead of O(len) because it internally visits empty buckets too.
    pub fn values_mut(&mut self) -> hash_map::ValuesMut<'_, K, V> {
        self.live.values_mut()
    }

    /// Creates a consuming iterator visiting all the values in arbitrary order.
    /// The map cannot be used after calling this.
    /// The iterator element type is `V`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let map = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
    ///
    /// let mut vec: Vec<i32> = map.into_values().collect();
    /// // The `IntoValues` iterator produces values in arbitrary order, so
    /// // the values must be sorted to test them against a sorted array.
    /// vec.sort_unstable();
    /// assert_eq!(vec, [1, 2, 3]);
    /// ```
    ///
    /// # Performance
    ///
    /// In the current implementation, iterating over values takes O(capacity) time
    /// instead of O(len) because it internally visits empty buckets too.
    #[inline]
    pub fn into_values(self) -> hash_map::IntoValues<K, V> {
        self.live.into_values()
    }

    /// An iterator visiting all key-value pairs in arbitrary order.
    /// The iterator element type is `(&'a K, &'a V)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let map = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
    ///
    /// for (key, val) in map.iter() {
    ///     println!("key: {key} val: {val}");
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// In the current implementation, iterating over map takes O(capacity) time
    /// instead of O(len) because it internally visits empty buckets too.
    pub fn iter(&self) -> hash_map::Iter<'_, K, V> {
        self.live.iter()
    }

    /// An iterator visiting all key-value pairs in arbitrary order,
    /// with mutable references to the values.
    /// The iterator element type is `(&'a K, &'a mut V)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
    ///
    /// // Update all values
    /// for (_, val) in map.iter_mut() {
    ///     *val *= 2;
    /// }
    ///
    /// for (key, val) in &map {
    ///     println!("key: {key} val: {val}");
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// In the current implementation, iterating over map takes O(capacity) time
    /// instead of O(len) because it internally visits empty buckets too.
    ///
    /// # Panics
    ///
    /// This function is not yet implemented and panics at runtime.
    pub fn iter_mut(&mut self) -> hash_map::IterMut<'_, K, V> {
        // FIXME: This does **NOT** have change detection.
        //
        // We will need to return our own `IterMut` type, which returns a custom
        // `Mut<&mut V>` type, which then implements `DerefMut` to return a
        // `&mut V`.
        //
        // self.live.iter_mut()
        panic!("`iter_mut` is not yet implemented. Use `iter_mut_untracked` instead.");
    }

    /// Returns the number of elements in the map.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut a = HashMap::new();
    /// assert_eq!(a.len(), 0);
    /// a.insert(1, "a");
    /// assert_eq!(a.len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Returns `true` if the map contains no elements.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut a = HashMap::new();
    /// assert!(a.is_empty());
    /// a.insert(1, "a");
    /// assert!(!a.is_empty());
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Returns a reference to the map's [`BuildHasher`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::{collections::HashMap, hash::RandomState};
    ///
    /// let hasher = RandomState::new();
    /// let map: HashMap<i32, i32> = HashMap::with_hasher(hasher);
    /// let hasher: &RandomState = map.hasher();
    /// ```
    #[inline]
    pub fn hasher(&self) -> &S {
        self.live.hasher()
    }

    /// Returns an iterator over the keys that have been removed from the map.
    pub fn removed_keys(&self) -> impl Iterator<Item = &K> {
        self.dead.iter()
    }

    /// Returns an iterator over the keys that have been modified since
    /// insertion.
    pub fn modified_keys(&self) -> impl Iterator<Item = &K> {
        self.modified.iter()
    }

    /// Returns true if the key has been modified since insertion.
    pub fn is_modified(&self, k: &K) -> bool
    where
        K: Eq + Hash,
    {
        self.modified.contains(k)
    }

    /// Returns an iterator over the modified key/value pairs.
    pub fn iter_modified(&self) -> impl Iterator<Item = (&K, &V)>
    where
        K: Eq + Hash,
    {
        self.live.iter().filter(|(k, _)| self.modified.contains(k))
    }

    /// This is a (temporary) workaround for the fact that `iter_mut` does not
    /// do change detection for individual elements in the mutable iterator.
    ///
    /// Sometimes that is okay, in which case you can use this specialized
    /// method.
    pub fn iter_mut_untracked(&mut self) -> impl Iterator<Item = (&K, &mut V)>
    where
        K: Eq + Hash,
    {
        self.live.iter_mut()
    }
}

impl<K, V, S> TombMap<K, V, S>
where
    K: Clone + Eq + Hash,
{
    /// Retains only the elements specified by the predicate.
    ///
    /// In other words, remove all pairs `(k, v)` for which `f(&k, &mut v)` returns `false`.
    /// The elements are visited in unsorted (and unspecified) order.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<i32, i32> = (0..8).map(|x| (x, x * 10)).collect();
    /// map.retain(|&k, _| k % 2 == 0);
    /// assert_eq!(map.len(), 4);
    /// ```
    ///
    /// # Performance
    ///
    /// In the current implementation, this operation takes O(capacity) time
    /// instead of O(len) because it internally visits empty buckets too.
    #[inline]
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        self.live.retain(|k, v| {
            let keep = f(k, v);
            if !keep {
                self.modified.remove(k);
                self.dead.insert(k.clone());
            }
            keep
        });
    }

    /// Clears the map, returning all key-value pairs as an iterator. Keeps the
    /// allocated memory for reuse.
    ///
    /// If the returned iterator is dropped before being fully consumed, it
    /// drops the remaining key-value pairs. The returned iterator keeps a
    /// mutable borrow on the map to optimize its implementation.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut a = HashMap::new();
    /// a.insert(1, "a");
    /// a.insert(2, "b");
    ///
    /// for (k, v) in a.drain().take(1) {
    ///     assert!(k == 1 || k == 2);
    ///     assert!(v == "a" || v == "b");
    /// }
    ///
    /// assert!(a.is_empty());
    /// ```
    #[inline]
    pub fn drain(&mut self) -> hash_map::Drain<'_, K, V> {
        for k in self.live.keys() {
            self.dead.insert(k.clone());
            self.modified.remove(k);
        }

        self.live.drain()
    }
}

impl<K, V, S> TombMap<K, V, S>
where
    K: Eq + Hash,
{
    /// Clears the map, removing all key-value pairs.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut a = HashMap::new();
    /// a.insert(1, "a");
    /// a.clear();
    /// assert!(a.is_empty());
    /// ```
    #[inline]
    pub fn clear(&mut self) {
        for (k, _) in self.live.drain() {
            self.dead.insert(k);
        }
        self.modified.clear();
    }
}

impl<K, V, S> TombMap<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    /// Reserves capacity for at least `additional` more elements to be inserted
    /// in the `HashMap`. The collection may reserve more space to speculatively
    /// avoid frequent reallocations. After calling `reserve`,
    /// capacity will be greater than or equal to `self.len() + additional`.
    /// Does nothing if capacity is already sufficient.
    ///
    /// # Panics
    ///
    /// Panics if the new allocation size overflows [`usize`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// let mut map: HashMap<&str, i32> = HashMap::new();
    /// map.reserve(10);
    /// ```
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.live.reserve(additional);
    }

    /// Tries to reserve capacity for at least `additional` more elements to be inserted
    /// in the `HashMap`. The collection may reserve more space to speculatively
    /// avoid frequent reallocations. After calling `try_reserve`,
    /// capacity will be greater than or equal to `self.len() + additional` if
    /// it returns `Ok(())`.
    /// Does nothing if capacity is already sufficient.
    ///
    /// # Errors
    ///
    /// If the capacity overflows, or the allocator reports a failure, then an error
    /// is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<&str, isize> = HashMap::new();
    /// map.try_reserve(10)
    ///     .expect("why is the test harness OOMing on a handful of bytes?");
    /// ```
    #[inline]
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), TryReserveError> {
        self.live.try_reserve(additional)
    }

    /// Shrinks the capacity of the map as much as possible. It will drop
    /// down as much as possible while maintaining the internal rules
    /// and possibly leaving some space in accordance with the resize policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<i32, i32> = HashMap::with_capacity(100);
    /// map.insert(1, 2);
    /// map.insert(3, 4);
    /// assert!(map.capacity() >= 100);
    /// map.shrink_to_fit();
    /// assert!(map.capacity() >= 2);
    /// ```
    #[inline]
    pub fn shrink_to_fit(&mut self) {
        self.live.shrink_to_fit();
    }

    /// Shrinks the capacity of the map with a lower limit. It will drop
    /// down no lower than the supplied limit while maintaining the internal rules
    /// and possibly leaving some space in accordance with the resize policy.
    ///
    /// If the current capacity is less than the lower limit, this is a no-op.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<i32, i32> = HashMap::with_capacity(100);
    /// map.insert(1, 2);
    /// map.insert(3, 4);
    /// assert!(map.capacity() >= 100);
    /// map.shrink_to(10);
    /// assert!(map.capacity() >= 10);
    /// map.shrink_to(0);
    /// assert!(map.capacity() >= 2);
    /// ```
    #[inline]
    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.live.shrink_to(min_capacity);
    }

    /// Gets the given key's corresponding entry in the map for in-place manipulation.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut letters = HashMap::new();
    ///
    /// for ch in "a short treatise on fungi".chars() {
    ///     letters
    ///         .entry(ch)
    ///         .and_modify(|counter| *counter += 1)
    ///         .or_insert(1);
    /// }
    ///
    /// assert_eq!(letters[&'s'], 2);
    /// assert_eq!(letters[&'t'], 3);
    /// assert_eq!(letters[&'u'], 1);
    /// assert_eq!(letters.get(&'y'), None);
    /// ```
    #[inline]
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        let dead = ptr::NonNull::from(&mut self.dead);
        let modified = ptr::NonNull::from(&mut self.modified);
        map_entry(self.live.entry(key), dead, modified)
    }

    /// Returns a reference to the value corresponding to the key.
    ///
    /// The key may be any borrowed form of the map's key type, but
    /// [`Hash`] and [`Eq`] on the borrowed form *must* match those for
    /// the key type.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map = HashMap::new();
    /// map.insert(1, "a");
    /// assert_eq!(map.get(&1), Some(&"a"));
    /// assert_eq!(map.get(&2), None);
    /// ```
    #[inline]
    pub fn get<Q>(&self, k: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.live.get(k)
    }

    /// Returns the key-value pair corresponding to the supplied key. This is
    /// potentially useful:
    /// - for key types where non-identical keys can be considered equal;
    /// - for getting the `&K` stored key value from a borrowed `&Q` lookup key; or
    /// - for getting a reference to a key with the same lifetime as the collection.
    ///
    /// The supplied key may be any borrowed form of the map's key type, but
    /// [`Hash`] and [`Eq`] on the borrowed form *must* match those for
    /// the key type.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::{
    ///     collections::HashMap,
    ///     hash::{Hash, Hasher},
    /// };
    ///
    /// #[derive(Clone, Copy, Debug)]
    /// struct S {
    ///     id: u32,
    /// #   #[allow(unused)] // prevents a "field `name` is never read" error
    ///     name: &'static str, // ignored by equality and hashing operations
    /// }
    ///
    /// impl PartialEq for S {
    ///     fn eq(&self, other: &S) -> bool {
    ///         self.id == other.id
    ///     }
    /// }
    ///
    /// impl Eq for S {}
    ///
    /// impl Hash for S {
    ///     fn hash<H: Hasher>(&self, state: &mut H) {
    ///         self.id.hash(state);
    ///     }
    /// }
    ///
    /// let j_a = S {
    ///     id: 1,
    ///     name: "Jessica",
    /// };
    /// let j_b = S {
    ///     id: 1,
    ///     name: "Jess",
    /// };
    /// let p = S {
    ///     id: 2,
    ///     name: "Paul",
    /// };
    /// assert_eq!(j_a, j_b);
    ///
    /// let mut map = HashMap::new();
    /// map.insert(j_a, "Paris");
    /// assert_eq!(map.get_key_value(&j_a), Some((&j_a, &"Paris")));
    /// assert_eq!(map.get_key_value(&j_b), Some((&j_a, &"Paris"))); // the notable case
    /// assert_eq!(map.get_key_value(&p), None);
    /// ```
    #[inline]
    pub fn get_key_value<Q>(&self, k: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.live.get_key_value(k)
    }

    /// Attempts to get mutable references to `N` values in the map at once.
    ///
    /// Returns an array of length `N` with the results of each query. For soundness, at most one
    /// mutable reference will be returned to any value. `None` will be used if the key is missing.
    ///
    /// # Panics
    ///
    /// Panics if any keys are overlapping.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut libraries = HashMap::new();
    /// libraries.insert("Bodleian Library".to_string(), 1602);
    /// libraries.insert("Athenæum".to_string(), 1807);
    /// libraries.insert("Herzogin-Anna-Amalia-Bibliothek".to_string(), 1691);
    /// libraries.insert("Library of Congress".to_string(), 1800);
    ///
    /// // Get Athenæum and Bodleian Library
    /// let [Some(a), Some(b)] = libraries.get_disjoint_mut(["Athenæum", "Bodleian Library"]) else {
    ///     panic!()
    /// };
    ///
    /// // Assert values of Athenæum and Library of Congress
    /// let got = libraries.get_disjoint_mut(["Athenæum", "Library of Congress"]);
    /// assert_eq!(got, [Some(&mut 1807), Some(&mut 1800),],);
    ///
    /// // Missing keys result in None
    /// let got = libraries.get_disjoint_mut(["Athenæum", "New York Public Library"]);
    /// assert_eq!(got, [Some(&mut 1807), None]);
    /// ```
    ///
    /// ```should_panic
    /// use std::collections::HashMap;
    ///
    /// let mut libraries = HashMap::new();
    /// libraries.insert("Athenæum".to_string(), 1807);
    ///
    /// // Duplicate keys panic!
    /// let got = libraries.get_disjoint_mut(["Athenæum", "Athenæum"]);
    /// ```
    #[inline]
    #[doc(alias = "get_many_mut")]
    pub fn get_disjoint_mut<Q, const N: usize>(&mut self, ks: [&Q; N]) -> [Option<&'_ mut V>; N]
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.live.get_disjoint_mut(ks)
    }

    /// Attempts to get mutable references to `N` values in the map at once, without validating that
    /// the values are unique.
    ///
    /// Returns an array of length `N` with the results of each query. `None` will be used if
    /// the key is missing.
    ///
    /// For a safe alternative see [`get_disjoint_mut`](`HashMap::get_disjoint_mut`).
    ///
    /// # Safety
    ///
    /// Calling this method with overlapping keys is *[undefined behavior]* even if the resulting
    /// references are not used.
    ///
    /// [undefined behavior]: https://doc.rust-lang.org/reference/behavior-considered-undefined.html
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut libraries = HashMap::new();
    /// libraries.insert("Bodleian Library".to_string(), 1602);
    /// libraries.insert("Athenæum".to_string(), 1807);
    /// libraries.insert("Herzogin-Anna-Amalia-Bibliothek".to_string(), 1691);
    /// libraries.insert("Library of Congress".to_string(), 1800);
    ///
    /// // SAFETY: The keys do not overlap.
    /// let [Some(a), Some(b)] =
    ///     (unsafe { libraries.get_disjoint_unchecked_mut(["Athenæum", "Bodleian Library"]) })
    /// else {
    ///     panic!()
    /// };
    ///
    /// // SAFETY: The keys do not overlap.
    /// let got = unsafe { libraries.get_disjoint_unchecked_mut(["Athenæum", "Library of Congress"]) };
    /// assert_eq!(got, [Some(&mut 1807), Some(&mut 1800),],);
    ///
    /// // SAFETY: The keys do not overlap.
    /// let got =
    ///     unsafe { libraries.get_disjoint_unchecked_mut(["Athenæum", "New York Public Library"]) };
    /// // Missing keys result in None
    /// assert_eq!(got, [Some(&mut 1807), None]);
    /// ```
    #[inline]
    #[doc(alias = "get_many_unchecked_mut")]
    pub unsafe fn get_disjoint_unchecked_mut<Q, const N: usize>(
        &mut self,
        ks: [&Q; N],
    ) -> [Option<&'_ mut V>; N]
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        unsafe { self.live.get_disjoint_unchecked_mut(ks) }
    }

    /// Returns `true` if the map contains a value for the specified key.
    ///
    /// The key may be any borrowed form of the map's key type, but
    /// [`Hash`] and [`Eq`] on the borrowed form *must* match those for
    /// the key type.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map = HashMap::new();
    /// map.insert(1, "a");
    /// assert_eq!(map.contains_key(&1), true);
    /// assert_eq!(map.contains_key(&2), false);
    /// ```
    #[inline]
    pub fn contains_key<Q>(&self, k: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.live.contains_key(k)
    }

    /// Returns a mutable reference to the value corresponding to the key.
    ///
    /// The key may be any borrowed form of the map's key type, but
    /// [`Hash`] and [`Eq`] on the borrowed form *must* match those for
    /// the key type.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map = HashMap::new();
    /// map.insert(1, "a");
    /// if let Some(x) = map.get_mut(&1) {
    ///     *x = "b";
    /// }
    /// assert_eq!(map[&1], "b");
    /// ```
    #[inline]
    pub fn get_mut<Q>(&mut self, k: &Q) -> Option<&mut V>
    where
        K: Borrow<Q> + Clone,
        Q: Hash + Eq + ?Sized,
    {
        if let Some((k, _)) = self.live.get_key_value(k) {
            self.modified.insert(k.clone());
        }

        self.live.get_mut(k)
    }

    /// Inserts a key-value pair into the map.
    ///
    /// If the map did not have this key present, [`None`] is returned.
    ///
    /// If the map did have this key present, the value is updated, and the old
    /// value is returned. The key is not updated, though; this matters for
    /// types that can be `==` without being identical. See the [module-level
    /// documentation] for more.
    ///
    /// [module-level documentation]: std::collections#insert-and-complex-keys
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map = HashMap::new();
    /// assert_eq!(map.insert(37, "a"), None);
    /// assert_eq!(map.is_empty(), false);
    ///
    /// map.insert(37, "b");
    /// assert_eq!(map.insert(37, "c"), Some("b"));
    /// assert_eq!(map[&37], "c");
    /// ```
    #[inline]
    pub fn insert(&mut self, k: K, v: V) -> Option<V>
    where
        K: Eq + Hash + Clone,
    {
        self.dead.remove(&k);
        match self.entry(k) {
            Entry::Occupied(mut entry) => Some(entry.insert(v)),
            Entry::Vacant(entry) => {
                entry.insert(v);
                None
            }
        }
    }

    /// Removes a key from the map, returning the value at the key if the key
    /// was previously in the map.
    ///
    /// The key may be any borrowed form of the map's key type, but
    /// [`Hash`] and [`Eq`] on the borrowed form *must* match those for
    /// the key type.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map = HashMap::new();
    /// map.insert(1, "a");
    /// assert_eq!(map.remove(&1), Some("a"));
    /// assert_eq!(map.remove(&1), None);
    /// ```
    #[inline]
    pub fn remove<Q>(&mut self, k: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.modified.remove(k);
        let (k, v) = self.live.remove_entry(k)?;
        self.dead.insert(k);
        Some(v)
    }

    /// Removes a key from the map, returning the value at the key if the key
    /// was previously in the map.
    ///
    /// As opposed to [`TombMap::remove`], this method does not mark the key as
    /// removed. It *does* unmark the key as modified, since the key no longer
    /// exists.
    #[inline]
    pub fn remove_untracked<Q>(&mut self, k: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.modified.remove(k);
        self.live.remove(k)
    }

    /// Removes a key from the map, returning the stored key and value if the
    /// key was previously in the map.
    ///
    /// The key may be any borrowed form of the map's key type, but
    /// [`Hash`] and [`Eq`] on the borrowed form *must* match those for
    /// the key type.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// # fn main() {
    /// let mut map = HashMap::new();
    /// map.insert(1, "a");
    /// assert_eq!(map.remove_entry(&1), Some((1, "a")));
    /// assert_eq!(map.remove(&1), None);
    /// # }
    /// ```
    #[inline]
    pub fn remove_entry<Q>(&mut self, k: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q> + Clone,
        Q: Hash + Eq + ?Sized,
    {
        self.modified.remove(k);
        let (k, v) = self.live.remove_entry(k)?;
        self.dead.insert(k.clone());
        Some((k, v))
    }
}

impl<K, V, S> Clone for TombMap<K, V, S>
where
    K: Clone,
    V: Clone,
    S: Clone,
{
    #[inline]
    fn clone(&self) -> Self {
        Self {
            live: self.live.clone(),
            dead: self.dead.clone(),
            modified: self.modified.clone(),
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.live.clone_from(&source.live);
    }
}

impl<K, V, S> PartialEq for TombMap<K, V, S>
where
    K: Eq + Hash,
    V: PartialEq,
    S: BuildHasher,
{
    fn eq(&self, other: &TombMap<K, V, S>) -> bool {
        if self.len() != other.len() {
            return false;
        }

        self.iter()
            .all(|(key, value)| other.get(key).is_some_and(|v| *value == *v))
    }
}

impl<K, V, S> Eq for TombMap<K, V, S>
where
    K: Eq + Hash,
    V: Eq,
    S: BuildHasher,
{
}

impl<K, V, S> Debug for TombMap<K, V, S>
where
    K: Debug,
    V: Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V, S> Default for TombMap<K, V, S>
where
    S: Default,
{
    /// Creates an empty `HashMap<K, V, S>`, with the `Default` value for the hasher.
    #[inline]
    fn default() -> TombMap<K, V, S> {
        TombMap::with_hasher(Default::default())
    }
}

impl<K, Q: ?Sized, V, S> Index<&Q> for TombMap<K, V, S>
where
    K: Eq + Hash + Borrow<Q>,
    Q: Eq + Hash,
    S: BuildHasher,
{
    type Output = V;

    /// Returns a reference to the value corresponding to the supplied key.
    ///
    /// # Panics
    ///
    /// Panics if the key is not present in the `HashMap`.
    #[inline]
    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("no entry found for key")
    }
}

// Note: as what is currently the most convenient built-in way to construct
// a HashMap, a simple usage of this function must not *require* the user
// to provide a type annotation in order to infer the third type parameter
// (the hasher parameter, conventionally "S").
// To that end, this impl is defined using RandomState as the concrete
// type of S, rather than being generic over `S: BuildHasher + Default`.
// It is expected that users who want to specify a hasher will manually use
// `with_capacity_and_hasher`.
// If type parameter defaults worked on impls, and if type parameter
// defaults could be mixed with const generics, then perhaps
// this could be generalized.
// See also the equivalent impl on HashSet.
impl<K, V, const N: usize> From<[(K, V); N]> for TombMap<K, V, RandomState>
where
    K: Eq + Hash + Clone,
{
    /// Converts a `[(K, V); N]` into a `HashMap<K, V>`.
    ///
    /// If any entries in the array have equal keys,
    /// all but one of the corresponding values will be dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let map1 = HashMap::from([(1, 2), (3, 4)]);
    /// let map2: HashMap<_, _> = [(1, 2), (3, 4)].into();
    /// assert_eq!(map1, map2);
    /// ```
    fn from(arr: [(K, V); N]) -> Self {
        Self::from_iter(arr)
    }
}

/// A view into a single entry in a map, which may either be vacant or occupied.
///
/// This `enum` is constructed from the [`entry`] method on [`HashMap`].
///
/// [`entry`]: HashMap::entry
pub enum Entry<'a, K, V> {
    /// An occupied entry.
    Occupied(OccupiedEntry<'a, K, V>),

    /// A vacant entry.
    Vacant(VacantEntry<'a, K, V>),
}

impl<K: Debug, V: Debug> Debug for Entry<'_, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Vacant(ref v) => f.debug_tuple("Entry").field(v).finish(),
            Occupied(ref o) => f.debug_tuple("Entry").field(o).finish(),
        }
    }
}

/// A view into an occupied entry in a `HashMap`.
/// It is part of the [`Entry`] enum.
pub struct OccupiedEntry<'a, K, V> {
    base: hash_map::OccupiedEntry<'a, K, V>,
    dead: ptr::NonNull<HashSet<K>>,
    modified: ptr::NonNull<HashSet<K>>,
}

impl<K: Debug, V: Debug> Debug for OccupiedEntry<'_, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OccupiedEntry")
            .field("key", self.key())
            .field("value", self.get())
            .finish_non_exhaustive()
    }
}

/// A view into a vacant entry in a `HashMap`.
/// It is part of the [`Entry`] enum.
pub struct VacantEntry<'a, K, V> {
    base: hash_map::VacantEntry<'a, K, V>,
    dead: ptr::NonNull<HashSet<K>>,
    modified: ptr::NonNull<HashSet<K>>,
}

impl<K: Debug, V> Debug for VacantEntry<'_, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VacantEntry").field(self.key()).finish()
    }
}

impl<'a, K, V, S> IntoIterator for &'a TombMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = hash_map::Iter<'a, K, V>;

    #[inline]
    fn into_iter(self) -> hash_map::Iter<'a, K, V> {
        self.iter()
    }
}

impl<'a, K, V, S> IntoIterator for &'a mut TombMap<K, V, S> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = hash_map::IterMut<'a, K, V>;

    #[inline]
    fn into_iter(self) -> hash_map::IterMut<'a, K, V> {
        panic!("`iter_mut` is not yet implemented. Use `iter_mut_untracked` instead.");
    }
}

impl<K, V, S> IntoIterator for TombMap<K, V, S> {
    type Item = (K, V);
    type IntoIter = hash_map::IntoIter<K, V>;

    /// Creates a consuming iterator, that is, one that moves each key-value
    /// pair out of the map in arbitrary order. The map cannot be used after
    /// calling this.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let map = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
    ///
    /// // Not possible with .iter()
    /// let vec: Vec<(&str, i32)> = map.into_iter().collect();
    /// ```
    #[inline]
    fn into_iter(self) -> hash_map::IntoIter<K, V> {
        self.live.into_iter()
    }
}

impl<'a, K, V> Entry<'a, K, V> {
    /// Ensures a value is in the entry by inserting the default if empty, and returns
    /// a mutable reference to the value in the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    ///
    /// map.entry("poneyland").or_insert(3);
    /// assert_eq!(map["poneyland"], 3);
    ///
    /// *map.entry("poneyland").or_insert(10) *= 2;
    /// assert_eq!(map["poneyland"], 6);
    /// ```
    #[inline]
    pub fn or_insert(self, default: V) -> &'a mut V
    where
        K: Eq + Hash + Clone,
    {
        match self {
            Occupied(entry) => entry.into_mut(),
            Vacant(entry) => entry.insert(default),
        }
    }

    /// Ensures a value is in the entry by inserting the result of the default function if empty,
    /// and returns a mutable reference to the value in the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map = HashMap::new();
    /// let value = "hoho";
    ///
    /// map.entry("poneyland").or_insert_with(|| value);
    ///
    /// assert_eq!(map["poneyland"], "hoho");
    /// ```
    #[inline]
    pub fn or_insert_with<F: FnOnce() -> V>(self, default: F) -> &'a mut V
    where
        K: Eq + Hash + Clone,
    {
        match self {
            Occupied(entry) => entry.into_mut(),
            Vacant(entry) => entry.insert(default()),
        }
    }

    /// Ensures a value is in the entry by inserting, if empty, the result of the default function.
    /// This method allows for generating key-derived values for insertion by providing the default
    /// function a reference to the key that was moved during the `.entry(key)` method call.
    ///
    /// The reference to the moved key is provided so that cloning or copying the key is
    /// unnecessary, unlike with `.or_insert_with(|| ... )`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<&str, usize> = HashMap::new();
    ///
    /// map.entry("poneyland")
    ///     .or_insert_with_key(|key| key.chars().count());
    ///
    /// assert_eq!(map["poneyland"], 9);
    /// ```
    #[inline]
    pub fn or_insert_with_key<F: FnOnce(&K) -> V>(self, default: F) -> &'a mut V
    where
        K: Eq + Hash + Clone,
    {
        match self {
            Occupied(entry) => entry.into_mut(),
            Vacant(entry) => {
                let value = default(entry.key());
                entry.insert(value)
            }
        }
    }

    /// Returns a reference to this entry's key.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    /// assert_eq!(map.entry("poneyland").key(), &"poneyland");
    /// ```
    #[inline]
    pub fn key(&self) -> &K {
        match *self {
            Occupied(ref entry) => entry.key(),
            Vacant(ref entry) => entry.key(),
        }
    }

    /// Provides in-place mutable access to an occupied entry before any
    /// potential inserts into the map.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    ///
    /// map.entry("poneyland").and_modify(|e| *e += 1).or_insert(42);
    /// assert_eq!(map["poneyland"], 42);
    ///
    /// map.entry("poneyland").and_modify(|e| *e += 1).or_insert(42);
    /// assert_eq!(map["poneyland"], 43);
    /// ```
    #[inline]
    #[must_use]
    pub fn and_modify<F>(self, f: F) -> Self
    where
        F: FnOnce(&mut V),
        K: Eq + Hash + Clone,
    {
        match self {
            Occupied(mut entry) => {
                // SAFETY: Pointer is valid for 'a
                let modified: &mut HashSet<K> = unsafe { entry.modified.as_mut() };
                modified.insert(entry.key().clone());

                f(entry.get_mut());
                Occupied(entry)
            }
            Vacant(entry) => Vacant(entry),
        }
    }

    /// Sets the value of the entry, and returns an `OccupiedEntry`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<&str, String> = HashMap::new();
    /// let entry = map.entry("poneyland").insert_entry("hoho".to_string());
    ///
    /// assert_eq!(entry.key(), &"poneyland");
    /// ```
    #[inline]
    pub fn insert_entry(self, value: V) -> OccupiedEntry<'a, K, V>
    where
        K: Eq + Hash + Clone,
    {
        match self {
            Occupied(mut entry) => {
                // SAFETY: Pointer is valid for 'a
                let modified: &mut HashSet<K> = unsafe { entry.modified.as_mut() };
                modified.insert(entry.key().clone());

                entry.insert(value);
                entry
            }
            Vacant(mut entry) => {
                // SAFETY: Pointer is valid for 'a
                let modified: &mut HashSet<K> = unsafe { entry.modified.as_mut() };
                modified.remove(entry.key());

                entry.insert_entry(value)
            }
        }
    }
}

impl<'a, K, V: Default> Entry<'a, K, V> {
    /// Ensures a value is in the entry by inserting the default value if empty,
    /// and returns a mutable reference to the value in the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<&str, Option<u32>> = HashMap::new();
    /// map.entry("poneyland").or_default();
    ///
    /// assert_eq!(map["poneyland"], None);
    /// # }
    /// ```
    #[inline]
    pub fn or_default(self) -> &'a mut V
    where
        K: Eq + Hash + Clone,
    {
        match self {
            Occupied(entry) => entry.into_mut(),
            Vacant(entry) => entry.insert(Default::default()),
        }
    }
}

impl<'a, K, V> OccupiedEntry<'a, K, V> {
    /// Gets a reference to the key in the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    /// map.entry("poneyland").or_insert(12);
    /// assert_eq!(map.entry("poneyland").key(), &"poneyland");
    /// ```
    #[inline]
    #[must_use]
    pub fn key(&self) -> &K {
        self.base.key()
    }

    /// Gets a reference to the value in the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{HashMap, hash_map::Entry};
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    /// map.entry("poneyland").or_insert(12);
    ///
    /// if let Entry::Occupied(o) = map.entry("poneyland") {
    ///     assert_eq!(o.get(), &12);
    /// }
    /// ```
    #[inline]
    #[must_use]
    pub fn get(&self) -> &V {
        self.base.get()
    }

    /// Gets a mutable reference to the value in the entry.
    ///
    /// If you need a reference to the `OccupiedEntry` which may outlive the
    /// destruction of the `Entry` value, see [`into_mut`].
    ///
    /// [`into_mut`]: Self::into_mut
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{HashMap, hash_map::Entry};
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    /// map.entry("poneyland").or_insert(12);
    ///
    /// assert_eq!(map["poneyland"], 12);
    /// if let Entry::Occupied(mut o) = map.entry("poneyland") {
    ///     *o.get_mut() += 10;
    ///     assert_eq!(*o.get(), 22);
    ///
    ///     // We can use the same Entry multiple times.
    ///     *o.get_mut() += 2;
    /// }
    ///
    /// assert_eq!(map["poneyland"], 24);
    /// ```
    #[inline]
    pub fn get_mut(&mut self) -> &mut V
    where
        K: Eq + Hash + Clone,
    {
        // SAFETY: Pointer is valid for 'a
        let modified: &mut HashSet<K> = unsafe { self.modified.as_mut() };
        modified.insert(self.key().clone());

        self.base.get_mut()
    }

    /// Converts the `OccupiedEntry` into a mutable reference to the value in the entry
    /// with a lifetime bound to the map itself.
    ///
    /// If you need multiple references to the `OccupiedEntry`, see [`get_mut`].
    ///
    /// [`get_mut`]: Self::get_mut
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{HashMap, hash_map::Entry};
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    /// map.entry("poneyland").or_insert(12);
    ///
    /// assert_eq!(map["poneyland"], 12);
    /// if let Entry::Occupied(o) = map.entry("poneyland") {
    ///     *o.into_mut() += 10;
    /// }
    ///
    /// assert_eq!(map["poneyland"], 22);
    /// ```
    #[inline]
    #[must_use]
    pub fn into_mut(mut self) -> &'a mut V
    where
        K: Eq + Hash + Clone,
    {
        // SAFETY: Pointer is valid for 'a
        let modified: &mut HashSet<K> = unsafe { self.modified.as_mut() };
        modified.insert(self.key().clone());

        self.base.into_mut()
    }

    /// Sets the value of the entry, and returns the entry's old value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{HashMap, hash_map::Entry};
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    /// map.entry("poneyland").or_insert(12);
    ///
    /// if let Entry::Occupied(mut o) = map.entry("poneyland") {
    ///     assert_eq!(o.insert(15), 12);
    /// }
    ///
    /// assert_eq!(map["poneyland"], 15);
    /// ```
    #[inline]
    pub fn insert(&mut self, value: V) -> V
    where
        K: Eq + Hash + Clone,
    {
        // SAFETY: Pointer is valid for 'a
        let modified: &mut HashSet<K> = unsafe { self.modified.as_mut() };
        modified.insert(self.key().clone());

        self.base.insert(value)
    }
}

impl<K, V> OccupiedEntry<'_, K, V>
where
    K: Clone + Eq + Hash,
{
    /// Take the ownership of the key and value from the map.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{HashMap, hash_map::Entry};
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    /// map.entry("poneyland").or_insert(12);
    ///
    /// if let Entry::Occupied(o) = map.entry("poneyland") {
    ///     // We delete the entry from the map.
    ///     o.remove_entry();
    /// }
    ///
    /// assert_eq!(map.contains_key("poneyland"), false);
    /// ```
    #[inline]
    #[must_use]
    pub fn remove_entry(mut self) -> (K, V) {
        // SAFETY: `dead` is guaranteed valid for '_ by TombMap::entry()
        let dead: &mut HashSet<K> = unsafe { self.dead.as_mut() };
        let modified: &mut HashSet<K> = unsafe { self.modified.as_mut() };

        let (k, v) = self.base.remove_entry();
        dead.insert(k.clone());
        modified.remove(&k);
        (k, v)
    }

    /// Takes the value out of the entry, and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{HashMap, hash_map::Entry};
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    /// map.entry("poneyland").or_insert(12);
    ///
    /// if let Entry::Occupied(o) = map.entry("poneyland") {
    ///     assert_eq!(o.remove(), 12);
    /// }
    ///
    /// assert_eq!(map.contains_key("poneyland"), false);
    /// ```
    #[inline]
    #[must_use]
    pub fn remove(self) -> V {
        self.remove_entry().1
    }
}

impl<'a, K: 'a, V: 'a> VacantEntry<'a, K, V> {
    /// Gets a reference to the key that would be used when inserting a value
    /// through the `VacantEntry`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    /// assert_eq!(map.entry("poneyland").key(), &"poneyland");
    /// ```
    #[inline]
    pub fn key(&self) -> &K {
        self.base.key()
    }

    /// Take ownership of the key.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{HashMap, hash_map::Entry};
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    ///
    /// if let Entry::Vacant(v) = map.entry("poneyland") {
    ///     v.into_key();
    /// }
    /// ```
    #[inline]
    pub fn into_key(self) -> K {
        self.base.into_key()
    }

    /// Sets the value of the entry with the `VacantEntry`'s key,
    /// and returns a mutable reference to it.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{HashMap, hash_map::Entry};
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    ///
    /// if let Entry::Vacant(o) = map.entry("poneyland") {
    ///     o.insert(37);
    /// }
    /// assert_eq!(map["poneyland"], 37);
    /// ```
    #[inline]
    pub fn insert(mut self, value: V) -> &'a mut V
    where
        K: Eq + Hash,
    {
        let modified: &mut HashSet<K> = unsafe { self.modified.as_mut() };
        modified.remove(self.key());

        self.base.insert(value)
    }

    /// Sets the value of the entry with the `VacantEntry`'s key,
    /// and returns an `OccupiedEntry`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::{HashMap, hash_map::Entry};
    ///
    /// let mut map: HashMap<&str, u32> = HashMap::new();
    ///
    /// if let Entry::Vacant(o) = map.entry("poneyland") {
    ///     o.insert_entry(37);
    /// }
    /// assert_eq!(map["poneyland"], 37);
    /// ```
    #[inline]
    pub fn insert_entry(self, value: V) -> OccupiedEntry<'a, K, V>
    where
        K: Eq + Hash,
    {
        let mut modified = self.modified;
        let modified_set: &mut HashSet<K> = unsafe { modified.as_mut() };
        modified_set.remove(self.key());

        let base = self.base.insert_entry(value);
        let dead = self.dead;
        OccupiedEntry {
            base,
            dead,
            modified,
        }
    }
}

impl<K, V, S> FromIterator<(K, V)> for TombMap<K, V, S>
where
    K: Eq + Hash + Clone,
    S: BuildHasher + Default,
{
    /// Constructs a `HashMap<K, V>` from an iterator of key-value pairs.
    ///
    /// If the iterator produces any pairs with equal keys,
    /// all but one of the corresponding values will be dropped.
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> TombMap<K, V, S> {
        let mut map = TombMap::with_hasher(Default::default());
        map.extend(iter);
        map
    }
}

/// Inserts all new key-values from the iterator and replaces values with existing
/// keys with new values returned from the iterator.
impl<K, V, S> Extend<(K, V)> for TombMap<K, V, S>
where
    K: Eq + Hash + Clone,
    S: BuildHasher,
{
    #[inline]
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<'a, K, V, S> Extend<(&'a K, &'a V)> for TombMap<K, V, S>
where
    K: Eq + Hash + Copy,
    V: Copy,
    S: BuildHasher,
{
    #[inline]
    fn extend<T: IntoIterator<Item = (&'a K, &'a V)>>(&mut self, iter: T) {
        for (k, v) in iter {
            self.insert(*k, *v);
        }
    }
}

#[inline]
fn map_entry<'a, K: 'a, V: 'a>(
    raw: hash_map::Entry<'a, K, V>,
    dead: ptr::NonNull<HashSet<K>>,
    modified: ptr::NonNull<HashSet<K>>,
) -> Entry<'a, K, V> {
    match raw {
        hash_map::Entry::Occupied(base) => Entry::Occupied(OccupiedEntry {
            base,
            dead,
            modified,
        }),
        hash_map::Entry::Vacant(base) => Entry::Vacant(VacantEntry {
            base,
            dead,
            modified,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        fmt::Debug,
        hash::Hash,
    };

    use test_log::test;

    use super::*;

    // Helper to compare HashMaps (standard library for expected state)
    fn assert_maps_equal<K, V>(actual: &HashMap<K, V>, expected: &HashMap<K, V>, context: &str)
    where
        K: Eq + Hash + Debug,
        V: Eq + Debug,
    {
        assert_eq!(actual.len(), expected.len(), "{context}");

        for (k, expected_v) in expected {
            match actual.get(k) {
                Some(actual_v) => assert_eq!(actual_v, expected_v, "{context}"),
                None => panic!("Expected key {k:?} missing in actual map - {context}"),
            }
        }

        for k in actual.keys() {
            assert!(expected.contains_key(k), "Unexpected key {k:?} found");
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Action {
        Insert(&'static str, i32),
        Remove(&'static str),
        GetMutIncrement(&'static str),
        AndModifyIncrement(&'static str),
        Clear,
        RetainOddValues,
        Drain,
    }

    #[derive(Debug, Clone)]
    struct ExpectedState {
        live: HashMap<&'static str, i32>,
        dead: HashSet<&'static str>,
        modified: HashSet<&'static str>,
    }

    impl ExpectedState {
        fn new(
            live: impl IntoIterator<Item = (&'static str, i32)>,
            dead: impl IntoIterator<Item = &'static str>,
            modified: impl IntoIterator<Item = &'static str>,
        ) -> Self {
            Self {
                live: live.into_iter().collect(),
                dead: dead.into_iter().collect(),
                modified: modified.into_iter().collect(),
            }
        }
    }

    #[test]
    fn tomb_map_operations_and_tracking() {
        let mut map: TombMap<&'static str, i32> = TombMap::new();

        let steps: Vec<(&'static str, Action, ExpectedState)> = vec![
            (
                "1. Initial Insert 'a'",
                Action::Insert("a", 1),
                ExpectedState::new([("a", 1)], [], []),
            ),
            (
                "2. Initial Insert 'b'",
                Action::Insert("b", 2),
                ExpectedState::new([("a", 1), ("b", 2)], [], []),
            ),
            (
                "3. Modify 'a' via Insert",
                Action::Insert("a", 10),
                ExpectedState::new([("a", 10), ("b", 2)], [], ["a"]),
            ),
            (
                "4. Modify 'b' via GetMutIncrement",
                Action::GetMutIncrement("b"),
                ExpectedState::new([("a", 10), ("b", 3)], [], ["a", "b"]),
            ),
            (
                "5. Modify 'a' again via AndModifyIncrement",
                Action::AndModifyIncrement("a"),
                ExpectedState::new([("a", 20), ("b", 3)], [], ["a", "b"]),
            ),
            (
                "6. Remove 'a'",
                Action::Remove("a"),
                ExpectedState::new([("b", 3)], ["a"], ["b"]),
            ),
            (
                "7. Remove non-existent 'c'",
                Action::Remove("c"),
                ExpectedState::new([("b", 3)], ["a"], ["b"]),
            ),
            (
                "8. Re-insert 'a'",
                Action::Insert("a", 100),
                ExpectedState::new([("a", 100), ("b", 3)], [], ["b"]),
            ),
            (
                "9. Modify re-inserted 'a'",
                Action::Insert("a", 101),
                ExpectedState::new([("a", 101), ("b", 3)], [], ["a", "b"]),
            ),
            (
                "10. Insert 'c'",
                Action::Insert("c", 30),
                ExpectedState::new([("a", 101), ("b", 3), ("c", 30)], [], ["a", "b"]),
            ),
            (
                "11. RetainOddValues",
                Action::RetainOddValues,
                ExpectedState::new([("a", 101), ("b", 3)], ["c"], ["a", "b"]),
            ),
            (
                "12. Clear",
                Action::Clear,
                ExpectedState::new([], ["a", "b", "c"], []),
            ),
            (
                "13. Insert 'x' after clear",
                Action::Insert("x", 5),
                ExpectedState::new([("x", 5)], ["a", "b", "c"], []),
            ),
            (
                "14. Modify 'x' after clear",
                Action::Insert("x", 50),
                ExpectedState::new([("x", 50)], ["a", "b", "c"], ["x"]),
            ),
            (
                "15. Drain",
                Action::Drain,
                ExpectedState::new([], ["a", "b", "c", "x"], []),
            ),
        ];

        for (i, (step_name, action, expected)) in steps.into_iter().enumerate() {
            let step_num = i + 1;
            let context = format!("Step {step_num} ('{step_name}'): Action: {action:?}");

            match action {
                Action::Insert(k, v) => {
                    map.insert(k, v);
                }
                Action::Remove(k) => {
                    map.remove(&k);
                }
                Action::GetMutIncrement(k) => {
                    if let Some(v) = map.get_mut(&k) {
                        *v += 1;
                    }
                }
                Action::AndModifyIncrement(k) => {
                    let _v = map.entry(k).and_modify(|v| *v += 10);
                }
                Action::Clear => {
                    map.clear();
                }
                Action::RetainOddValues => {
                    map.retain(|_k, v| *v % 2 != 0);
                }
                Action::Drain => {
                    let _drained: Vec<_> = map.drain().collect();
                }
            }

            assert_maps_equal(&map.live, &expected.live, &context);

            let actual_dead: HashSet<&'static str> = map.removed_keys().copied().collect();
            assert_eq!(actual_dead, expected.dead, "{context}");

            let actual_modified: HashSet<&'static str> = map.modified_keys().copied().collect();
            assert_eq!(actual_modified, expected.modified, "{context}");
        }
    }
}
