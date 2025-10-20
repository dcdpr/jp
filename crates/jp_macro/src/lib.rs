/// A macro to make `tokio::select!` more ergonomic.
///
/// This macro is a drop-in replacement for `tokio::select!` that works exactly
/// the same, but supports rustfmt. It uses closure syntax which rustfmt
/// supports with support for `if branches` and the "biased" keyword.
///
/// ```rust,ignore
/// select!(
///     biased,
///     tasks.join_next(),
///     |task| if (!ready) {
///         // ...
///     },
///     title.changed(),
///     |title| {
///         // ...
///     }
/// )
/// ```
#[macro_export]
macro_rules! select {
    (
        biased,
        $(
            $next:expr,
            |$name:ident| $(if ($del:expr) )* {
                $( $rest:tt )*
            }
        ),*
        $(,)?
    ) => {
        tokio::select! {
            biased;
            $(
                $name = $next $(, if $del)* => {
                    $($rest)*
                }
            ),*
        }
    };
    (
        $(
            $next:expr,
            |$name:ident| $(if ($del:expr) )* {
                $( $rest:tt )*
            }
        ),*
        $(,)?
    ) => {
        tokio::select! {
            $(
                $name = $next $(, if $del)* => {
                    $($rest)*
                }
            ),*
        }
    };
}
