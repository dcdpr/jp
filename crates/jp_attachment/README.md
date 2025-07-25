# Attachments

A URI-based attachment trait.

## Usage

```rust
use jp_attachment::{Attachment, Handler};
use std::error::Error;

pub struct MyAttachmentHandler(Vec<Attachment>);

impl Handler for MyAttachmentHandler {
    fn scheme(&self) -> &'static str {
        "my-scheme"
    }

    fn add(&mut self, uri: &Url) -> Result<(), Box<dyn Error>> {
        let attachment = url_to_attachment(uri);
        self.0.push(attachment);
    }

    fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error>> {
        let attachment = url_to_attachment(uri);
        self.0.remove(&attachment);
    }

    fn get(&self, cwd: &Path) -> Result<Vec<Attachment>, Box<dyn Error>> {
        self.0.clone()
    }
}

fn url_to_attachment(_: &Url) -> Attachment {
    todo!()
}
```
