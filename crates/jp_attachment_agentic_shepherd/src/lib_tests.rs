use camino::Utf8Path;
use jp_attachment::Handler;
use url::Url;

use super::AgenticShepherd;

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
