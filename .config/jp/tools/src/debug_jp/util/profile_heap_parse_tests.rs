use super::*;

const FIXTURE: &str = r#"{
  "dhatFileVersion": 2,
  "mode": "heap",
  "verb": "Allocated",
  "bklt": true,
  "bkacc": false,
  "tu": "instrs",
  "Mtu": "Minstr",
  "tuth": 10,
  "cmd": "jp c fork",
  "pid": 12345,
  "tg": 67890,
  "te": 5000000,
  "pps": [
    {
      "tb": 1024,
      "tbk": 10,
      "mb": 512,
      "mbk": 5,
      "gb": 256,
      "gbk": 3,
      "eb": 0,
      "ebk": 0,
      "fs": [1, 2, 3]
    },
    {
      "tb": 4096,
      "tbk": 5,
      "mb": 2048,
      "mbk": 2,
      "gb": 1024,
      "gbk": 1,
      "eb": 100,
      "ebk": 1,
      "fs": [4, 2, 3]
    }
  ],
  "ftbl": [
    "[root]",
    "alloc::raw_vec::RawVec<u8>::reserve_for_push",
    "std::vec::Vec::push",
    "main",
    "alloc::collections::btree::map::BTreeMap::new"
  ]
}"#;

#[test]
fn parse_extracts_top_level_metadata() {
    let profile = parse(FIXTURE).unwrap();
    assert_eq!(profile.elapsed_units, 5_000_000);
    assert_eq!(profile.time_unit, "instrs");
}

#[test]
fn parse_extracts_program_points() {
    let profile = parse(FIXTURE).unwrap();
    assert_eq!(profile.program_points.len(), 2);

    let pp0 = &profile.program_points[0];
    assert_eq!(pp0.total_bytes, 1024);
    assert_eq!(pp0.total_blocks, 10);
    assert_eq!(pp0.peak_bytes, 256);
    assert_eq!(pp0.peak_blocks, 3);
    assert_eq!(pp0.frames, vec![
        "RawVec<u8>::reserve_for_push".to_owned(),
        "Vec::push".to_owned(),
        "main".to_owned(),
    ]);
}

#[test]
fn parse_sums_totals_across_pps() {
    let profile = parse(FIXTURE).unwrap();
    assert_eq!(profile.total_bytes, 1024 + 4096);
    assert_eq!(profile.total_blocks, 10 + 5);
    assert_eq!(profile.peak_bytes, 256 + 1024);
    assert_eq!(profile.peak_blocks, 3 + 1);
    assert_eq!(profile.end_bytes, 100);
    assert_eq!(profile.end_blocks, 1);
}

#[test]
fn aggregate_by_leaf_groups_distinct_leaves() {
    let profile = parse(FIXTURE).unwrap();
    let agg = profile.aggregate_by_leaf();
    // Two PPs with two different leaves (frames[0] differs).
    assert_eq!(agg.len(), 2);
    // Sorted by total_blocks descending: first PP has 10 blocks > second's 5.
    assert_eq!(agg[0].leaf, "RawVec<u8>::reserve_for_push");
    assert_eq!(agg[0].total_blocks, 10);
    assert_eq!(agg[0].total_bytes, 1024);
    assert_eq!(agg[1].leaf, "BTreeMap::new");
    assert_eq!(agg[1].total_blocks, 5);
}

#[test]
fn aggregate_by_leaf_sums_pps_with_same_leaf() {
    // Two PPs with the same leaf frame should be combined.
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 100, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 2]},
        {"tb": 200, "tbk": 3, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 3]}
      ],
      "ftbl": ["[root]", "leaf_fn", "caller_a", "caller_b"]
    }"#;
    let profile = parse(json).unwrap();
    let agg = profile.aggregate_by_leaf();
    assert_eq!(agg.len(), 1);
    assert_eq!(agg[0].leaf, "leaf_fn");
    assert_eq!(agg[0].total_blocks, 4);
    assert_eq!(agg[0].total_bytes, 300);
    assert_eq!(agg[0].sites, 2);
}

#[test]
fn parse_strips_address_prefix() {
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 1, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 2, 3]}
      ],
      "ftbl": [
        "[root]",
        "0x102058e68: <alloc::vec::Vec<u8> as core::clone::Clone>::clone (src/vec/mod.rs:3768:9)",
        "0xabcdef: jp_config::PartialAppConfig::clone (lib.rs:42:5)",
        "no-address-here: jp_cli::run"
      ]
    }"#;
    let profile = parse(json).unwrap();
    let frames = &profile.program_points[0].frames;
    // Address prefix stripped, stdlib path polished.
    assert_eq!(
        frames[0],
        "<Vec<u8> as Clone>::clone (src/vec/mod.rs:3768:9)"
    );
    assert_eq!(
        frames[1],
        "jp_config::PartialAppConfig::clone (lib.rs:42:5)"
    );
    // Strings that don't match `0xHEX: ` are passed through unchanged.
    assert_eq!(frames[2], "no-address-here: jp_cli::run");
}

#[test]
fn interesting_leaf_finds_jp_frame() {
    // Stack: allocator -> Vec::clone -> jp_config::clone -> jp_conversation::extend
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 1, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 2, 3, 4]}
      ],
      "ftbl": [
        "[root]",
        "<alloc::alloc::Global as core::alloc::Allocator>::allocate",
        "<alloc::vec::Vec<u8> as core::clone::Clone>::clone",
        "<jp_config::PartialAppConfig as core::clone::Clone>::clone",
        "jp_conversation::stream::ConversationStream::extend"
      ]
    }"#;
    let profile = parse(json).unwrap();
    let pp = &profile.program_points[0];
    assert_eq!(
        pp.interesting_leaf(),
        "<jp_config::PartialAppConfig as Clone>::clone"
    );
}

#[test]
fn interesting_leaf_falls_back_to_first_when_no_jp_frame() {
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 1, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 2]}
      ],
      "ftbl": [
        "[root]",
        "<alloc::alloc::Global as core::alloc::Allocator>::allocate",
        "std::vec::Vec::push"
      ]
    }"#;
    let profile = parse(json).unwrap();
    let pp = &profile.program_points[0];
    assert_eq!(
        pp.interesting_leaf(),
        "<alloc::alloc::Global as core::alloc::Allocator>::allocate"
    );
}

#[test]
fn parse_polishes_stdlib_paths() {
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 1, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 2, 3]}
      ],
      "ftbl": [
        "[root]",
        "<alloc::vec::Vec<alloc::string::String> as core::clone::Clone>::clone",
        "core::iter::traits::iterator::Iterator::for_each::<...>",
        "<jp_config::PartialAppConfig as core::clone::Clone>::clone"
      ]
    }"#;
    let profile = parse(json).unwrap();
    let frames = &profile.program_points[0].frames;
    assert_eq!(frames[0], "<Vec<String> as Clone>::clone");
    assert_eq!(frames[1], "Iterator::for_each::<...>");
    assert_eq!(frames[2], "<jp_config::PartialAppConfig as Clone>::clone");
}

#[test]
fn parse_filters_fn_trampolines() {
    // FnMut::call_mut between two real frames should be filtered out.
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 1, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 2, 3]}
      ],
      "ftbl": [
        "[root]",
        "<indexmap::Bucket<X> as core::clone::Clone>::clone",
        "<<indexmap::Bucket<X> as core::clone::Clone>::clone as core::ops::function::FnMut<(&indexmap::Bucket<X>,)>>::call_mut",
        "<jp_conversation::ConversationStream>::extend"
      ]
    }"#;
    let profile = parse(json).unwrap();
    let frames = &profile.program_points[0].frames;
    // The FnMut trampoline frame is dropped; only the real two remain.
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0], "<indexmap::Bucket<X> as Clone>::clone");
    assert_eq!(frames[1], "<jp_conversation::ConversationStream>::extend");
}

#[test]
fn parse_filters_vec_clone_specialization_helpers() {
    // Vec::clone specialization is implemented via a chain of internal
    // helpers that all delegate to the upstream T::clone(). After the
    // upstream Bucket::clone leaf and before the upstream Vec::clone_from
    // frame, these helpers contribute no signal. They must all be filtered.
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 1, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 2, 3, 4, 5, 6, 7]}
      ],
      "ftbl": [
        "[root]",
        "<indexmap::Bucket<X> as core::clone::Clone>::clone",
        "<alloc::vec::Vec<indexmap::Bucket<X>>>::extend_trusted::<core::iter::adapters::cloned::Cloned<core::slice::iter::Iter<indexmap::Bucket<X>>>>",
        "<alloc::vec::Vec<indexmap::Bucket<X>> as alloc::vec::spec_extend::SpecExtend<indexmap::Bucket<X>, core::iter::adapters::cloned::Cloned<core::slice::iter::Iter<indexmap::Bucket<X>>>>>::spec_extend",
        "<alloc::vec::Vec<indexmap::Bucket<X>>>::extend_from_slice (alloc/src/vec/mod.rs:3519:14)",
        "<[indexmap::Bucket<X>] as alloc::slice::SpecCloneIntoVec<indexmap::Bucket<X>, alloc::alloc::Global>>::clone_into",
        "<alloc::vec::Vec<indexmap::Bucket<X>> as core::clone::Clone>::clone_from",
        "<indexmap::inner::Core<X> as core::clone::Clone>::clone_from"
      ]
    }"#;
    let profile = parse(json).unwrap();
    let frames = &profile.program_points[0].frames;
    // Only the leaf, Vec::clone_from, and Core::clone_from should survive.
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0], "<indexmap::Bucket<X> as Clone>::clone");
    assert_eq!(frames[1], "<Vec<indexmap::Bucket<X>> as Clone>::clone_from");
    assert_eq!(frames[2], "<indexmap::inner::Core<X> as Clone>::clone_from");
}

#[test]
fn parse_filters_to_vec_in_helpers() {
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 1, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 2, 3, 4]}
      ],
      "ftbl": [
        "[root]",
        "<jp_config::assistant::sections::PartialSectionConfig as core::clone::Clone>::clone",
        "<jp_config::assistant::sections::PartialSectionConfig as <[_]>::to_vec_in::ConvertVec>::to_vec::<alloc::alloc::Global>",
        "<[jp_config::assistant::sections::PartialSectionConfig]>::to_vec_in::<alloc::alloc::Global>",
        "<alloc::vec::Vec<jp_config::assistant::sections::PartialSectionConfig> as core::clone::Clone>::clone"
      ]
    }"#;
    let profile = parse(json).unwrap();
    let frames = &profile.program_points[0].frames;
    // ConvertVec::to_vec and slice::to_vec_in are dropped; leaf and
    // Vec::clone survive.
    assert_eq!(frames.len(), 2);
    assert_eq!(
        frames[0],
        "<jp_config::assistant::sections::PartialSectionConfig as Clone>::clone"
    );
    assert_eq!(
        frames[1],
        "<Vec<jp_config::assistant::sections::PartialSectionConfig> as Clone>::clone"
    );
}

#[test]
fn parse_filters_iterator_adapter_wrappers() {
    // <Map<...> as Iterator>::fold is a stdlib adapter wrapper; filter it.
    // <ConversationStream::IntoIter as Iterator>::next is OUR iterator; keep it.
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 1, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1, 2, 3, 4]}
      ],
      "ftbl": [
        "[root]",
        "<core::iter::adapters::map::Map<X, F> as core::iter::traits::iterator::Iterator>::fold::<()>",
        "<core::iter::adapters::cloned::Cloned<X> as core::iter::traits::iterator::Iterator>::fold::<()>",
        "<jp_conversation::stream::IntoIter as core::iter::traits::iterator::Iterator>::next",
        "<jp_conversation::ConversationStream>::extend"
      ]
    }"#;
    let profile = parse(json).unwrap();
    let frames = &profile.program_points[0].frames;
    // Map/Cloned wrappers dropped; the IntoIter and extend frames survive.
    assert_eq!(frames.len(), 2);
    assert!(frames[0].contains("jp_conversation::stream::IntoIter"));
    assert!(frames[0].contains("Iterator>::next"));
    assert_eq!(frames[1], "<jp_conversation::ConversationStream>::extend");
}

#[test]
fn parse_demangles_mangled_frames() {
    // If a frame slips through as raw v0-mangled, demangle it.
    let json = r#"{
      "dhatFileVersion": 2,
      "mode": "heap",
      "verb": "Allocated",
      "tu": "instrs",
      "Mtu": "Minstr",
      "cmd": "x",
      "pid": 0,
      "tg": 0,
      "te": 0,
      "pps": [
        {"tb": 1, "tbk": 1, "mb": 0, "mbk": 0, "gb": 0, "gbk": 0, "eb": 0, "ebk": 0, "fs": [1]}
      ],
      "ftbl": ["[root]", "_RNvCsh5KVlJR8TJf_6jp_cli3run"]
    }"#;
    let profile = parse(json).unwrap();
    assert_eq!(profile.program_points[0].frames[0], "jp_cli::run");
}
