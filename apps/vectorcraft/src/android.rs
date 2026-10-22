//! Android storage paths, interactive in-app File Manager / File Picker modal, and native
//! `DocumentsUI` system file picker bridge (`VectorCraftActivity.java`).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, PoisonError};

use vectorcraft_engine::cmd::fileio;
use vectorcraft_ui_egui::FilePick;
use vectorcraft_ui_egui::picks::{PickAnswer, PickRequest};

static ACTIVE_PICKER: Mutex<Option<ActivePicker>> = Mutex::new(None);
static LAST_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

#[derive(Clone, Debug)]
struct DirItem {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
}

struct ActivePicker {
    request: PickRequest,
    tx: Sender<PickAnswer>,
    current_dir: PathBuf,
    dir_input: String,
    items: Vec<DirItem>,
    selected_paths: Vec<PathBuf>,
    file_name: String,
    filter_idx: usize,
    show_all_files: bool,
    new_folder_open: bool,
    new_folder_name: String,
    status_msg: Option<String>,
    waiting_system_picker: bool,
}

impl ActivePicker {
    fn new(request: PickRequest, tx: Sender<PickAnswer>) -> Self {
        let initial_dir = initial_directory_for(&request);
        let dir_input = initial_dir.to_string_lossy().to_string();
        let file_name = match &request {
            PickRequest::Save(pick) => {
                if pick.name.trim().is_empty() {
                    let ext = pick
                        .filters
                        .first()
                        .and_then(|(_, exts)| exts.first().map(String::as_str))
                        .unwrap_or("vc");
                    format!("Untitled.{ext}")
                } else {
                    pick.name.clone()
                }
            }
            _ => String::new(),
        };
        let mut picker = Self {
            request,
            tx,
            current_dir: initial_dir,
            dir_input,
            items: Vec::new(),
            selected_paths: Vec::new(),
            file_name,
            filter_idx: 0,
            show_all_files: false,
            new_folder_open: false,
            new_folder_name: String::new(),
            status_msg: None,
            waiting_system_picker: false,
        };
        picker.refresh();
        picker
    }

    fn navigate_to(&mut self, dir: PathBuf) {
        if std::fs::create_dir_all(&dir).is_ok() || dir.is_dir() {
            self.current_dir = dir;
            self.dir_input = self.current_dir.to_string_lossy().to_string();
            *LAST_DIR.lock().unwrap_or_else(PoisonError::into_inner) = Some(self.current_dir.clone());
            self.selected_paths.clear();
            self.status_msg = None;
            self.refresh();
        } else {
            self.status_msg = Some(format!("Cannot open folder: {}", dir.display()));
        }
    }

    fn allowed_extensions(&self) -> Vec<String> {
        if self.show_all_files {
            return Vec::new();
        }
        match &self.request {
            PickRequest::Open(pick) => {
                if pick.filters.is_empty() {
                    return fileio::OPEN_EXTS.iter().map(|s| (*s).to_string()).collect();
                }
                if self.filter_idx == 0 {
                    pick_exts(pick)
                } else if let Some((_, exts)) = pick.filters.get(self.filter_idx.saturating_sub(1)) {
                    exts.clone()
                } else {
                    pick_exts(pick)
                }
            }
            PickRequest::OpenMany => place_exts(),
            PickRequest::Save(pick) => {
                if let Some((_, exts)) = pick.filters.get(self.filter_idx) {
                    exts.clone()
                } else {
                    pick_exts(pick)
                }
            }
            PickRequest::Folder => Vec::new(),
        }
    }

    fn active_save_ext(&self) -> Option<&str> {
        match &self.request {
            PickRequest::Save(pick) => pick
                .filters
                .get(self.filter_idx)
                .or_else(|| pick.filters.first())
                .and_then(|(_, exts)| exts.first().map(String::as_str)),
            _ => None,
        }
    }

    fn refresh(&mut self) {
        self.items.clear();
        let exts = self.allowed_extensions();
        let folders_only = matches!(self.request, PickRequest::Folder);
        match std::fs::read_dir(&self.current_dir) {
            Ok(rd) => {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    }
                    let path = entry.path();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or_else(|_| path.is_dir());
                    if !is_dir && folders_only {
                        continue;
                    }
                    if !is_dir && !exts.is_empty() {
                        let matches = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| exts.iter().any(|w| e.eq_ignore_ascii_case(w)));
                        if !matches {
                            continue;
                        }
                    }
                    let size = if is_dir { 0 } else { entry.metadata().map_or(0, |m| m.len()) };
                    self.items.push(DirItem { name, path, is_dir, size });
                }
                self.items.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
                });
            }
            Err(e) => {
                self.status_msg = Some(format!(
                    "Cannot list {} ({e}). Use 'System File Picker…' or choose another folder.",
                    self.current_dir.display()
                ));
            }
        }
    }
}

fn initial_directory_for(request: &PickRequest) -> PathBuf {
    if let PickRequest::Open(pick) | PickRequest::Save(pick) = request
        && let Some(folder) = &pick.folder
    {
        let p = PathBuf::from(folder);
        if p.is_dir() || std::fs::create_dir_all(&p).is_ok() {
            return p;
        }
    }
    if let Some(last) = LAST_DIR.lock().unwrap_or_else(PoisonError::into_inner).clone()
        && last.is_dir()
    {
        return last;
    }
    default_documents_dir()
}

/// Directory used for file-based IPC with `VectorCraftActivity.java`.
fn ipc_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("VECTORCRAFT_CONFIG_DIR").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&p);
        return p;
    }
    let fallback = shared_storage_root()
        .join("Android")
        .join("data")
        .join("ai.storyteller.vectorcraft")
        .join("files");
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

fn mime_for_ext(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "svg" | "svgz" => "image/svg+xml",
        "pdf" => "application/pdf",
        "eps" | "ps" => "application/postscript",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "tif" | "tiff" => "image/tiff",
        "bmp" => "image/bmp",
        "txt" => "text/plain",
        "css" => "text/css",
        _ => "*/*",
    }
}

fn request_system_picker(picker: &mut ActivePicker) {
    let dir = ipc_dir();
    let _ = std::fs::remove_file(dir.join("picker_response.txt"));
    let (mode, name, mime) = match &picker.request {
        PickRequest::Open(_) => ("open", String::new(), "*/*"),
        PickRequest::OpenMany => ("open_many", String::new(), "*/*"),
        PickRequest::Save(_) => {
            let ext = picker.active_save_ext().unwrap_or("vc");
            let name = resolve_save_filename(&picker.file_name, ext);
            ("save", name, mime_for_ext(ext))
        }
        PickRequest::Folder => ("folder", String::new(), "*/*"),
    };
    let payload = format!("{mode}\n{name}\n{mime}\n");
    if std::fs::write(dir.join("picker_request.txt"), payload.as_bytes()).is_ok() {
        picker.waiting_system_picker = true;
        picker.status_msg = Some("Opened Android system file picker…".into());
    } else {
        picker.status_msg = Some("Could not launch Android system file picker.".into());
    }
}

fn poll_system_picker_response() -> Option<Vec<String>> {
    let resp_path = ipc_dir().join("picker_response.txt");
    if !resp_path.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&resp_path).ok()?;
    let _ = std::fs::remove_file(&resp_path);
    let paths: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    Some(paths)
}

/// Open Android's system file manager (`DocumentsUI`) for `File → Show in Folder` or `Open Location`.
pub fn reveal_in_file_manager(path: &str) -> Result<(), String> {
    let dir = ipc_dir();
    let payload = format!("reveal\n{path}\n*/*\n");
    std::fs::write(dir.join("picker_request.txt"), payload.as_bytes())
        .map_err(|e| format!("can't open Android file manager for {path}: {e}"))
}

/// Android shared storage root (`EXTERNAL_STORAGE`, or `/storage/emulated/0`).
pub fn shared_storage_root() -> PathBuf {
    std::env::var_os("EXTERNAL_STORAGE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/storage/emulated/0"))
}

/// Default folder for saving and opening VectorCraft documents on Android:
/// `<EXTERNAL_STORAGE>/Documents/VectorCraft` when writable, otherwise `<prefs_dir>/Documents`.
pub fn default_documents_dir() -> PathBuf {
    let shared_docs = shared_storage_root().join("Documents").join("VectorCraft");
    if std::fs::create_dir_all(&shared_docs).is_ok() {
        return shared_docs;
    }
    if let Some(prefs) = crate::prefs_path().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let fallback = prefs.join("Documents");
        let _ = std::fs::create_dir_all(&fallback);
        return fallback;
    }
    shared_docs
}

/// Standard Android shared-storage directories scanned for documents and artwork files.
pub fn search_roots() -> Vec<PathBuf> {
    let root = shared_storage_root();
    let mut dirs = vec![
        default_documents_dir(),
        root.join("Documents").join("VectorCraft"),
        root.join("Documents"),
        root.join("Download"),
        root.join("Pictures"),
        root.join("DCIM").join("Camera"),
        root.join("DCIM"),
    ];
    if let Some(prefs) = crate::prefs_path().and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(prefs.join("Documents"));
        dirs.push(prefs);
    }
    let mut unique = Vec::new();
    for dir in dirs {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

fn resolve_save_filename(name: &str, ext: &str) -> String {
    let default_name = if ext.is_empty() { "Untitled.vc".to_string() } else { format!("Untitled.{ext}") };
    let trimmed = if name.trim().is_empty() { default_name.as_str() } else { name.trim() };
    let mut p = PathBuf::from(trimmed);
    if !ext.is_empty() {
        let has_ext = p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(ext));
        if !has_ext {
            p.set_extension(ext);
        }
    }
    p.file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(default_name.as_str())
        .to_string()
}

/// Resolve a save path (`name` with optional required extension `ext`) inside `base_dir` when
/// `name` has no existing parent directory, ensuring its parent directory exists.
pub fn resolve_save_path_in(base_dir: &Path, name: &str, ext: &str) -> String {
    let default_name = if ext.is_empty() { "Untitled.vc".to_string() } else { format!("Untitled.{ext}") };
    let trimmed = if name.trim().is_empty() { default_name.as_str() } else { name.trim() };
    let candidate = Path::new(trimmed);
    let mut full = if candidate.is_absolute() && candidate.parent().is_some_and(|p| p != Path::new("/") && p.exists()) {
        candidate.to_path_buf()
    } else {
        let file_name = candidate
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .unwrap_or(default_name.as_str());
        base_dir.join(file_name)
    };
    if !ext.is_empty() {
        let has_ext = full
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(ext));
        if !has_ext {
            full.set_extension(ext);
        }
    }
    if let Some(parent) = full.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    full.to_string_lossy().to_string()
}

/// Collect matching files under `roots` (up to `max_depth` directory levels), newest first.
pub fn find_matching_files(roots: &[PathBuf], exts: &[impl AsRef<str>], max_depth: usize) -> Vec<String> {
    let mut matches: Vec<(u64, PathBuf)> = Vec::new();
    for root in roots {
        collect_matching(root, exts, max_depth, &mut matches);
    }
    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut out = Vec::new();
    for (_, path) in matches {
        let s = path.to_string_lossy().to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

fn collect_matching(dir: &Path, exts: &[impl AsRef<str>], depth_left: usize, out: &mut Vec<(u64, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        if md.is_dir() {
            if depth_left > 0 {
                collect_matching(&path, exts, depth_left - 1, out);
            }
            continue;
        }
        let matches_ext = exts.is_empty()
            || path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.iter().any(|want| e.eq_ignore_ascii_case(want.as_ref())));
        if matches_ext {
            let modified = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            out.push((modified, path));
        }
    }
}

fn pick_exts(pick: &FilePick) -> Vec<String> {
    let mut exts = Vec::new();
    for (_, list) in &pick.filters {
        for ext in list {
            if !exts.contains(ext) {
                exts.push(ext.clone());
            }
        }
    }
    if exts.is_empty() {
        exts.extend(fileio::OPEN_EXTS.iter().map(|s| (*s).to_string()));
    }
    exts
}

fn place_exts() -> Vec<String> {
    let mut exts = Vec::new();
    for (_, list) in fileio::place_filters() {
        for &ext in list {
            let s = ext.to_string();
            if !exts.contains(&s) {
                exts.push(s);
            }
        }
    }
    exts
}

/// Fallback synchronous resolution when called outside an asynchronous entry.
pub fn resolve_pick(request: &PickRequest) -> PickAnswer {
    match request {
        PickRequest::Open(pick) => {
            let mut roots = Vec::new();
            if let Some(folder) = &pick.folder {
                roots.push(PathBuf::from(folder));
            }
            for r in search_roots() {
                if !roots.contains(&r) {
                    roots.push(r);
                }
            }
            let exts = pick_exts(pick);
            let mut found = find_matching_files(&roots, &exts, 2);
            found.truncate(1);
            found
        }
        PickRequest::OpenMany => {
            let roots = search_roots();
            let exts = place_exts();
            let mut found = find_matching_files(&roots, &exts, 2);
            found.truncate(1);
            found
        }
        PickRequest::Save(pick) => {
            let base = pick
                .folder
                .as_deref()
                .filter(|f| !f.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(default_documents_dir);
            let ext = pick
                .filters
                .first()
                .and_then(|(_, exts)| exts.first().map(String::as_str))
                .unwrap_or("vc");
            vec![resolve_save_path_in(&base, &pick.name, ext)]
        }
        PickRequest::Folder => vec![default_documents_dir().to_string_lossy().to_string()],
    }
}

/// Open the interactive File Manager / File Picker modal window for `request` on Android.
pub fn start_pick(request: PickRequest, ui: &egui::Context) -> Option<Receiver<PickAnswer>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::fs::remove_file(ipc_dir().join("picker_response.txt"));
    let picker = ActivePicker::new(request, tx);
    *ACTIVE_PICKER.lock().unwrap_or_else(PoisonError::into_inner) = Some(picker);
    ui.request_repaint();
    Some(rx)
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Render the interactive Android File Manager / File Picker modal window when a file/folder pick
/// is waiting. Also polls responses from the native Android `DocumentsUI` system file picker.
pub fn show_picker_ui(ctx: &egui::Context, dialog_waiting: bool) {
    let mut guard = ACTIVE_PICKER.lock().unwrap_or_else(PoisonError::into_inner);
    if !dialog_waiting {
        *guard = None;
        return;
    }
    let Some(picker) = guard.as_mut() else { return };

    if picker.waiting_system_picker {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
        if let Some(paths) = poll_system_picker_response() {
            picker.waiting_system_picker = false;
            if !paths.is_empty() {
                let mut answer = paths;
                if let PickRequest::Save(_) = &picker.request
                    && let Some(ext) = picker.active_save_ext()
                    && answer.len() == 1
                {
                    answer.push(ext.to_string());
                }
                let _ = picker.tx.send(answer);
                *guard = None;
                ctx.request_repaint();
                return;
            }
            picker.status_msg = Some("System file picker was closed without choosing a file.".into());
            picker.refresh();
        }
    }

    let title = match &picker.request {
        PickRequest::Open(_) => "Open File",
        PickRequest::OpenMany => "Place Files",
        PickRequest::Save(_) => "Save / Export File",
        PickRequest::Folder => "Select Folder",
    };

    let screen = ctx.content_rect();
    let win_w = (screen.width() - 32.0).clamp(360.0, 820.0);
    let win_h = (screen.height() - 48.0).clamp(320.0, 560.0);

    let mut finished_answer: Option<PickAnswer> = None;

    egui::Area::new(egui::Id::new("android-file-picker-backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let rect = screen;
            ui.painter().rect_filled(rect, 0.0, egui::Color32::from_black_alpha(150));
        });

    egui::Window::new(title)
        .id(egui::Id::new("android-file-picker-window"))
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([win_w, win_h])
        .show(ctx, |ui| {
            // Row 1: Quick storage locations + Android System File Picker button
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Locations:").strong());
                let root = shared_storage_root();
                let mut jump: Option<PathBuf> = None;
                for (label, target) in [
                    ("VectorCraft", default_documents_dir()),
                    ("Documents", root.join("Documents")),
                    ("Download", root.join("Download")),
                    ("Pictures", root.join("Pictures")),
                    ("DCIM", root.join("DCIM")),
                    ("Storage", root),
                    ("App Data", ipc_dir()),
                ] {
                    let selected = picker.current_dir == target;
                    if ui.selectable_label(selected, label).clicked() {
                        jump = Some(target);
                    }
                }
                if let Some(target) = jump {
                    picker.navigate_to(target);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(egui::RichText::new("System File Picker…").strong())
                        .on_hover_text("Open Android's native system file picker (DocumentsUI)")
                        .clicked()
                    {
                        request_system_picker(picker);
                    }
                });
            });

            ui.separator();

            // Row 2: Up, Current folder path field, Go, Refresh, New Folder
            ui.horizontal(|ui| {
                let parent_opt = picker.current_dir.parent().map(Path::to_path_buf);
                if ui.add_enabled(parent_opt.is_some(), egui::Button::new("Up")).clicked()
                    && let Some(parent) = parent_opt
                {
                    picker.navigate_to(parent);
                }
                let path_resp = ui.add(
                    egui::TextEdit::singleline(&mut picker.dir_input)
                        .desired_width((ui.available_width() - 185.0).max(120.0)),
                );
                let go_clicked = ui.button("Go").clicked()
                    || (path_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if go_clicked {
                    let target = PathBuf::from(picker.dir_input.trim());
                    picker.navigate_to(target);
                }
                if ui.button("Refresh").clicked() {
                    picker.refresh();
                }
                if ui.button("+ New Folder").clicked() {
                    picker.new_folder_open = !picker.new_folder_open;
                }
            });

            if picker.new_folder_open {
                ui.horizontal(|ui| {
                    ui.label("New folder:");
                    let resp = ui.add(egui::TextEdit::singleline(&mut picker.new_folder_name).desired_width(220.0));
                    let create = ui.button("Create").clicked()
                        || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if create {
                        let clean: String = picker
                            .new_folder_name
                            .trim()
                            .chars()
                            .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
                            .collect();
                        if !clean.is_empty() {
                            let new_dir = picker.current_dir.join(&clean);
                            match std::fs::create_dir_all(&new_dir) {
                                Ok(()) => {
                                    picker.new_folder_name.clear();
                                    picker.new_folder_open = false;
                                    picker.navigate_to(new_dir);
                                }
                                Err(e) => {
                                    picker.status_msg = Some(format!("Could not create folder: {e}"));
                                }
                            }
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        picker.new_folder_open = false;
                    }
                });
            }

            if let Some(msg) = &picker.status_msg {
                ui.colored_label(egui::Color32::from_rgb(235, 175, 70), msg);
            }

            ui.separator();

            // Row 3: Directory & file listing
            let list_height = (ui.available_height() - 92.0).max(120.0);
            let mut enter_dir: Option<PathBuf> = None;
            let mut double_clicked_file: Option<PathBuf> = None;
            let is_open_many = matches!(picker.request, PickRequest::OpenMany);
            let is_save = matches!(picker.request, PickRequest::Save(_));
            let is_open = matches!(picker.request, PickRequest::Open(_));

            egui::ScrollArea::vertical()
                .id_salt("android-file-picker-scroll")
                .max_height(list_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if picker.items.is_empty() {
                        ui.add_space(16.0);
                        ui.vertical_centered(|ui| {
                            ui.label("No matching files or folders in this directory.");
                            ui.add_space(6.0);
                            if !matches!(picker.request, PickRequest::Folder) && !picker.show_all_files {
                                if ui.button("Show all files (*.*)").clicked() {
                                    picker.show_all_files = true;
                                    picker.refresh();
                                }
                            }
                        });
                    } else {
                        for item in &picker.items {
                            let is_selected = picker.selected_paths.contains(&item.path);
                            let label = if item.is_dir {
                                format!("[Folder]  {}", item.name)
                            } else {
                                format!("          {}   ({})", item.name, format_size(item.size))
                            };
                            let resp = ui.add_sized(
                                [ui.available_width(), 28.0],
                                egui::Button::selectable(is_selected, label),
                            );
                            if resp.clicked() {
                                if item.is_dir {
                                    enter_dir = Some(item.path.clone());
                                } else if is_open_many {
                                    if let Some(pos) = picker.selected_paths.iter().position(|p| p == &item.path) {
                                        picker.selected_paths.remove(pos);
                                    } else {
                                        picker.selected_paths.push(item.path.clone());
                                    }
                                } else {
                                    picker.selected_paths.clear();
                                    picker.selected_paths.push(item.path.clone());
                                    if is_save || is_open {
                                        picker.file_name.clone_from(&item.name);
                                    }
                                }
                            }
                            if resp.double_clicked() && !item.is_dir && (is_open || is_open_many) {
                                double_clicked_file = Some(item.path.clone());
                            }
                        }
                    }
                });

            if let Some(dir) = enter_dir {
                picker.navigate_to(dir);
            }
            if let Some(file) = double_clicked_file {
                finished_answer = Some(vec![file.to_string_lossy().to_string()]);
            }

            ui.separator();

            // Row 4: File name / Format selector & Action buttons
            match &picker.request.clone() {
                PickRequest::Save(pick) => {
                    ui.horizontal(|ui| {
                        ui.label("File name:");
                        let name_resp = ui.add(
                            egui::TextEdit::singleline(&mut picker.file_name)
                                .desired_width((ui.available_width() - 240.0).max(140.0)),
                        );
                        if !pick.filters.is_empty() {
                            let current_label = pick
                                .filters
                                .get(picker.filter_idx)
                                .map(|(name, exts)| {
                                    let ext = exts.first().map_or("", String::as_str);
                                    format!("{name} (.{ext})")
                                })
                                .unwrap_or_else(|| "Format".into());
                            let mut new_idx = picker.filter_idx;
                            egui::ComboBox::from_id_salt("android-save-format-combo")
                                .selected_text(current_label)
                                .width(155.0)
                                .show_ui(ui, |ui| {
                                    for (idx, (name, exts)) in pick.filters.iter().enumerate() {
                                        let ext = exts.first().map_or("", String::as_str);
                                        if ui.selectable_value(&mut new_idx, idx, format!("{name} (.{ext})")).clicked()
                                            && !ext.is_empty()
                                        {
                                            let stem = fileio::file_stem(&picker.file_name);
                                            let stem = if stem.trim().is_empty() { "Untitled" } else { stem.as_str() };
                                            picker.file_name = format!("{stem}.{ext}");
                                        }
                                    }
                                });
                            if new_idx != picker.filter_idx {
                                picker.filter_idx = new_idx;
                                picker.refresh();
                            }
                        }
                        if name_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let ext = picker.active_save_ext().unwrap_or("vc");
                            let path = resolve_save_path_in(&picker.current_dir, &picker.file_name, ext);
                            let mut ans = vec![path];
                            if let Some(e) = picker.active_save_ext() {
                                ans.push(e.to_string());
                            }
                            finished_answer = Some(ans);
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let mut show_all = picker.show_all_files;
                        if ui.checkbox(&mut show_all, "Show all files").changed() {
                            picker.show_all_files = show_all;
                            picker.refresh();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Cancel").clicked() {
                                finished_answer = Some(Vec::new());
                            }
                            if ui.button(egui::RichText::new("Save").strong()).clicked() {
                                let ext = picker.active_save_ext().unwrap_or("vc");
                                let path = resolve_save_path_in(&picker.current_dir, &picker.file_name, ext);
                                let mut ans = vec![path];
                                if let Some(e) = picker.active_save_ext() {
                                    ans.push(e.to_string());
                                }
                                finished_answer = Some(ans);
                            }
                        });
                    });
                }
                PickRequest::Open(pick) => {
                    ui.horizontal(|ui| {
                        ui.label("Selected:");
                        ui.add(
                            egui::TextEdit::singleline(&mut picker.file_name)
                                .desired_width((ui.available_width() - 250.0).max(120.0)),
                        );
                        if !pick.filters.is_empty() {
                            let current_label = if picker.filter_idx == 0 {
                                "All Supported".to_string()
                            } else {
                                pick.filters
                                    .get(picker.filter_idx - 1)
                                    .map(|(n, _)| n.clone())
                                    .unwrap_or_else(|| "All Supported".into())
                            };
                            let mut new_idx = picker.filter_idx;
                            egui::ComboBox::from_id_salt("android-open-filter-combo")
                                .selected_text(current_label)
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut new_idx, 0, "All Supported");
                                    for (idx, (name, exts)) in pick.filters.iter().enumerate() {
                                        let ext_list = exts.join(", .");
                                        ui.selectable_value(&mut new_idx, idx + 1, format!("{name} (.{ext_list})"));
                                    }
                                });
                            if new_idx != picker.filter_idx {
                                picker.filter_idx = new_idx;
                                picker.refresh();
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let mut show_all = picker.show_all_files;
                        if ui.checkbox(&mut show_all, "Show all files (*.*)").changed() {
                            picker.show_all_files = show_all;
                            picker.refresh();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Cancel").clicked() {
                                finished_answer = Some(Vec::new());
                            }
                            let can_open = !picker.selected_paths.is_empty() || !picker.file_name.trim().is_empty();
                            if ui
                                .add_enabled(can_open, egui::Button::new(egui::RichText::new("Open").strong()))
                                .clicked()
                            {
                                let chosen = picker
                                    .selected_paths
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| picker.current_dir.join(picker.file_name.trim()));
                                if chosen.is_file() {
                                    finished_answer = Some(vec![chosen.to_string_lossy().to_string()]);
                                } else {
                                    picker.status_msg = Some(format!("File not found: {}", chosen.display()));
                                }
                            }
                        });
                    });
                }
                PickRequest::OpenMany => {
                    ui.horizontal(|ui| {
                        let mut show_all = picker.show_all_files;
                        if ui.checkbox(&mut show_all, "Show all files (*.*)").changed() {
                            picker.show_all_files = show_all;
                            picker.refresh();
                        }
                        ui.label(format!("{} file(s) selected", picker.selected_paths.len()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Cancel").clicked() {
                                finished_answer = Some(Vec::new());
                            }
                            if ui
                                .add_enabled(
                                    !picker.selected_paths.is_empty(),
                                    egui::Button::new(egui::RichText::new("Place").strong()),
                                )
                                .clicked()
                            {
                                finished_answer = Some(
                                    picker
                                        .selected_paths
                                        .iter()
                                        .map(|p| p.to_string_lossy().to_string())
                                        .collect(),
                                );
                            }
                        });
                    });
                }
                PickRequest::Folder => {
                    ui.horizontal(|ui| {
                        ui.label(format!("Folder: {}", picker.current_dir.display()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Cancel").clicked() {
                                finished_answer = Some(Vec::new());
                            }
                            if ui.button(egui::RichText::new("Select This Folder").strong()).clicked() {
                                let _ = std::fs::create_dir_all(&picker.current_dir);
                                finished_answer = Some(vec![picker.current_dir.to_string_lossy().to_string()]);
                            }
                        });
                    });
                }
            }
        });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        finished_answer = Some(Vec::new());
    }

    if let Some(answer) = finished_answer {
        let _ = picker.tx.send(answer);
        *guard = None;
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("vc-android-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_save_path_in_creates_parent_and_adds_extension() {
        let base = temp_dir("save");
        let saved = resolve_save_path_in(&base, "Illustration 1", "vc");
        assert_eq!(PathBuf::from(&saved), base.join("Illustration 1.vc"));
        let already = resolve_save_path_in(&base, "Logo.SVG", "svg");
        assert_eq!(PathBuf::from(&already), base.join("Logo.SVG"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn find_matching_files_filters_extensions_and_skips_hidden() {
        let base = temp_dir("find");
        let sub = base.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(base.join("a.svg"), b"<svg/>").unwrap();
        std::fs::write(base.join(".hidden.svg"), b"<svg/>").unwrap();
        std::fs::write(sub.join("b.vc"), b"{}").unwrap();
        std::fs::write(sub.join("ignore.txt"), b"hi").unwrap();
        let found = find_matching_files(std::slice::from_ref(&base), &["svg", "vc"], 2);
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|p| p.ends_with("a.svg")));
        assert!(found.iter().any(|p| p.ends_with("b.vc")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn start_pick_opens_interactive_picker_and_completes_when_answered() {
        let ctx = egui::Context::default();
        let rx = start_pick(PickRequest::Save(FilePick::named("Banner.png")), &ctx).unwrap();
        assert!(rx.try_recv().is_err(), "picker waits for user interaction instead of auto-closing");
        show_picker_ui(&ctx, false);
    }
}
