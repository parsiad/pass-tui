use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    Entry,
}

#[derive(Debug, Clone)]
pub struct StoreEntry {
    pub path: PathBuf, // path relative to store root, directories end without trailing slash
    pub kind: EntryKind,
}

impl StoreEntry {
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    }

    pub fn relative_entry_path(&self) -> Option<String> {
        match self.kind {
            EntryKind::Dir => None,
            EntryKind::Entry => Some(self.store_key()),
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }

    pub fn store_key(&self) -> String {
        path_to_store_key(&self.path)
    }
}

pub fn build_store_index(root: &Path) -> Result<Vec<StoreEntry>> {
    if !root.exists() {
        return Err(anyhow!("Password store not found: {}", root.display()));
    }

    let mut entries: Vec<StoreEntry> = Vec::new();

    // Always include the root as a directory with empty relative path
    entries.push(StoreEntry {
        path: PathBuf::new(),
        kind: EntryKind::Dir,
    });

    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path == root || path.file_name().map_or(false, |name| name == ".git") {
            continue;
        }

        let rel = match path.strip_prefix(root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };

        if entry.file_type().is_dir() {
            entries.push(StoreEntry {
                path: rel.to_path_buf(),
                kind: EntryKind::Dir,
            });
            continue;
        }

        if entry.file_type().is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("gpg")
        {
            let mut rel_no_ext = rel.to_path_buf();
            rel_no_ext.set_extension("");
            entries.push(StoreEntry {
                path: rel_no_ext,
                kind: EntryKind::Entry,
            });
        }
    }

    // Sort: directories first, then entries; lexicographic by relative path
    entries.sort_by(|a, b| match (a.kind, b.kind) {
        (EntryKind::Dir, EntryKind::Entry) => std::cmp::Ordering::Less,
        (EntryKind::Entry, EntryKind::Dir) => std::cmp::Ordering::Greater,
        _ => a.path.cmp(&b.path),
    });

    Ok(entries)
}

pub fn path_to_store_key(path: &Path) -> String {
    let mut key = String::new();
    for component in path.iter() {
        if !key.is_empty() {
            key.push('/');
        }
        let segment = component.to_string_lossy();
        key.push_str(&segment);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use std::fs;

    #[test]
    fn index_lists_dirs_and_entries() -> Result<()> {
        let tmp = TempDir::new()?;
        let root = tmp.path().join("store");
        fs::create_dir_all(root.join("a/b"))?;
        fs::create_dir_all(root.join("x"))?;
        fs::write(root.join("a/b/one.gpg"), b"dummy")?;
        fs::write(root.join("x/two.gpg"), b"dummy")?;
        fs::create_dir_all(root.join(".git"))?;
        fs::write(root.join(".git/ignore"), b"")?;

        let entries = build_store_index(&root)?;
        // Includes root dir (empty path), plus a, a/b, x, and two entries
        assert!(entries
            .iter()
            .any(|e| e.kind == EntryKind::Dir && e.path.as_os_str().is_empty()));
        assert!(entries
            .iter()
            .any(|e| e.kind == EntryKind::Dir && e.path == PathBuf::from("a")));
        assert!(entries
            .iter()
            .any(|e| e.kind == EntryKind::Dir && e.path == PathBuf::from("a/b")));
        assert!(entries
            .iter()
            .any(|e| e.kind == EntryKind::Entry && e.path == PathBuf::from("a/b/one")));
        assert!(entries
            .iter()
            .any(|e| e.kind == EntryKind::Entry && e.path == PathBuf::from("x/two")));
        Ok(())
    }
}
