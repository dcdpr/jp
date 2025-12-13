/// Get the name of the calling function using Rust's type system
#[macro_export]
macro_rules! function_name {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        let name = name.trim_end_matches("::{{closure}}::f");
        match &name.rfind(':') {
            Some(pos) => &name[pos + 1..name.len()],
            None => &name[..name.len()],
        }
    }};
}
