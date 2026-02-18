pub struct Page<T> {
    pub(crate) items: Vec<T>,
}

impl<T> Page<T> {
    #[must_use]
    pub fn new(items: Vec<T>) -> Self {
        Self { items }
    }
}
