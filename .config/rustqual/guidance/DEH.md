The function calls `downcast` to find out which concrete type a `dyn Any` or a
boxed error holds.

Fixes, in order:

1. Keep the type.
   Have the function that produces the value return the concrete error, or an
   enum with a variant for it, so the caller matches on it.
2. When several types are possible, name them in an enum where the value is
   created.

A typed error that has to pass through a signature someone else fixed, such as
a trait method that returns `Box<dyn Error>`, can only be recovered with
`downcast`.
Say that the finding is of this kind and leave it.

Don't replace the `downcast` with a comparison on the error's message, and
don't widen a typed error to `Box<dyn Error>` to avoid adding a variant.

Marker, only when the user asks for one: `// qual:allow(coupling, deh) reason:
"…"` above the function.
