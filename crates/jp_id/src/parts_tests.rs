use test_log::test;

use super::*;

#[test]
#[allow(clippy::type_complexity)]
fn test_parts_from_str() {
    #[rustfmt::skip]
        let cases: Vec<(&str, Result<(&str, char, &str, Option<&str>), &str>)> = vec![
            // With global_id
            ("jp-bar-baz", Ok(("jp", 'b', "ar", Some("baz")))),
            ("jp-qux-ba1z23", Ok(("jp", 'q', "ux", Some("ba1z23")))),
            ("jp-boo_baa_bop-ba1z13", Ok(("jp", 'b', "oo_baa_bop", Some("ba1z13")))),
            // Without global_id
            ("jp-bar", Ok(("jp", 'b', "ar", None))),
            ("jp-c17457886043", Ok(("jp", 'c', "17457886043", None))),
            // Errors
            ("jp", Err("Missing prefix: jp")),
            ("jp-", Err("Missing variant")),
            ("jp-b", Err("Missing target ID")),
            ("jp-foo-", Err("Missing global ID")),
            ("jp-afoo-baz-qux", Err("Invalid global ID, must be [a-z]: baz-qux")),
            ("foo-bar-baz", Err("Invalid prefix, must be jp: foo")),
        ];

    for (input, result) in cases {
        let parts = Parts::from_str(input)
            .map(|parts| {
                (
                    parts.prefix,
                    parts.variant.into_inner(),
                    parts.target_id.to_string(),
                    parts.global_id.map(|g| g.to_string()),
                )
            })
            .map_err(|e| e.to_string());

        let result = result
            .map(|(prefix, variant, target_id, global_id)| {
                (
                    prefix.to_string(),
                    variant,
                    target_id.to_string(),
                    global_id.map(str::to_string),
                )
            })
            .map_err(str::to_string);

        assert_eq!(parts, result, "input: {input}");
    }
}
