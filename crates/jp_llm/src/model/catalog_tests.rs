use super::*;

struct Model(&'static str);

impl Cataloged for Model {
    fn catalog_id(&self) -> &str {
        self.0
    }
}

fn catalog() -> Catalog<Model> {
    Catalog::new(vec![
        Entry {
            aliases: &["alpha-latest", "alpha-2026-01-01"],
            value: Model("alpha"),
        },
        Entry {
            aliases: &[],
            value: Model("beta"),
        },
    ])
}

#[test]
fn finds_an_entry_under_its_canonical_id() {
    assert_eq!(catalog().get("beta").map(|m| m.0), Some("beta"));
}

#[test]
fn finds_an_entry_under_any_alias() {
    let catalog = catalog();

    assert_eq!(catalog.get("alpha-latest").map(|m| m.0), Some("alpha"));
    assert_eq!(catalog.get("alpha-2026-01-01").map(|m| m.0), Some("alpha"));
}

#[test]
fn an_unlisted_id_finds_nothing() {
    assert!(catalog().get("gamma").is_none());
}

#[test]
fn values_come_back_in_listed_order() {
    let ids: Vec<_> = catalog().values().map(|m| m.0).collect();

    assert_eq!(ids, vec!["alpha", "beta"]);
}

#[test]
fn an_alias_that_repeats_another_canonical_id_is_a_duplicate() {
    let catalog = Catalog(vec![
        Entry {
            aliases: &[],
            value: Model("alpha"),
        },
        Entry {
            aliases: &["alpha"],
            value: Model("beta"),
        },
    ]);

    assert_eq!(catalog.duplicate_id(), Some("alpha"));
}

#[test]
fn distinct_ids_have_no_duplicate() {
    assert_eq!(catalog().duplicate_id(), None);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "alpha is cataloged twice")]
fn building_a_catalog_with_a_duplicate_panics_in_debug() {
    Catalog::new(vec![
        Entry {
            aliases: &[],
            value: Model("alpha"),
        },
        Entry {
            aliases: &[],
            value: Model("alpha"),
        },
    ]);
}
