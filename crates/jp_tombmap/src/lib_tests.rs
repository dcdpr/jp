use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    hash::Hash,
};

use test_log::test;

use super::*;

// Helper to compare HashMaps (standard library for expected state)
fn assert_maps_equal<K, V, S>(actual: &HashMap<K, V, S>, expected: &HashMap<K, V, S>, context: &str)
where
    K: Eq + Hash + Debug,
    V: Eq + Debug,
    S: BuildHasher,
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
    IterMutIncrement(&'static str),
    Drain,
}

#[derive(Debug, Clone)]
struct ExpectedState {
    live: HashMap<&'static str, i32, RandomState>,
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
            "15. Insert 'y' after clear",
            Action::Insert("y", 51),
            ExpectedState::new([("x", 50), ("y", 51)], ["a", "b", "c"], ["x"]),
        ),
        (
            "16. Insert 'z' after clear",
            Action::Insert("z", 52),
            ExpectedState::new([("x", 50), ("y", 51), ("z", 52)], ["a", "b", "c"], ["x"]),
        ),
        (
            "17. IterMut all, modify 'y'",
            Action::IterMutIncrement("y"),
            ExpectedState::new([("x", 50), ("y", 52), ("z", 52)], ["a", "b", "c"], [
                "x", "y",
            ]),
        ),
        (
            "18. Drain",
            Action::Drain,
            ExpectedState::new([], ["a", "b", "c", "x", "y", "z"], []),
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
            #[expect(clippy::explicit_iter_loop)]
            Action::IterMutIncrement(k1) => {
                for (k, mut v) in map.iter_mut() {
                    if k == &k1 {
                        *v += 1;
                    }
                }
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
