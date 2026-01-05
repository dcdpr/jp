use std::{
    convert::Infallible,
    env::current_dir,
    path::{Path, PathBuf},
};

use clean_path::Clean as _;
use relative_path::RelativePathBuf;
use tracing::trace;
use url::Url;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub(crate) enum AttachmentUrlOrPath {
    Url(Url),
    Path(RelativePathBuf),
}

impl AttachmentUrlOrPath {
    pub fn parse(&self, root: Option<&Path>) -> Result<Url> {
        let path = match &self {
            AttachmentUrlOrPath::Url(url) => return Ok(url.clone()),
            AttachmentUrlOrPath::Path(path) => path,
        };

        // Special case for file attachments
        trace!(
            path = path.as_str(),
            "URI is not a valid URL, treating as file path."
        );
        let (path, exclude) = match path.as_str().strip_prefix('!') {
            Some(path) => (path, "?exclude=true"),
            None => (path.as_str(), ""),
        };

        // Check if the path falls within the workspace.
        //
        // If `root` is `None`, then we allow absolute paths, otherwise we
        // assume the context is a workspace and we only allow relative paths.
        let mut path = PathBuf::from(path);
        if let Some(root) = root {
            if path.is_relative() {
                let Ok(cwd) = current_dir() else {
                    return Err(Error::Attachment(format!(
                        "Attachment path is relative, but the current directory could not be \
                         determined: {}",
                        path.display()
                    )));
                };

                path = cwd.join(path);
            }

            if !path.exists() {
                return Err(Error::Attachment(format!(
                    "Attachment path does not exist: {}",
                    path.display()
                )));
            }

            let p = path.clean();
            let Ok(p) = p.strip_prefix(root) else {
                return Err(Error::Attachment(format!(
                    "Attachment path must be relative to the workspace: {}",
                    path.display()
                )));
            };

            path = p.to_path_buf();
        } else if !path.exists() {
            return Err(Error::Attachment(format!(
                "Attachment path does not exist: {}",
                path.display()
            )));
        }

        Url::parse(&format!("file:{}{exclude}", path.display())).map_err(Into::into)
    }
}

impl std::str::FromStr for AttachmentUrlOrPath {
    type Err = Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Url::parse(s)
            .map(Self::Url)
            .or_else(|_| Ok(Self::Path(RelativePathBuf::from(s))))
    }
}
