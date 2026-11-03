//! The schema of a parameter taking labels: the labels the vocabulary offers,
//! and, where a list can't say it, the same in words.
//!
//! A tool writing an item's labels is offered the labels it may add, and told
//! what the vocabulary asks of an item: the keys it bounds with `min` or `max`,
//! and the keys one item holds on its own.
//! A tool filtering by labels is also offered retired values and a bare key per
//! facet, which is how a filter asks for any value under it.
//!
//! A vocabulary with a key that takes any value gets no list at all: a list
//! holds only the values it names, so it would refuse that key's.
//! Its labels are described in words instead.

use jp_label::{Unique, Vocabulary};

use crate::toml_string;

/// How a tool uses the labels a parameter takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The labels a new item carries; the parameter is required when the
    /// vocabulary needs an item to carry one.
    Create,

    /// Labels added to an item.
    Add,

    /// Labels a listing is narrowed by, which an item doesn't carry, so the
    /// vocabulary's rules don't apply.
    Filter,
}

/// The schema of `parameter`, of the tool keyed `key` (already a TOML key),
/// taking labels of `vocabulary` that an `item` carries.
///
/// Empty for a vocabulary that defines no labels: an empty `enum` accepts
/// nothing, which some providers refuse outright.
pub(crate) fn render(
    key: &str,
    parameter: &str,
    vocabulary: &Vocabulary,
    item: &str,
    role: Role,
) -> String {
    if vocabulary.is_empty() {
        return String::new();
    }

    let filter = role == Role::Filter;
    let tokens = tokens(vocabulary, filter);
    let open: Vec<&str> = vocabulary.open_keys().collect();
    let rules = if filter {
        None
    } else {
        rules(vocabulary, item)
    };

    let mut out = String::new();
    if role == Role::Create && vocabulary.facets().any(|(_, facet)| facet.min() > 0) {
        out.push_str(&format!(
            "\n[conversation.tools.{key}.parameters.{parameter}]\nrequired = true\n"
        ));
    }
    out.push_str(&format!(
        "\n[conversation.tools.{key}.parameters.{parameter}.items]\n"
    ));
    if let Some(summary) = summary(&tokens, &open, rules.as_deref(), filter) {
        out.push_str(&format!("summary = {}\n", toml_string(&summary)));
    }
    if open.is_empty() {
        out.push_str("enum = [\n");
        for token in tokens {
            out.push_str(&format!("  {},\n", toml_string(&token)));
        }
        out.push_str("]\n");
    }

    out
}

/// The labels a parameter is offered: what a write may add, or for a `filter`,
/// also retired values and a bare key per facet.
fn tokens(vocabulary: &Vocabulary, filter: bool) -> Vec<String> {
    if !filter {
        return vocabulary.tokens();
    }

    let mut tokens = vocabulary.tokens_including_retired();
    tokens.extend(vocabulary.keys().map(ToOwned::to_owned));
    tokens.sort();
    tokens.dedup();

    tokens
}

/// The description of one label in a tool's schema, or `None` when the list of
/// accepted values says all there is.
///
/// With `open` keys there is no list, so the labels are named here instead.
/// A `filter` also takes a bare key, for any value under it.
fn summary(tokens: &[String], open: &[&str], rules: Option<&str>, filter: bool) -> Option<String> {
    if open.is_empty() && rules.is_none() {
        return None;
    }

    let mut summary = if filter {
        "One `key=value` label, or a bare `key` for any value under it".to_owned()
    } else {
        "One `key=value` label".to_owned()
    };
    if !open.is_empty() {
        let any = open
            .iter()
            .map(|key| format!("{key}=<value>"))
            .collect::<Vec<_>>()
            .join(", ");
        summary.push_str(&if tokens.is_empty() {
            format!(": {any}, with any value")
        } else {
            format!(": {}, or {any} with any value", tokens.join(", "))
        });
    }
    summary.push('.');
    if let Some(rules) = rules {
        summary.push(' ');
        summary.push_str(rules);
    }

    Some(summary)
}

/// What the vocabulary's bounded and unique keys ask of an `item`, as
/// sentences, or `None` when no key asks anything.
fn rules(vocabulary: &Vocabulary, item: &str) -> Option<String> {
    let bounds: Vec<String> = vocabulary
        .facets()
        .filter_map(|(key, facet)| facet.bound().map(|bound| format!("{bound} under `{key}`")))
        .collect();

    let mut sentences = vec![];
    if let Some((last, rest)) = bounds.split_last() {
        let all = if rest.is_empty() {
            last.clone()
        } else {
            format!("{} and {last}", rest.join(", "))
        };
        sentences.push(format!("A {item} carries {all}."));
    }
    for (key, facet) in vocabulary.facets() {
        match facet.unique() {
            Some(Unique::KeyValue) if !facet.is_bare() => {
                sentences.push(format!("Only one {item} carries each `{key}` value."));
            }
            Some(_) => sentences.push(format!("Only one {item} carries `{key}`.")),
            None => {}
        }
    }

    (!sentences.is_empty()).then(|| sentences.join(" "))
}

#[cfg(test)]
#[path = "labels_tests.rs"]
mod tests;
