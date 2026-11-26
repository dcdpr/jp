# A serve-chat timing test fails under full-suite load

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-20
- **Label**: type=bug

`bridge::tests::a_failed_lookup_is_logged_and_left_out` in `jp-serve-chat`
asserts the logged lookup took `0` ms and sometimes gets `1`.
It failed twice in full `cargo test` runs on 2026-11-20 and passes when run
alone, so the assertion is on wall-clock time.
The fix is to stub the clock or assert on the other fields only, the same shape
of problem as T-19b6xrf.
