use super::{FunctionStat, by_samples, name_matches};

fn stat(function: &str, samples: u64) -> FunctionStat {
    FunctionStat {
        function: function.to_string(),
        samples,
        fraction: 0.0,
    }
}

fn ranked(mut stats: Vec<FunctionStat>) -> Vec<String> {
    stats.sort_by(by_samples);

    stats.into_iter().map(|stat| stat.function).collect()
}

#[test]
fn the_busiest_function_ranks_first() {
    assert_eq!(
        ranked(vec![
            stat("parse", 4),
            stat("render", 90),
            stat("decode", 12)
        ]),
        vec!["render", "decode", "parse"]
    );
}

/// Inclusive counting makes ties the common case rather than the exception:
/// every frame on one call chain carries that chain's whole count.
/// Those counts come out of a `HashMap`, so without the name as a second key
/// the top-N cut keeps an arbitrary subset of the chain and two reads of one
/// trace disagree about which functions are hot.
#[test]
fn a_shared_call_chain_ranks_the_same_whichever_order_it_arrives_in() {
    let one = vec![
        stat("main", 469),
        stat("dispatch", 469),
        stat("applicationDidFinishLaunching", 469),
        stat("body", 469),
    ];
    let other = vec![
        stat("body", 469),
        stat("main", 469),
        stat("applicationDidFinishLaunching", 469),
        stat("dispatch", 469),
    ];

    assert_eq!(ranked(one.clone()), ranked(other));
    assert_eq!(ranked(one), vec![
        "applicationDidFinishLaunching",
        "body",
        "dispatch",
        "main"
    ]);
}

#[test]
fn a_function_is_matched_whole_or_by_part_of_its_name() {
    assert!(name_matches(
        "jp_config::partial::partial_opt",
        "partial_opt"
    ));
    assert!(name_matches("deserialize", "deserialize"));
    assert!(!name_matches("serialize", "deserialize"));
}
