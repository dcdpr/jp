use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    hash::{BuildHasher, Hash},
};

use crate::config::MergeResult;

/// Discard both previous and next values and return [`None`].
pub fn discard<T, C>(_: T, _: T, _: &C) -> MergeResult<T> {
    Ok(None)
}

/// Always preserve the previous value over the next value.
pub fn preserve<T, C>(prev: T, _: T, _: &C) -> MergeResult<T> {
    Ok(Some(prev))
}

/// Always replace the previous value with the next value.
pub fn replace<T, C>(_: T, next: T, _: &C) -> MergeResult<T> {
    Ok(Some(next))
}

/// Append the items from the next vector to the end of the previous vector.
pub fn append_vec<T, C>(mut prev: Vec<T>, next: Vec<T>, _: &C) -> MergeResult<Vec<T>> {
    prev.extend(next);

    Ok(Some(prev))
}

/// Prepend the items from the next vector to the start of the previous vector.
pub fn prepend_vec<T, C>(prev: Vec<T>, next: Vec<T>, _: &C) -> MergeResult<Vec<T>> {
    let mut new = vec![];
    new.extend(next);
    new.extend(prev);

    Ok(Some(new))
}

/// Shallow merge the next [`BTreeMap`] into the previous [`BTreeMap`].
/// Any items in the next [`BTreeMap`] will overwrite items in the previous
/// [`BTreeMap`] of the same key.
#[deprecated(note = "Use `merge_iter` instead")]
pub fn merge_btreemap<K, V, C>(
    prev: BTreeMap<K, V>,
    next: BTreeMap<K, V>,
    c: &C,
) -> MergeResult<BTreeMap<K, V>>
where
    K: Eq + Hash + Ord,
{
    merge_iter(prev, next, c)
}

/// Shallow merge the next [`BTreeSet`] into the previous [`BTreeSet`],
/// overwriting duplicates.
#[deprecated(note = "Use `merge_iter` instead")]
pub fn merge_btreeset<T, C>(prev: BTreeSet<T>, next: BTreeSet<T>, c: &C) -> MergeResult<BTreeSet<T>>
where
    T: Eq + Hash + Ord,
{
    merge_iter(prev, next, c)
}

/// Shallow merge the next [`HashMap`] into the previous [`HashMap`].
/// Any items in the next [`HashMap`] will overwrite items in the previous
/// [`HashMap`] of the same key.
#[deprecated(note = "Use `merge_iter` instead")]
pub fn merge_hashmap<K, V, C, S>(
    prev: HashMap<K, V, S>,
    next: HashMap<K, V, S>,
    c: &C,
) -> MergeResult<HashMap<K, V, S>>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    merge_iter(prev, next, c)
}

/// Shallow merge the next [`HashSet`] into the previous [`HashSet`],
/// overwriting duplicates.
#[deprecated(note = "Use `merge_iter` instead")]
pub fn merge_hashset<T, C, S>(
    prev: HashSet<T, S>,
    next: HashSet<T, S>,
    c: &C,
) -> MergeResult<HashSet<T, S>>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    merge_iter(prev, next, c)
}

pub fn merge_iter<M, A, C>(mut prev: M, next: M, _: &C) -> MergeResult<M>
where
    M: Extend<A> + IntoIterator<Item = A>,
{
    prev.extend(next);
    Ok(Some(prev))
}
