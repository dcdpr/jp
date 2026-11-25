use camino::Utf8Path;
use jp_attachment::Handler;
use url::Url;

use super::AgenticShepherd;

fn canonical(uri: &str) -> Result<String, String> {
    AgenticShepherd::default()
        .canonical(&Url::parse(uri).unwrap())
        .map(|url| url.to_string())
        .map_err(|error| error.to_string())
}

#[test]
fn canonical_collapses_every_spelling_of_one_issue() {
    for spelling in ["ag:592", "ag://592", "ag:issue/592", "ag://issues/592"] {
        assert_eq!(
            canonical(spelling).as_deref(),
            Ok("ag://issues/592"),
            "{spelling}"
        );
    }
}

#[test]
fn canonical_sorts_and_dedups_environment_names() {
    assert_eq!(
        canonical("ag:592?env=B&envs=A&env=B").as_deref(),
        Ok("ag://issues/592?env=A&env=B")
    );
}

#[test]
fn canonical_rejects_an_unknown_namespace() {
    assert_eq!(
        canonical("ag://prs/1"),
        Err(
            "unsupported ag URI `ag://prs/1`; expected `ag://issues/N`, `ag:issues/N`, or `ag:N`"
                .to_owned()
        )
    );
}

#[tokio::test]
async fn add_list_remove_roundtrip() {
    let mut handler = AgenticShepherd::default();
    let cwd = Utf8Path::new(".");

    handler
        .add(&Url::parse("ag:592").unwrap(), cwd)
        .await
        .unwrap();
    handler
        .add(&Url::parse("ag://issues/100").unwrap(), cwd)
        .await
        .unwrap();
    // A different spelling of an existing reference collapses to one entry.
    handler
        .add(&Url::parse("ag://592").unwrap(), cwd)
        .await
        .unwrap();

    let urls: Vec<String> = handler
        .list()
        .await
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(urls, vec![
        "ag://issues/100".to_string(),
        "ag://issues/592".to_string(),
    ]);

    handler
        .remove(&Url::parse("ag:issue/592").unwrap())
        .await
        .unwrap();

    let urls: Vec<String> = handler
        .list()
        .await
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(urls, vec!["ag://issues/100".to_string()]);
}
