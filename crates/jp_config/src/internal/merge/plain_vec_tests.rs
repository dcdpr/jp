use test_log::test;

use super::*;

#[test]
fn appends_new_items() {
    let result = append_vec_dedup(vec![1, 2], vec![3, 4], &())
        .unwrap()
        .unwrap();

    assert_eq!(result, vec![1, 2, 3, 4]);
}

#[test]
fn drops_items_already_present() {
    // Two config layers naming the same directory contribute it once, which is
    // what `config_load_paths` and `beta_headers` need: the resolved list is
    // searched (respectively sent) in order, and a repeat is pure noise.
    let result = append_vec_dedup(vec!["a", "b"], vec!["b", "c"], &())
        .unwrap()
        .unwrap();

    assert_eq!(result, vec!["a", "b", "c"]);
}

#[test]
fn keeps_first_occurrence_order() {
    let result = append_vec_dedup(vec![3, 1], vec![2, 1, 3], &())
        .unwrap()
        .unwrap();

    assert_eq!(result, vec![3, 1, 2]);
}

#[test]
fn collapses_duplicates_within_a_single_layer() {
    let result = append_vec_dedup(vec![1], vec![2, 2], &()).unwrap().unwrap();

    assert_eq!(result, vec![1, 2]);
}
