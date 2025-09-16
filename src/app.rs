use crate::backend::{Backend, PassCliBackend, PassStatusError};
use crate::store::{build_store_index, path_to_store_key, EntryKind, StoreEntry};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum ModalAction {
    AddHere,
    DeleteSelected,
    Rename { from: String },
}

#[derive(Debug, Clone)]
pub enum Modal {
    Input {
        title: String,
        buffer: String,
        action: ModalAction,
    },
    Confirm {
        title: String,
        message: String,
        action: ModalAction,
        selected_ok: bool,
    },
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    Edit(String),
    Add(String),
    Delete,
    Rename { from: String, to: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMode {
    Raw,
    Qr,
}

type EntryIndex = usize;
type DirKey = String;

pub struct App {
    pub backend: Box<dyn Backend>,
    pub store_dir: PathBuf,
    pub cwd: PathBuf,
    pub entries: Vec<StoreEntry>,
    pub rows: Vec<ViewRow>,
    pub expanded: HashSet<DirKey>,
    pub cursor: usize,
    pub quit: bool,
    pub modal: Option<Modal>,
    pub pending: Option<PendingAction>,
    pub pending_preview: Option<(String, PreviewMode)>,

    pub filter: String,
    pub filter_mode: bool,
    pub filter_input: String,

    pub status: Option<String>,
    pub preview_key: Option<String>,
    pub preview_text: String,
    pub preview_is_error: bool,
    pub preview_mode: PreviewMode,
}

#[derive(Debug, Clone)]
pub struct ViewRow {
    pub idx: usize,          // index into entries
    pub branches: Vec<bool>, // for each level: is_last at that level
}

impl App {
    pub fn new_with_store(store_dir: Option<PathBuf>) -> Result<Self> {
        let store_dir = store_dir.unwrap_or_else(password_store_dir);
        if !store_dir.exists() {
            anyhow::bail!(
                "Password store not found: {}. Set PASSWORD_STORE_DIR or --store.",
                store_dir.display()
            );
        }
        let entries = build_store_index(&store_dir)?;
        let mut expanded = HashSet::new();
        expanded.insert(String::new()); // root expanded by default

        Ok(Self {
            backend: Box::new(PassCliBackend::new(Some(store_dir.clone()))),
            store_dir,
            cwd: PathBuf::new(),
            entries,
            rows: Vec::new(),
            expanded,
            cursor: 0,
            quit: false,
            modal: None,
            pending: None,
            pending_preview: None,
            filter: String::new(),
            filter_mode: false,
            filter_input: String::new(),
            status: None,
            preview_key: None,
            preview_text: String::new(),
            preview_is_error: false,
            preview_mode: PreviewMode::Raw,
        })
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.entries = build_store_index(&self.store_dir)?;
        self.apply_filter();
        Ok(())
    }

    pub fn apply_filter(&mut self) {
        let filter_active = !self.filter.is_empty();
        let mut include: HashSet<EntryIndex> = HashSet::new();
        let mut index_by_path: HashMap<PathBuf, EntryIndex> = HashMap::new();

        for (idx, entry) in self.entries.iter().enumerate() {
            index_by_path.insert(entry.path.clone(), idx);
            if !entry.path.starts_with(&self.cwd) || entry.path == self.cwd {
                continue;
            }
            if filter_active && !entry.display_name().contains(&self.filter) {
                continue;
            }
            include.insert(idx);
            if filter_active {
                self.add_visible_ancestors(idx, &mut include, &index_by_path);
            }
        }

        let mut children: BTreeMap<DirKey, Vec<EntryIndex>> = BTreeMap::new();
        for &idx in &include {
            let entry = &self.entries[idx];
            let relative = self.relative_to_cwd(&entry.path);
            if relative.as_os_str().is_empty() {
                continue;
            }
            let parent_key = relative.parent().map(path_to_store_key).unwrap_or_default();
            children.entry(parent_key).or_default().push(idx);
        }

        for siblings in children.values_mut() {
            siblings.sort_by(|&left, &right| self.cmp_entries(left, right));
        }

        self.rows.clear();
        let mut branch_stack = Vec::new();
        self.build_rows(&children, "", &mut branch_stack, filter_active);

        if self.cursor >= self.rows.len() {
            self.cursor = self.rows.len().saturating_sub(1);
        }
    }

    fn add_visible_ancestors(
        &self,
        idx: EntryIndex,
        include: &mut HashSet<EntryIndex>,
        index_by_path: &HashMap<PathBuf, EntryIndex>,
    ) {
        let mut current = self.entries[idx].path.as_path();
        while let Some(parent) = current.parent() {
            if parent == self.cwd.as_path() {
                break;
            }
            if let Some(&parent_idx) = index_by_path.get(parent) {
                include.insert(parent_idx);
            }
            current = parent;
        }
    }

    fn relative_to_cwd<'a>(&'a self, path: &'a Path) -> &'a Path {
        path.strip_prefix(&self.cwd).unwrap_or(path)
    }

    fn entry_key(&self, idx: EntryIndex) -> DirKey {
        let relative = self.relative_to_cwd(&self.entries[idx].path);
        path_to_store_key(relative)
    }

    fn cmp_entries(&self, left: EntryIndex, right: EntryIndex) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        let left_entry = &self.entries[left];
        let right_entry = &self.entries[right];
        match (left_entry.kind, right_entry.kind) {
            (EntryKind::Dir, EntryKind::Entry) => Ordering::Less,
            (EntryKind::Entry, EntryKind::Dir) => Ordering::Greater,
            _ => left_entry.path.cmp(&right_entry.path),
        }
    }

    fn build_rows(
        &mut self,
        children: &BTreeMap<DirKey, Vec<EntryIndex>>,
        parent: &str,
        branch_stack: &mut Vec<bool>,
        filter_active: bool,
    ) {
        if let Some(siblings) = children.get(parent) {
            for (pos, &idx) in siblings.iter().enumerate() {
                let is_last = pos + 1 == siblings.len();
                branch_stack.push(is_last);
                self.rows.push(ViewRow {
                    idx,
                    branches: branch_stack.clone(),
                });

                if self.entries[idx].kind == EntryKind::Dir {
                    let key = self.entry_key(idx);
                    if filter_active || self.expanded.contains(&key) {
                        self.build_rows(children, &key, branch_stack, filter_active);
                    }
                }

                branch_stack.pop();
            }
        }
    }

    pub fn enter(&mut self) {
        if let Some(row) = self.rows.get(self.cursor) {
            let entry = &self.entries[row.idx];
            if entry.is_dir() {
                let key = self.entry_key(row.idx);
                if self.expanded.contains(&key) {
                    self.expanded.remove(&key);
                } else {
                    self.expanded.insert(key);
                }
                self.apply_filter();
            }
        }
    }

    pub fn selected_entry_path(&self) -> Option<String> {
        self.rows
            .get(self.cursor)
            .and_then(|r| self.entries[r.idx].relative_entry_path())
    }

    pub fn delete_selected(&mut self) -> Result<()> {
        if let Some(row) = self.rows.get(self.cursor) {
            let entry = &self.entries[row.idx];
            if entry.is_dir() {
                let rel = entry.store_key();
                self.backend.rm(&rel, true)?;
            } else if let Some(rel) = entry.relative_entry_path() {
                self.backend.rm(&rel, false)?;
            }
            self.refresh()?;
        }
        Ok(())
    }

    pub fn open_add_modal(&mut self) {
        // Prefill with absolute path (within store). If hovering a directory, prefill "dir/".
        let mut prefix = String::new();
        if let Some(row) = self.rows.get(self.cursor) {
            let entry = &self.entries[row.idx];
            if entry.is_dir() {
                prefix = entry.store_key();
            } else if let Some(parent) = entry.path.parent() {
                prefix = path_to_store_key(parent);
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
        }
        self.modal = Some(Modal::Input {
            title: "New entry path".into(),
            buffer: prefix,
            action: ModalAction::AddHere,
        });
    }

    pub fn open_rename_modal(&mut self) {
        if let Some((from, suggested)) = self.selected_any_path_and_name() {
            self.modal = Some(Modal::Input {
                title: "Rename entry".into(),
                buffer: suggested,
                action: ModalAction::Rename { from },
            });
        }
    }

    pub fn open_delete_modal(&mut self) {
        self.modal = Some(Modal::Confirm {
            title: "Confirm Delete".into(),
            message: "Delete selected entry?".into(),
            action: ModalAction::DeleteSelected,
            selected_ok: true,
        });
    }

    pub fn submit_modal(&mut self) -> Option<PendingAction> {
        let modal = self.modal.take()?;
        match modal {
            Modal::Input { action, buffer, .. } => match action {
                ModalAction::AddHere => {
                    let name = buffer.trim();
                    if name.is_empty() {
                        None
                    } else {
                        Some(PendingAction::Add(name.to_string()))
                    }
                }
                ModalAction::DeleteSelected => None,
                ModalAction::Rename { from } => {
                    let to = buffer.trim();
                    if to.is_empty() || to == from {
                        return None;
                    }
                    if self.path_exists(to) {
                        self.status = Some(format!("Target '{}' exists — rename aborted", to));
                        None
                    } else {
                        Some(PendingAction::Rename {
                            from,
                            to: to.to_string(),
                        })
                    }
                }
            },
            Modal::Confirm {
                action,
                selected_ok,
                ..
            } => match action {
                ModalAction::DeleteSelected if selected_ok => Some(PendingAction::Delete),
                _ => None,
            },
        }
    }

    fn selected_any_path_and_name(&self) -> Option<(String, String)> {
        let row = self.rows.get(self.cursor)?;
        let entry = &self.entries[row.idx];
        if entry.is_dir() {
            let key = entry.store_key();
            Some((key.clone(), key))
        } else {
            entry.relative_entry_path().map(|rel| (rel.clone(), rel))
        }
    }

    fn path_exists(&self, rel: &str) -> bool {
        let p = self.store_dir.join(rel);
        if p.is_dir() {
            return true;
        }
        let mut f = p.clone();
        let _ = f.set_extension("gpg");
        f.is_file()
    }

    fn set_preview_state(&mut self, rel: String, text: String, is_error: bool, mode: PreviewMode) {
        self.preview_key = Some(rel);
        self.preview_text = text;
        self.preview_is_error = is_error;
        self.preview_mode = mode;
    }

    fn load_preview(&mut self, rel: String, mode: PreviewMode, allow_unlock: bool) -> Result<()> {
        let result = match mode {
            PreviewMode::Raw => self.backend.show(&rel),
            PreviewMode::Qr => self.backend.show_qr(&rel),
        };
        match result {
            Ok(text) => {
                self.pending_preview = None;
                self.set_preview_state(rel, text, false, mode);
                Ok(())
            }
            Err(err) => {
                if !allow_unlock {
                    if let Some(status_err) = err.downcast_ref::<PassStatusError>() {
                        if status_err.status.code() == Some(2) {
                            self.pending_preview = Some((rel.clone(), mode));
                            self.set_preview_state(
                                rel,
                                "GPG key locked. Prompting for passphrase...".to_string(),
                                true,
                                mode,
                            );
                            return Ok(());
                        }
                    }
                }
                let message = err.to_string();
                self.set_preview_state(rel, message.clone(), true, mode);
                Err(err)
            }
        }
    }

    pub fn take_pending_preview(&mut self) -> Option<(String, PreviewMode)> {
        self.pending_preview.take()
    }

    pub fn load_preview_after_unlock(&mut self, rel: String, mode: PreviewMode) -> Result<()> {
        self.load_preview(rel, mode, true)
    }

    pub fn update_preview(&mut self) {
        // Determine selected entry path (only files have content)
        let key = self.selected_entry_path();
        match key {
            Some(rel) => {
                if self.preview_key.as_deref() != Some(&rel)
                    || self.preview_mode != PreviewMode::Raw
                {
                    if let Err(err) = self.load_preview(rel.clone(), PreviewMode::Raw, false) {
                        self.status = Some(err.to_string());
                    }
                }
            }
            None => {
                // Directory selected or no selection
                self.preview_key = None;
                self.preview_text.clear();
                self.preview_is_error = false;
                self.preview_mode = PreviewMode::Raw;
                self.pending_preview = None;
            }
        }
    }

    pub fn update_preview_qr(&mut self) {
        let key = self.selected_entry_path();
        if let Some(rel) = key {
            if self.preview_key.as_deref() != Some(&rel) || self.preview_mode != PreviewMode::Qr {
                if let Err(err) = self.load_preview(rel.clone(), PreviewMode::Qr, false) {
                    self.status = Some(err.to_string());
                }
            }
        }
    }
}

fn password_store_dir() -> PathBuf {
    if let Ok(dir) = env::var("PASSWORD_STORE_DIR") {
        return PathBuf::from(dir);
    }
    let home = dirs_next::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".password-store")
}
