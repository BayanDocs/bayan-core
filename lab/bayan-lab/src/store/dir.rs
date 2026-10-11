//! The local-directory backend: objects are files below a root directory.

use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use super::{Key, MAX_OBJECT_SIZE, Store, StoreError};

/// A store in a local directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirStore {
    root: PathBuf,
}

impl DirStore {
    /// The store whose root is `root`. The directory is created when the first object is stored.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn file(&self, key: &Key) -> PathBuf {
        key.path()
            .split('/')
            .fold(self.root.clone(), |path, segment| path.join(segment))
    }
}

impl Store for DirStore {
    fn describe(&self) -> String {
        format!("the directory {}", self.root.display())
    }

    fn get(&self, key: &Key) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.file(key);
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(StoreError(format!("cannot read {}: {error}", key.path()))),
        };
        let mut bytes = Vec::new();
        file.take(MAX_OBJECT_SIZE + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| StoreError(format!("cannot read {}: {error}", key.path())))?;
        if !u64::try_from(bytes.len()).is_ok_and(|len| len <= MAX_OBJECT_SIZE) {
            return Err(StoreError(format!(
                "{} is larger than the limit",
                key.path()
            )));
        }
        Ok(Some(bytes))
    }

    fn put(&self, key: &Key, bytes: &[u8], _: &Path) -> Result<(), StoreError> {
        let path = self.file(key);
        let folder = path
            .parent()
            .ok_or_else(|| StoreError(format!("no folder for {}", key.path())))?;
        fs::create_dir_all(folder).map_err(|error| {
            StoreError(format!(
                "cannot create the folder for {}: {error}",
                key.path()
            ))
        })?;
        // Write to a temporary name in the same folder, then rename: a reader never sees half an object, and an interrupted write leaves no object behind.
        let temporary = folder.join(format!(".{}.partial", key.sha256));
        fs::write(&temporary, bytes)
            .map_err(|error| StoreError(format!("cannot write {}: {error}", key.path())))?;
        fs::rename(&temporary, &path).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            StoreError(format!("cannot write {}: {error}", key.path()))
        })
    }

    fn contains(&self, key: &Key) -> Result<bool, StoreError> {
        match fs::metadata(self.file(key)) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(StoreError(format!("cannot read {}: {error}", key.path()))),
        }
    }

    fn list(&self) -> Result<Option<Vec<String>>, StoreError> {
        let mut found = Vec::new();
        for area in ["objects", "licenses"] {
            list_folder(&self.root.join(area), area, 0, &mut found)?;
        }
        found.sort();
        Ok(Some(found))
    }
}

/// How deep below `objects/` and `licenses/` the listing descends: objects lie one folder deep, so anything deeper is unexpected anyway, and a deeper folder is listed as one entry.
const MAX_LIST_DEPTH: usize = 4;

/// Adds every file below `folder` to `found`, as a `/`-separated path starting with `prefix`. Symbolic links are listed as entries, not followed; folders deeper than [`MAX_LIST_DEPTH`] are listed as entries, not entered.
fn list_folder(
    folder: &Path,
    prefix: &str,
    depth: usize,
    found: &mut Vec<String>,
) -> Result<(), StoreError> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(StoreError(format!("cannot list {prefix}: {error}"))),
    };
    for entry in entries {
        let entry = entry.map_err(|error| StoreError(format!("cannot list {prefix}: {error}")))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}/{name}");
        let kind = entry
            .file_type()
            .map_err(|error| StoreError(format!("cannot list {prefix}: {error}")))?;
        if kind.is_dir() && depth < MAX_LIST_DEPTH {
            list_folder(&entry.path(), &path, depth + 1, found)?;
        } else {
            found.push(path);
        }
    }
    Ok(())
}
