The function calls `downcast` (`downcast`, `downcast_ref`, or `downcast_mut`) to
recover a concrete type from `dyn Any` or a boxed error.

Fixes, in order:

1. Preserve the concrete type: return the specific error type or an enum with a
   variant for it so callers can `match` directly.
2. When a producer can return multiple types, represent them as an enum at the
   construction site instead of erasing them to `dyn Any` or `Box<dyn Error>`.

When a typed error must cross a fixed trait boundary that returns `Box<dyn
Error>` (such as an external trait implementation), downcasting is the only way
to recover the concrete type.
Report that the finding is required by the trait signature and leave the code as
is.

Don't replace `downcast` with string matching on the error message, and don't
erase a typed error to `Box<dyn Error>` to avoid adding an enum variant.

Marker, only when the user asks for one: `// qual:allow(coupling, deh) reason:
"…"` above the function.
