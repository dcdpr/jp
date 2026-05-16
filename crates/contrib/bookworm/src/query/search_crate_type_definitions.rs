use std::{collections::HashSet, rc::Rc};

use rusqlite::{Connection, named_params, types::Value};
use serde::Serialize;

use crate::{dl, docs, docs::Item, error::Error, index, index::EntryType, query::GLOBAL_CLIENT};

#[derive(Serialize)]
pub struct TypeDefinition {
    #[serde(flatten)]
    pub item: Item,
    pub docs_resource: String,
    pub src_resource: Option<String>,
}

/// Fetch the type definition for a docs.rs URI.
pub async fn search_crate_type_definitions(
    crate_name: &str,
    crate_version: &str,
    query: &str,
    mut kinds: Vec<EntryType>,
    limit: Option<u32>,
) -> Result<Vec<TypeDefinition>, Error> {
    let dl_cfg = dl::Config::default()
        .crate_name(crate_name)
        .version(crate_version)
        .root(&GLOBAL_CLIENT.crates_path)
        .client(GLOBAL_CLIENT.http_client.clone());

    let root = dl::download(dl_cfg).await?;

    let index_file = root.join("index.sqlite");
    let index_cfg = index::Config::default().source(&root).output(&index_file);

    index::index(index_cfg)?;

    let conn = Connection::open(index_file)?;
    rusqlite::vtab::array::load_module(&conn)?;

    if kinds.is_empty() {
        kinds = EntryType::all();
    }

    let kinds = Rc::new(
        kinds
            .iter()
            .map(ToString::to_string)
            .map(Value::from)
            .collect::<Vec<Value>>(),
    );

    let limit = limit.unwrap_or(u32::MAX);

    let exact_query = query.replace('%', "");
    let fuzzy_query = match query {
        "" => "%",
        _ if query.starts_with('%') => query,
        _ if query.ends_with('%') => query,
        _ => &format!("%{}%", query.replace(' ', "%")),
    };

    let mut stmt = conn.prepare(
        "
        SELECT path
        FROM searchIndex
        WHERE (name LIKE :fuzzy_query OR path LIKE :fuzzy_query) AND type IN rarray(:kinds)
        ORDER BY
           CASE
                WHEN name = :exact_query THEN 0
                WHEN path = :exact_query THEN 1
                WHEN name LIKE '%' || :exact_query THEN 2
                WHEN name LIKE :exact_query || '%' THEN 3
                WHEN path LIKE '%' || :exact_query THEN 4
                WHEN path LIKE :exact_query || '%' THEN 5
                ELSE 6
           END,
           length(name), length(path) ASC
        LIMIT :limit
    ",
    )?;

    let rows = stmt.query_map(
        named_params![
            ":fuzzy_query": fuzzy_query,
            ":exact_query": exact_query,
            ":kinds": &kinds,
            ":limit": limit
        ],
        |row| row.get::<_, String>(0),
    )?;

    // Rustdoc indexes re-exported items at every canonical path they're
    // reachable from (e.g. `serde_json::Value` and `serde_json::value::Value`
    // are the same enum). Their `src_path` is identical, so we dedupe on it
    // to avoid emitting near-identical results that waste tokens and create
    // false ambiguity for the LLM.
    let mut definitions = vec![];
    let mut seen_src: HashSet<String> = HashSet::new();
    for row in rows {
        let documentation_resource = row?;

        let item = docs::Docs::new(&root, &conn)?.item(&documentation_resource)?;

        if let Some(src) = item.src_path.as_deref()
            && !seen_src.insert(src.to_owned())
        {
            continue;
        }

        let src_resource = item
            .src_path
            .as_ref()
            .map(|p| format!("crate://{crate_name}/{crate_version}{p}"));

        let docs_resource =
            format!("crate://{crate_name}/{crate_version}/items/{documentation_resource}");

        definitions.push(TypeDefinition {
            item,
            docs_resource,
            src_resource,
        });
    }

    Ok(definitions)
}
