use std::{
    fs::{self, File},
    io::Write as _,
    path::PathBuf,
};

use crate::Error;

pub(crate) async fn fs_create_file(
    root: PathBuf,
    path: String,
    content: Option<String>,
) -> std::result::Result<String, Error> {
    let p = PathBuf::from(&path);

    if p.is_absolute() {
        return Err("Path must be relative.".into());
    }

    if p.iter().any(|c| c.len() > 30) {
        return Err("Individual path components must be less than 30 characters long.".into());
    }

    if p.iter().count() > 20 {
        return Err("Path must be less than 20 components long.".into());
    }

    let absolute_path = root.join(path.trim_start_matches('/'));
    if absolute_path.is_dir() {
        return Err("Path is an existing directory.".into());
    }

    if absolute_path.exists() {
        return Err("Path points to existing file".into());
    }

    let Some(parent) = absolute_path.parent() else {
        return Err("Path has no parent".into());
    };

    fs::create_dir_all(parent)?;
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&absolute_path)?;

    if let Some(content) = content {
        file.write_all(content.as_bytes())?;
    }

    Ok(format!(
        "File '{}' created. File size: {}",
        path,
        file.metadata()?.len()
    ))
}
