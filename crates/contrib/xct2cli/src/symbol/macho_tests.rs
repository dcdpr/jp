use super::{
    FilePc, Slide, SlideCandidate, demangle, function_containing, function_length, rank_candidates,
};

fn candidate(slide: u64, top_function_samples: u64, top_function_size: u64) -> SlideCandidate {
    SlideCandidate {
        slide: Slide::new(slide),
        covered_samples: top_function_samples,
        top_function_samples,
        top_function_address: FilePc::new(0x1000),
        top_function_size,
        top_function_name: None,
    }
}

fn ranked(mut candidates: Vec<SlideCandidate>) -> Vec<u64> {
    candidates.sort_by(rank_candidates);

    candidates.into_iter().map(|c| c.slide.raw()).collect()
}

/// The heuristic: the slide that concentrates the most samples in one function
/// is the likeliest, and among equals the tighter function is the better
/// evidence.
#[test]
fn the_most_concentrated_candidate_ranks_first() {
    assert_eq!(
        ranked(vec![
            candidate(0x8000, 10, 400),
            candidate(0x4000, 90, 400),
            candidate(0xc000, 50, 400),
        ]),
        vec![0x4000, 0xc000, 0x8000]
    );
}

#[test]
fn a_tie_on_samples_prefers_the_tighter_function() {
    assert_eq!(
        ranked(vec![
            candidate(0x8000, 90, 4_000),
            candidate(0x4000, 90, 40)
        ]),
        vec![0x4000, 0x8000]
    );
}

/// `enumerate_slides` says itself that several slides look equally valid for a
/// short trace or a stripped binary, and it accumulates its candidates in a
/// set.
/// Without the slide as a last key, which one a caller takes first would differ
/// between runs — and a wrong slide does not fail, it names the wrong
/// functions.
#[test]
fn candidates_tied_on_every_heuristic_rank_by_slide() {
    let one = vec![
        candidate(0xc000, 90, 400),
        candidate(0x4000, 90, 400),
        candidate(0x8000, 90, 400),
    ];
    let other = vec![
        candidate(0x8000, 90, 400),
        candidate(0xc000, 90, 400),
        candidate(0x4000, 90, 400),
    ];

    assert_eq!(ranked(one.clone()), ranked(other));
    assert_eq!(ranked(one), vec![0x4000, 0x8000, 0xc000]);
}

/// A probe lands in the function whose range covers it, and the last function's
/// range runs to the end of `__TEXT` because nothing follows it to bound it.
#[test]
fn a_probe_resolves_to_the_function_containing_it() {
    let starts = [0x1000, 0x1400, 0x2000];

    assert_eq!(function_containing(&starts, 0x1000, 0x3000), Some(0x1000));
    assert_eq!(function_containing(&starts, 0x13ff, 0x3000), Some(0x1000));
    assert_eq!(function_containing(&starts, 0x1400, 0x3000), Some(0x1400));
    assert_eq!(function_containing(&starts, 0x2fff, 0x3000), Some(0x2000));
}

#[test]
fn a_probe_outside_every_function_resolves_to_none() {
    let starts = [0x1000, 0x1400];

    // Before the first function.
    assert_eq!(function_containing(&starts, 0x0fff, 0x2000), None);

    // Past the end of `__TEXT`, which bounds the last function.
    assert_eq!(function_containing(&starts, 0x2000, 0x2000), None);
    assert_eq!(function_containing(&[], 0x1000, 0x2000), None);
}

#[test]
fn a_functions_length_runs_to_the_next_start_or_to_the_end_of_text() {
    let starts = [0x1000, 0x1400, 0x2000];

    assert_eq!(function_length(&starts, 0x1000, 0x3000), 0x400);
    assert_eq!(function_length(&starts, 0x1400, 0x3000), 0xc00);
    assert_eq!(function_length(&starts, 0x2000, 0x3000), 0x1000);
}

/// A `__TEXT` end below the function start would otherwise underflow.
#[test]
fn a_function_starting_past_the_end_of_text_has_no_length() {
    assert_eq!(function_length(&[0x4000], 0x4000, 0x1000), 0);
}

/// Three manglings reach this, and a name in none of them is its own answer: a
/// symbol nobody can demangle is still the only name that frame has.
#[test]
fn a_name_is_demangled_by_whichever_scheme_claims_it() {
    assert_eq!(demangle("_$s2JP5TraceO5eventyyF"), "JP.Trace.event() -> ()");
    assert_eq!(
        demangle("_ZN4core3fmt9Formatter3pad17h0123456789abcdefE"),
        "core::fmt::Formatter::pad"
    );
    assert_eq!(demangle("main"), "main");
    assert_eq!(demangle(""), "");
}
