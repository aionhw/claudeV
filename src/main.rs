mod vcd_parser;
use vcd_parser::{VcdData, ValueChange, SAMPLE_VCD, parse_vcd};

use std::collections::{HashSet, HashMap};
use std::error::Error;
use std::path::{Path, PathBuf};
use x11rb::connection::Connection;
use x11rb::protocol::{xproto::*, Event};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

type Res<T> = Result<T, Box<dyn Error>>;

// ── Keysyms ───────────────────────────────────────────────────────────────────
const XK_ESCAPE:    u32 = 0xFF1B;
const XK_TAB:       u32 = 0xFF09;
const XK_LEFT:      u32 = 0xFF51;
const XK_UP:        u32 = 0xFF52;
const XK_RIGHT:     u32 = 0xFF53;
const XK_DOWN:      u32 = 0xFF54;
const XK_PAGE_UP:   u32 = 0xFF55;
const XK_PAGE_DOWN: u32 = 0xFF56;
const XK_RETURN:    u32 = 0xFF0D;
const XK_DELETE:    u32 = 0xFFFF;
const XK_BACKSPACE: u32 = 0xFF08;

// ── Colours ───────────────────────────────────────────────────────────────────
const C_BG:         u32 = 0x10141B;
const C_WAVE_ALT:   u32 = 0x131924;
const C_PANEL:      u32 = 0x161D28;
const C_HEADER:     u32 = 0x0D1219;
const C_TOOLBAR:    u32 = 0x121925;
const C_OVERVIEW:   u32 = 0x0E1520;
const C_HI:         u32 = 0x3BE374;
const C_LO:         u32 = 0x1E8C52;
const C_X:          u32 = 0xFF8A4C;
const C_Z:          u32 = 0x56B6FF;
const C_BUS:        u32 = 0x48D597;
const C_CUR:        u32 = 0xFFD84D;
const C_LBL:        u32 = 0xD8E1EE;
const C_DIM:        u32 = 0x6C778A;
const C_BDR:        u32 = 0x263244;
const C_BDR_FOCUS:  u32 = 0x4AA3FF;
const C_SEL_MOD:    u32 = 0x1A2432;
const C_SEL_SIG:    u32 = 0x1C2838;
const C_SEL_WAVE:   u32 = 0x202B3A;
const C_RUL:        u32 = 0x8AA4C0;
const C_GRID:       u32 = 0x1D2734;
const C_MOD_BG:     u32 = 0x141B25;
const C_MOD_LBL:    u32 = 0x9AB0C8;
const C_MOD_SEL:    u32 = 0x68BCFF;
const C_BIT_LBL:    u32 = 0x7A8698;
const C_PINNED:     u32 = 0x7EE787;
const C_PATH:       u32 = 0x73839A;
const C_VALUE_BG:   u32 = 0x111722;
const C_OVERVIEW_WIN: u32 = 0x24527D;
const C_OVERVIEW_ALL: u32 = 0x2A3444;

// ── Layout ────────────────────────────────────────────────────────────────────
const LEFT_W:    i16 = 300;
const NAME_W:    i16 = 250;
const VALUE_W:   i16 = 92;
const ROW_H:     i16 = 24;
const SIG_H:     i16 = 24;
const WAVE_H:    i16 = 32;
const RULER_H:   i16 = 28;
const HEADER_H:  i16 = 20;
const TOOLBAR_H: i16 = 24;
const OVERVIEW_H:i16 = 28;
const TOP_H:     i16 = HEADER_H + TOOLBAR_H + OVERVIEW_H;
const MARKER_H:  i16 = 42;
const STATUS_H:  i16 = 18;
const FW:        i16 = 6;
const FA:        i16 = 10;
const INDENT:    i16 = 16;
const SB:        i16 = 12; // signal-list scrollbar thickness
const MOD_SPLIT: f32 = 0.35; // fraction of body height for module tree
const MENU_ITEMS: [&str; 8] = ["File", "Edit", "Search", "View", "Trace", "Tools", "Window", "Help"];
const FILE_MENU_ITEMS: [&str; 3] = ["Open", "Reload", "Exit"];
const ALL_SCOPE_PATH: &str = "__all_scopes__";
const ALL_SCOPE_LABEL: &str = "(all scopes)";

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragMode {
    None,
    LeftPanel,
    ModuleSplit,
    NameColumn,
    SigVScroll,
    SigHScroll,
    SigToWave,
}

// ── Module tree node ──────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
struct ModNode {
    path:     String,         // full dot-path e.g. "testbench.uut"
    name:     String,         // just "uut"
    depth:    usize,
    children: Vec<String>,    // child scope paths
    expanded: bool,
}

// ── Wave row ──────────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
enum WaveRow {
    Signal   { sig_idx: usize },
    BitSlice { sig_idx: usize, bit: usize },
}

// ── Focus ─────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq)]
enum Focus { ModTree, SigList, Wave }

#[derive(Clone, Copy, PartialEq)]
enum MenuAction {
    None,
    Quit,
}

#[derive(Clone, Debug)]
struct FileEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

// ── App ───────────────────────────────────────────────────────────────────────
struct App {
    vcd:           Option<VcdData>,
    filename:      String,
    file_path:     Option<String>,

    // Module tree
    mod_nodes:     HashMap<String, ModNode>, // path -> node
    mod_roots:     Vec<String>,              // top-level scope paths
    mod_rows:      Vec<String>,              // visible flattened paths (in order)
    mod_sel:       usize,                    // selected row in mod_rows
    mod_scroll:    usize,

    // Signal list (signals in selected module, direct children only)
    sig_rows:      Vec<usize>,               // sig_idx list for selected scope
    sig_sel:       usize,
    sig_scroll:    usize,
    sig_hscroll:   usize,

    // Waveform
    pinned:        Vec<usize>,
    wave_expanded: HashSet<usize>,
    wave_rows:     Vec<WaveRow>,
    wave_sel:      usize,
    wave_scroll:   usize,

    // View
    zoom:       f64,
    view_start: f64,
    cursor:     Option<f64>,
    focus:      Focus,
    status:     String,
    left_w:     i16,
    name_w:     i16,
    mod_split:  f32,
    filter_text:String,
    filter_edit:bool,
    drag_mode:  DragMode,
    drag_sig:   Option<usize>,
    markers:    [Option<f64>; 2],
    active_marker: usize,
    selected_menu: Option<usize>,
    file_browser_active: bool,
    file_dir: PathBuf,
    file_entries: Vec<FileEntry>,
    file_sel: usize,
    file_scroll: usize,
}

impl App {
    fn new() -> Self {
        App {
            vcd: None, filename: String::new(),
            file_path: None,
            mod_nodes: HashMap::new(), mod_roots: Vec::new(),
            mod_rows: Vec::new(), mod_sel: 0, mod_scroll: 0,
            sig_rows: Vec::new(), sig_sel: 0, sig_scroll: 0,
            sig_hscroll: 0,
            pinned: Vec::new(), wave_expanded: HashSet::new(),
            wave_rows: Vec::new(), wave_sel: 0, wave_scroll: 0,
            zoom: 1.0, view_start: 0.0, cursor: None,
            focus: Focus::ModTree,
            status: "Tab=switch focus  a=add signal  drag signal->wave  d=remove  +/-=zoom  q=quit".into(),
            left_w: LEFT_W,
            name_w: NAME_W,
            mod_split: MOD_SPLIT,
            filter_text: String::new(),
            filter_edit: false,
            drag_mode: DragMode::None,
            drag_sig: None,
            markers: [None, None],
            active_marker: 0,
            selected_menu: None,
            file_browser_active: false,
            file_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            file_entries: Vec::new(),
            file_sel: 0,
            file_scroll: 0,
        }
    }

    fn max_time(&self) -> f64 { self.vcd.as_ref().map(|v| v.max_time as f64).unwrap_or(100.0) }
    fn view_end(&self)   -> f64 { self.view_start + self.max_time() / self.zoom }

    fn clamp_view(&mut self) {
        let r = self.max_time() / self.zoom;
        self.view_start = self.view_start.clamp(0.0, (self.max_time() - r).max(0.0));
    }

    fn wave_vis_rows(&self, win_h: i16) -> usize {
        ((win_h - TOP_H - STATUS_H - MARKER_H - RULER_H) / WAVE_H).max(1) as usize
    }

    fn mod_vis_rows(&self, mod_panel_h: i16) -> usize {
        ((mod_panel_h - ROW_H) / ROW_H).max(1) as usize   // subtract header
    }

    fn sig_vis_rows(&self, sig_panel_h: i16) -> usize {
        ((sig_panel_h - ROW_H - SB) / SIG_H).max(1) as usize
    }

    fn left_split_x(&self) -> i16 { self.left_w }
    fn name_split_x(&self) -> i16 { self.left_w + self.name_w }

    fn set_left_w(&mut self, x: i16, win_w: i16) {
        let max_left = (win_w - VALUE_W - 220).max(220);
        self.left_w = x.clamp(180, max_left);
        self.clamp_sig_hscroll();
    }

    fn set_name_w(&mut self, x: i16, win_w: i16) {
        let max_name = (win_w - self.left_w - VALUE_W - 120).max(120);
        self.name_w = (x - self.left_w).clamp(140, max_name);
    }

    fn set_mod_split_from_y(&mut self, y: i16, win_h: i16) {
        let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
        if body_h <= 0 { return; }
        let rel = (y - TOP_H).clamp(80, body_h - 80);
        self.mod_split = (rel as f32 / body_h as f32).clamp(0.18, 0.82);
    }

    // ── Load ──────────────────────────────────────────────────────────────────
    fn load_text(&mut self, text: &str, name: &str) {
        match parse_vcd(text) {
            Ok(data) => {
                let (n, mt, ts) = (data.signals.len(), data.max_time, data.timescale.clone());
                self.filename = name.to_string();
                self.zoom = 1.0; self.view_start = 0.0; self.cursor = None;
                self.markers = [None, None];
                self.active_marker = 0;
                self.mod_sel = 0; self.mod_scroll = 0;
                self.sig_sel = 0; self.sig_scroll = 0;
                self.wave_sel = 0; self.wave_scroll = 0;
                self.wave_expanded.clear();
                self.pinned.clear();
                self.build_mod_tree(&data);
                self.vcd = Some(data);
                self.rebuild_mod_rows();
                self.rebuild_sig_rows();
                self.rebuild_wave();
                self.status = format!("Loaded '{}'  signals={}  end={}{}  pinned=0", name, n, mt, ts);
            }
            Err(e) => self.status = format!("Parse error: {}", e),
        }
    }

    fn focus_name(&self) -> &'static str {
        match self.focus {
            Focus::ModTree => "browser",
            Focus::SigList => "objects",
            Focus::Wave => "wave",
        }
    }

    fn load_file(&mut self, path: &str) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.file_path = Some(path.to_string());
                let fname = Path::new(path)
                    .file_name().unwrap_or_default().to_string_lossy().to_string();
                self.load_text(&text, &fname);
                self.file_browser_active = false;
            }
            Err(e) => self.status = format!("File error: {}", e),
        }
    }

    fn file_vis_rows(&self, win_h: i16) -> usize {
        let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
        ((body_h - ROW_H) / ROW_H).max(1) as usize
    }

    fn refresh_file_entries(&mut self) {
        let mut rows = Vec::new();
        if let Some(parent) = self.file_dir.parent() {
            rows.push(FileEntry {
                name: "..".into(),
                path: parent.to_path_buf(),
                is_dir: true,
            });
        }

        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.file_dir) {
            for item in rd.flatten() {
                let path = item.path();
                let name = item.file_name().to_string_lossy().to_string();
                if name.is_empty() {
                    continue;
                }
                let is_dir = path.is_dir();
                if !is_dir {
                    let is_vcd = path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext.eq_ignore_ascii_case("vcd"))
                        .unwrap_or(false);
                    if !is_vcd {
                        continue;
                    }
                }
                let row = FileEntry { name, path, is_dir };
                if is_dir {
                    dirs.push(row);
                } else {
                    files.push(row);
                }
            }
        }
        dirs.sort_by(|a, b| a.name.cmp(&b.name));
        files.sort_by(|a, b| a.name.cmp(&b.name));
        rows.extend(dirs);
        rows.extend(files);

        self.file_entries = rows;
        self.file_sel = self.file_sel.min(self.file_entries.len().saturating_sub(1));
        self.file_scroll = self.file_scroll.min(self.file_entries.len().saturating_sub(1));
    }

    fn resolve_loaded_file_dir(&self) -> Option<PathBuf> {
        let path = Path::new(self.file_path.as_deref()?);
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().ok()?.join(path)
        };
        Some(abs.parent()?.to_path_buf())
    }

    fn select_loaded_file_entry(&mut self) {
        let Some(path) = self.file_path.as_deref() else { return };
        let Some(name) = Path::new(path).file_name().and_then(|n| n.to_str()) else { return };
        if let Some(idx) = self
            .file_entries
            .iter()
            .position(|entry| !entry.is_dir && entry.name == name)
        {
            self.file_sel = idx;
            self.file_scroll = self.file_sel.saturating_sub(1);
        }
    }

    fn update_file_browser_status(&mut self) {
        if let Some(entry) = self.file_entries.get(self.file_sel) {
            self.status = format!("Open: {}  selected {}", self.file_dir.display(), entry.name);
        } else {
            self.status = format!("Open: {}", self.file_dir.display());
        }
    }

    fn open_file_panel(&mut self) {
        self.file_dir = self
            .resolve_loaded_file_dir()
            .unwrap_or_else(|| self.file_dir.clone());
        self.file_browser_active = true;
        self.file_sel = 0;
        self.file_scroll = 0;
        self.refresh_file_entries();
        self.select_loaded_file_entry();
        self.update_file_browser_status();
    }

    fn open_selected_file_entry(&mut self) {
        if let Some(entry) = self.file_entries.get(self.file_sel).cloned() {
            if entry.is_dir {
                self.file_dir = entry.path;
                self.file_sel = 0;
                self.file_scroll = 0;
                self.refresh_file_entries();
                self.update_file_browser_status();
            } else if let Some(path) = entry.path.to_str() {
                self.load_file(path);
            } else {
                self.status = "File error: invalid path".into();
            }
        }
    }

    fn scroll_file(&mut self, win_h: i16) {
        let vis = self.file_vis_rows(win_h);
        if self.file_sel < self.file_scroll {
            self.file_scroll = self.file_sel;
        } else if self.file_sel >= self.file_scroll + vis {
            self.file_scroll = self.file_sel + 1 - vis;
        }
    }

    fn hit_file_browser_row(&self, mouse_x: i16, mouse_y: i16, win_h: i16) -> Option<usize> {
        if !self.file_browser_active || mouse_x < 0 || mouse_x >= self.left_w {
            return None;
        }
        let marker_y = win_h - STATUS_H - MARKER_H;
        if mouse_y < TOP_H + ROW_H || mouse_y >= marker_y {
            return None;
        }
        let vis_row = ((mouse_y - (TOP_H + ROW_H)) / ROW_H) as usize;
        let row_idx = self.file_scroll + vis_row;
        if row_idx < self.file_entries.len() {
            Some(row_idx)
        } else {
            None
        }
    }

    fn reload_file(&mut self) {
        if let Some(path) = self.file_path.clone() {
            self.load_file(&path);
        } else {
            self.status = "Reload unavailable: no file loaded".into();
        }
    }

    // ── Build module tree from VCD signals ────────────────────────────────────
    fn build_mod_tree(&mut self, vcd: &VcdData) {
        self.mod_nodes.clear();
        self.mod_roots.clear();

        // Collect all unique scope paths from signal full_names
        let mut seen: HashSet<String> = HashSet::new();
        for sig in &vcd.signals {
            let parts: Vec<&str> = sig.scope.split('.').filter(|p| !p.is_empty()).collect();
            for depth in 1..=parts.len() {
                let path = parts[..depth].join(".");
                if seen.insert(path.clone()) {
                    let name  = parts[depth - 1].to_string();
                    let parent = if depth > 1 { Some(parts[..depth-1].join(".")) } else { None };
                    let node  = ModNode { path: path.clone(), name, depth: depth - 1,
                                          children: Vec::new(), expanded: true };
                    self.mod_nodes.insert(path.clone(), node);
                    if let Some(p) = parent {
                        if let Some(pn) = self.mod_nodes.get_mut(&p) {
                            if !pn.children.contains(&path) { pn.children.push(path); }
                        }
                    } else {
                        if !self.mod_roots.contains(&path) { self.mod_roots.push(path); }
                    }
                }
            }
        }

        // If no scopes at all, create a virtual root
        if self.mod_roots.is_empty() && !vcd.signals.is_empty() {
            let root = ModNode { path: "".to_string(), name: "(top)".to_string(),
                                  depth: 0, children: Vec::new(), expanded: true };
            self.mod_nodes.insert("".to_string(), root);
            self.mod_roots.push("".to_string());
        }

        // Synthetic selector for viewing/pinning all signals.
        self.mod_nodes.insert(
            ALL_SCOPE_PATH.to_string(),
            ModNode {
                path: ALL_SCOPE_PATH.to_string(),
                name: ALL_SCOPE_LABEL.to_string(),
                depth: 0,
                children: Vec::new(),
                expanded: false,
            },
        );
        self.mod_roots.push(ALL_SCOPE_PATH.to_string());
    }

    // ── Flatten visible module rows ───────────────────────────────────────────
    fn rebuild_mod_rows(&mut self) {
        self.mod_rows.clear();
        let roots = self.mod_roots.clone();
        for r in &roots { self.flatten_mod(r); }
        if self.mod_sel >= self.mod_rows.len() {
            self.mod_sel = self.mod_rows.len().saturating_sub(1);
        }
    }

    fn flatten_mod(&mut self, path: &str) {
        self.mod_rows.push(path.to_string());
        let children = self.mod_nodes.get(path)
            .map(|n| if n.expanded { n.children.clone() } else { Vec::new() })
            .unwrap_or_default();
        for child in children { self.flatten_mod(&child); }
    }

    // ── Signals directly in selected module ───────────────────────────────────
    fn selected_scope(&self) -> Option<String> {
        self.mod_rows.get(self.mod_sel).cloned()
    }

    fn is_all_scope(scope: &str) -> bool {
        scope == ALL_SCOPE_PATH
    }

    fn scope_label(&self, scope: &str) -> String {
        if Self::is_all_scope(scope) {
            ALL_SCOPE_LABEL.to_string()
        } else if scope.is_empty() {
            "(top)".to_string()
        } else {
            scope.to_string()
        }
    }

    fn selected_scope_label(&self) -> Option<String> {
        self.selected_scope().map(|scope| self.scope_label(&scope))
    }

    fn rebuild_sig_rows(&mut self) {
        self.sig_rows.clear();
        let Some(vcd) = &self.vcd else { return };
        let scope = match self.selected_scope() {
            Some(s) => s,
            None    => return,
        };
        let show_all = Self::is_all_scope(&scope);
        for (si, sig) in vcd.signals.iter().enumerate() {
            let matches_filter = self.filter_text.is_empty()
                || sig.name.contains(&self.filter_text)
                || sig.full_name.contains(&self.filter_text);
            if (show_all || sig.scope == scope) && matches_filter {
                self.sig_rows.push(si);
            }
        }
        self.sig_sel    = 0;
        self.sig_scroll = 0;
        self.sig_hscroll = 0;
    }

    fn sig_view_cols(&self) -> usize {
        ((self.left_w - SB - 16) / FW).max(1) as usize
    }

    fn sig_content_cols(&self) -> usize {
        let Some(vcd) = &self.vcd else { return 0 };
        self.sig_rows
            .iter()
            .map(|&si| {
                let sig = &vcd.signals[si];
                let mut cols = sig.name.chars().count();
                if sig.width > 1 {
                    cols += format!("[{}:0]", sig.width - 1).chars().count();
                }
                cols
            })
            .max()
            .unwrap_or(0)
    }

    fn sig_max_hscroll(&self) -> usize {
        self.sig_content_cols().saturating_sub(self.sig_view_cols())
    }

    fn clamp_sig_hscroll(&mut self) {
        self.sig_hscroll = self.sig_hscroll.min(self.sig_max_hscroll());
    }

    fn scroll_sig_h_by(&mut self, delta: i32) {
        let max_h = self.sig_max_hscroll() as i32;
        let next = (self.sig_hscroll as i32 + delta).clamp(0, max_h);
        self.sig_hscroll = next as usize;
    }

    fn set_sig_vscroll_from_mouse(&mut self, mouse_y: i16, win_h: i16) {
        let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
        let mod_h = (body_h as f32 * self.mod_split) as i16;
        let sig_h = body_h - mod_h;
        let sig_y = TOP_H + mod_h;
        let rows_y = sig_y + ROW_H;
        let rows_h = (sig_h - ROW_H - SB).max(1);
        let vis_rows = self.sig_vis_rows(sig_h);
        let max_scroll = self.sig_rows.len().saturating_sub(vis_rows);
        if max_scroll == 0 {
            self.sig_scroll = 0;
            return;
        }
        let rel = (mouse_y - rows_y).clamp(0, rows_h - 1) as f64 / rows_h as f64;
        self.sig_scroll = (rel * max_scroll as f64).round() as usize;
    }

    fn set_sig_hscroll_from_mouse(&mut self, mouse_x: i16) {
        let track_w = (self.left_w - SB).max(1);
        let max_scroll = self.sig_max_hscroll();
        if max_scroll == 0 {
            self.sig_hscroll = 0;
            return;
        }
        let rel = mouse_x.clamp(0, track_w - 1) as f64 / track_w as f64;
        self.sig_hscroll = (rel * max_scroll as f64).round() as usize;
    }

    // ── Pinning ───────────────────────────────────────────────────────────────
    fn is_pinned(&self, si: usize) -> bool { self.pinned.contains(&si) }

    fn pin(&mut self, si: usize) {
        if !self.is_pinned(si) { self.pinned.push(si); self.rebuild_wave(); }
    }

    fn unpin(&mut self, si: usize) {
        self.pinned.retain(|&s| s != si);
        self.wave_expanded.remove(&si);
        self.rebuild_wave();
    }

    fn toggle_pin(&mut self, si: usize) {
        if self.is_pinned(si) { self.unpin(si); } else { self.pin(si); }
    }

    fn pin_scope(&mut self, scope: &str) {
        let Some(vcd) = &self.vcd else { return };
        let show_all = Self::is_all_scope(scope);
        let to_add: Vec<usize> = vcd.signals.iter().enumerate()
            .filter(|(_, sig)| {
                show_all
                    || scope.is_empty()
                    || sig.scope == scope
                    || (!scope.is_empty() && sig.scope.starts_with(scope) && sig.scope.as_bytes().get(scope.len()) == Some(&b'.'))
            })
            .map(|(i, _)| i).collect();
        for si in to_add { if !self.is_pinned(si) { self.pinned.push(si); } }
        self.rebuild_wave();
    }

    // ── Wave rows ─────────────────────────────────────────────────────────────
    fn rebuild_wave(&mut self) {
        let Some(vcd) = &self.vcd else { self.wave_rows.clear(); return };
        self.wave_rows.clear();
        for &si in &self.pinned {
            self.wave_rows.push(WaveRow::Signal { sig_idx: si });
            if self.wave_expanded.contains(&si) {
                for bit in (0..vcd.signals[si].width).rev() {
                    self.wave_rows.push(WaveRow::BitSlice { sig_idx: si, bit });
                }
            }
        }
        self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
    }

    // ── Scroll helpers ────────────────────────────────────────────────────────
    fn scroll_mod(&mut self, win_h: i16) {
        let mod_h  = ((win_h - TOP_H - STATUS_H - MARKER_H) as f32 * self.mod_split) as i16;
        let vr     = self.mod_vis_rows(mod_h);
        if self.mod_sel < self.mod_scroll { self.mod_scroll = self.mod_sel; }
        else if self.mod_sel >= self.mod_scroll + vr { self.mod_scroll = self.mod_sel + 1 - vr; }
    }

    fn scroll_sig(&mut self, win_h: i16) {
        let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
        let mod_h  = (body_h as f32 * self.mod_split) as i16;
        let sig_h  = body_h - mod_h;
        let vr     = self.sig_vis_rows(sig_h);
        if self.sig_sel < self.sig_scroll { self.sig_scroll = self.sig_sel; }
        else if self.sig_sel >= self.sig_scroll + vr { self.sig_scroll = self.sig_sel + 1 - vr; }
        self.clamp_sig_hscroll();
    }

    fn scroll_wave(&mut self, win_h: i16) {
        let vr = self.wave_vis_rows(win_h);
        if self.wave_sel < self.wave_scroll { self.wave_scroll = self.wave_sel; }
        else if self.wave_sel >= self.wave_scroll + vr { self.wave_scroll = self.wave_sel + 1 - vr; }
    }

    // ── Edge jump ─────────────────────────────────────────────────────────────
    fn jump_edge(&mut self, fwd: bool) {
        let (Some(vcd), Some(cursor)) = (&self.vcd, self.cursor) else { return };
        let si = match self.wave_rows.get(self.wave_sel) {
            Some(WaveRow::Signal   { sig_idx }) |
            Some(WaveRow::BitSlice { sig_idx, .. }) => *sig_idx,
            None => return,
        };
        let Some(changes) = vcd.changes.get(&vcd.signals[si].id) else { return };
        let t = cursor as u64;
        let nt = if fwd {
            changes.iter().find(|vc| vc.time > t).map(|vc| vc.time as f64)
        } else {
            changes.iter().rev().find(|vc| vc.time < t).map(|vc| vc.time as f64)
        };
        if let Some(nt) = nt {
            self.cursor = Some(nt);
            if nt > self.view_end()  { self.view_start = (nt - self.max_time()/self.zoom*0.1).max(0.0); self.clamp_view(); }
            if nt < self.view_start  { self.view_start = (nt - self.max_time()/self.zoom*0.1).max(0.0); }
        }
    }

    fn zoom_by(&mut self, factor: f64, pivot: Option<f64>) {
        let oz = self.zoom;
        self.zoom = (self.zoom * factor).clamp(1.0, 100_000.0);
        if (self.zoom - oz).abs() < 1e-9 { return; }
        let p    = pivot.unwrap_or(self.view_start + self.max_time() / oz / 2.0);
        let frac = (p - self.view_start) / (self.max_time() / oz);
        self.view_start = p - frac * (self.max_time() / self.zoom);
        self.clamp_view();
    }

    fn fit_all(&mut self) { self.zoom = 1.0; self.view_start = 0.0; }

    fn select_menu(&mut self, menu_idx: usize) {
        if let Some(label) = MENU_ITEMS.get(menu_idx) {
            self.selected_menu = if self.selected_menu == Some(menu_idx) {
                None
            } else {
                Some(menu_idx)
            };
            self.status = format!("Menu: {}", label);
        }
    }

    fn body_layout(&self, win_h: i16) -> (i16, i16, i16, i16) {
        let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
        let mod_h = (body_h as f32 * self.mod_split) as i16;
        let sig_y = TOP_H + mod_h;
        let marker_y = win_h - STATUS_H - MARKER_H;
        (body_h, mod_h, sig_y, marker_y)
    }

    fn hit_menu(&self, mouse_x: i16, mouse_y: i16) -> Option<usize> {
        if !(0..HEADER_H).contains(&mouse_y) {
            return None;
        }
        let mut x = 6;
        for (idx, label) in MENU_ITEMS.iter().enumerate() {
            let item_w = tw(label) + 16;
            if mouse_x >= x && mouse_x < x + item_w {
                return Some(idx);
            }
            x += item_w;
        }
        None
    }

    fn file_menu_geometry(&self) -> Option<(i16, i16, i16, i16)> {
        if self.selected_menu != Some(0) {
            return None;
        }
        let x = 2;
        let y = HEADER_H;
        let w = FILE_MENU_ITEMS.iter().map(|item| tw(item)).max().unwrap_or(0) + 20;
        let h = FILE_MENU_ITEMS.len() as i16 * ROW_H;
        Some((x, y, w, h))
    }

    fn hit_file_menu(&self, mouse_x: i16, mouse_y: i16) -> Option<usize> {
        let (x, y, w, h) = self.file_menu_geometry()?;
        if mouse_x < x || mouse_x >= x + w || mouse_y < y || mouse_y >= y + h {
            return None;
        }
        Some(((mouse_y - y) / ROW_H) as usize)
    }

    fn activate_file_menu(&mut self, item_idx: usize) -> MenuAction {
        self.selected_menu = None;
        match item_idx {
            0 => {
                self.open_file_panel();
                MenuAction::None
            }
            1 => {
                self.reload_file();
                MenuAction::None
            }
            2 => MenuAction::Quit,
            _ => MenuAction::None,
        }
    }

    fn select_mod_row(&mut self, row_idx: usize, win_h: i16) {
        if row_idx >= self.mod_rows.len() {
            return;
        }
        self.focus = Focus::ModTree;
        self.mod_sel = row_idx;
        self.scroll_mod(win_h);
        self.rebuild_sig_rows();
        if let Some(path) = self.selected_scope() {
            self.status = format!("Module: {}", self.scope_label(&path));
        }
    }

    fn toggle_selected_scope_expanded(&mut self) {
        if let Some(path) = self.selected_scope() {
            if let Some(node) = self.mod_nodes.get_mut(&path) {
                if !node.children.is_empty() {
                    node.expanded = !node.expanded;
                }
            }
            self.rebuild_mod_rows();
            self.mod_sel = self.mod_sel.min(self.mod_rows.len().saturating_sub(1));
        }
    }

    fn select_sig_row(&mut self, row_idx: usize, win_h: i16) {
        if row_idx >= self.sig_rows.len() {
            return;
        }
        self.focus = Focus::SigList;
        self.sig_sel = row_idx;
        self.scroll_sig(win_h);
        if let Some(vcd) = &self.vcd {
            let sig = &vcd.signals[self.sig_rows[row_idx]];
            self.status = format!("Signal: {}", sig.full_name);
        }
    }

    fn hit_sig_row(&self, mouse_x: i16, mouse_y: i16, win_h: i16) -> Option<usize> {
        if mouse_x < 0 || mouse_x >= self.left_w - SB {
            return None;
        }
        let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
        let mod_h = (body_h as f32 * self.mod_split) as i16;
        let sig_h = body_h - mod_h;
        let sig_y = TOP_H + mod_h;
        let rows_y = sig_y + ROW_H;
        let rows_h = (sig_h - ROW_H - SB).max(1);
        if mouse_y < rows_y || mouse_y >= rows_y + rows_h {
            return None;
        }
        let vis_row = ((mouse_y - rows_y) / SIG_H) as usize;
        let row_idx = self.sig_scroll + vis_row;
        (row_idx < self.sig_rows.len()).then_some(row_idx)
    }

    fn hit_sig_vscroll(&self, mouse_x: i16, mouse_y: i16, win_h: i16) -> bool {
        let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
        let mod_h = (body_h as f32 * self.mod_split) as i16;
        let sig_h = body_h - mod_h;
        let sig_y = TOP_H + mod_h;
        let rows_y = sig_y + ROW_H;
        let rows_h = (sig_h - ROW_H - SB).max(1);
        mouse_x >= self.left_w - SB && mouse_x < self.left_w && mouse_y >= rows_y && mouse_y < rows_y + rows_h
    }

    fn hit_sig_hscroll(&self, mouse_x: i16, mouse_y: i16, win_h: i16) -> bool {
        let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
        let mod_h = (body_h as f32 * self.mod_split) as i16;
        let sig_h = body_h - mod_h;
        let sig_y = TOP_H + mod_h;
        let bar_y = sig_y + sig_h - SB;
        mouse_x >= 0 && mouse_x < self.left_w - SB && mouse_y >= bar_y && mouse_y < bar_y + SB
    }

    fn select_wave_row(&mut self, row_idx: usize, win_h: i16) {
        if row_idx >= self.wave_rows.len() {
            return;
        }
        self.focus = Focus::Wave;
        self.wave_sel = row_idx;
        self.scroll_wave(win_h);
        if let Some(vcd) = &self.vcd {
            let label = match self.wave_rows.get(row_idx) {
                Some(WaveRow::Signal { sig_idx }) => vcd.signals[*sig_idx].full_name.clone(),
                Some(WaveRow::BitSlice { sig_idx, bit }) => format!("{}[{}]", vcd.signals[*sig_idx].full_name, bit),
                None => String::new(),
            };
            if !label.is_empty() {
                self.status = format!("◆ {}", label);
            }
        }
    }

    fn select_wave_signal(&mut self, sig_idx: usize, win_h: i16) {
        if let Some(row_idx) = self
            .wave_rows
            .iter()
            .position(|row| matches!(row, WaveRow::Signal { sig_idx: s } if *s == sig_idx))
        {
            self.select_wave_row(row_idx, win_h);
        }
    }

    fn toggle_wave_row_expanded(&mut self, row_idx: usize) {
        match self.wave_rows.get(row_idx).cloned() {
            Some(WaveRow::Signal { sig_idx }) => {
                if let Some(vcd) = &self.vcd {
                    if vcd.signals[sig_idx].width > 1 {
                        if self.wave_expanded.contains(&sig_idx) {
                            self.wave_expanded.remove(&sig_idx);
                        } else {
                            self.wave_expanded.insert(sig_idx);
                        }
                        self.rebuild_wave();
                        self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
                    }
                }
            }
            Some(WaveRow::BitSlice { sig_idx, .. }) => {
                self.wave_expanded.remove(&sig_idx);
                self.rebuild_wave();
                self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
            }
            None => {}
        }
    }

    fn set_cursor_from_wave_x(&mut self, mouse_x: i16, win_w: i16) {
        let wave_x = self.left_w + self.name_w + VALUE_W;
        let wave_w = (win_w - wave_x).max(1) as f64;
        let rel = (mouse_x - wave_x).clamp(0, wave_w as i16 - 1) as f64;
        let frac = rel / wave_w;
        self.cursor = Some(self.view_start + frac * (self.max_time() / self.zoom));
    }

    // ── Key handler ───────────────────────────────────────────────────────────
    fn handle_keysym(&mut self, ks: u32, win_h: i16) -> bool {
        if self.filter_edit {
            match ks {
                k if k == XK_ESCAPE => {
                    self.filter_edit = false;
                    self.status = format!("Filter: {}", self.filter_text);
                    return false;
                }
                k if k == XK_RETURN => {
                    self.filter_edit = false;
                    self.rebuild_sig_rows();
                    self.status = format!("Filter applied: {}", self.filter_text);
                    return false;
                }
                k if k == XK_BACKSPACE => {
                    self.filter_text.pop();
                    self.rebuild_sig_rows();
                    self.status = format!("Filter: {}", self.filter_text);
                    return false;
                }
                _ => {
                    if (0x20..=0x7E).contains(&ks) {
                        self.filter_text.push(char::from_u32(ks).unwrap_or_default());
                        self.rebuild_sig_rows();
                        self.status = format!("Filter: {}", self.filter_text);
                    }
                    return false;
                }
            }
        }

        if self.file_browser_active {
            let n = self.file_entries.len();
            match ks {
                k if k == XK_ESCAPE => {
                    self.file_browser_active = false;
                    self.status = "Open canceled".into();
                    return false;
                }
                k if k == XK_UP || k == 0x6B => {
                    if self.file_sel > 0 {
                        self.file_sel -= 1;
                    }
                    self.scroll_file(win_h);
                    self.update_file_browser_status();
                    return false;
                }
                k if k == XK_DOWN || k == 0x6A => {
                    if n > 0 && self.file_sel + 1 < n {
                        self.file_sel += 1;
                    }
                    self.scroll_file(win_h);
                    self.update_file_browser_status();
                    return false;
                }
                k if k == XK_PAGE_UP => {
                    let vr = self.file_vis_rows(win_h);
                    self.file_sel = self.file_sel.saturating_sub(vr);
                    self.scroll_file(win_h);
                    self.update_file_browser_status();
                    return false;
                }
                k if k == XK_PAGE_DOWN => {
                    let vr = self.file_vis_rows(win_h);
                    self.file_sel = (self.file_sel + vr).min(n.saturating_sub(1));
                    self.scroll_file(win_h);
                    self.update_file_browser_status();
                    return false;
                }
                k if k == XK_RETURN || k == 0x65 => {
                    self.open_selected_file_entry();
                    return false;
                }
                _ => return false,
            }
        }

        // Global
        match ks {
            k if k == 0x71 || k == 0x51 || k == XK_ESCAPE => return true,
            0x73 | 0x53 => { self.load_text(SAMPLE_VCD, "sample.vcd"); return false; }
            0x2F => {
                self.filter_edit = true;
                self.status = format!("Filter edit: {}", self.filter_text);
                return false;
            }
            0x58 => {
                self.filter_text.clear();
                self.rebuild_sig_rows();
                self.status = "Filter cleared".into();
                return false;
            }
            k if k == XK_TAB => {
                self.focus = match self.focus {
                    Focus::ModTree => Focus::SigList,
                    Focus::SigList => Focus::Wave,
                    Focus::Wave    => Focus::ModTree,
                };
                return false;
            }
            0x2B | 0x3D => { self.zoom_by(2.0, None); return false; }
            0x2D | 0x5F => { self.zoom_by(0.5, None); return false; }
            0x30 | 0x66 | 0x46 => { self.fit_all(); return false; }
            k if k == XK_LEFT  || k == 0x68 => { self.view_start = (self.view_start - self.max_time()/self.zoom*0.1).max(0.0); return false; }
            k if k == XK_RIGHT || k == 0x6C => { self.view_start += self.max_time()/self.zoom*0.1; self.clamp_view(); return false; }
            0x48 => { self.view_start = 0.0; return false; }
            0x4C => { self.view_start = (self.max_time() - self.max_time()/self.zoom).max(0.0); return false; }
            0x63 => { self.cursor = Some(self.view_start + (self.max_time()/self.zoom)/2.0); return false; }
            0x43 => { self.cursor = None; return false; }
            0x6D => {
                let t = self.cursor.unwrap_or(self.view_start + (self.max_time()/self.zoom)/2.0);
                self.markers[self.active_marker] = Some(t);
                self.status = format!("Marker {} set to {:.0}", if self.active_marker == 0 { "A" } else { "B" }, t);
                return false;
            }
            0x4D => {
                self.active_marker = (self.active_marker + 1) % 2;
                self.status = format!("Active marker: {}", if self.active_marker == 0 { "A" } else { "B" });
                return false;
            }
            0x44 => {
                self.markers = [None, None];
                self.status = "Markers cleared".into();
                return false;
            }
            0x5B => { if let Some(t) = self.cursor { self.cursor = Some((t - self.max_time()/self.zoom*0.02).max(0.0)); } return false; }
            0x5D => { if let Some(t) = self.cursor { self.cursor = Some((t + self.max_time()/self.zoom*0.02).min(self.max_time())); } return false; }
            0x6E => { self.jump_edge(true);  return false; }
            0x4E => { self.jump_edge(false); return false; }
            0x3F => {
                self.status = "Tab=focus | /=filter X=clear | m=set marker M=next D=clear | MOD j/k Enter A | SIG j/k a A | WAVE j/k J/K d e | VIEW +/- f h/l c n/N q".into();
                return false;
            }
            _ => {}
        }
        match self.focus {
            Focus::ModTree => self.key_mod(ks, win_h),
            Focus::SigList => self.key_sig(ks, win_h),
            Focus::Wave    => self.key_wave(ks, win_h),
        }
        false
    }

    fn key_mod(&mut self, ks: u32, win_h: i16) {
        let n = self.mod_rows.len();
        match ks {
            k if k == XK_UP   || k == 0x6B => { if self.mod_sel > 0 { self.mod_sel -= 1; } self.scroll_mod(win_h); self.rebuild_sig_rows(); }
            k if k == XK_DOWN || k == 0x6A => { if n > 0 && self.mod_sel + 1 < n { self.mod_sel += 1; } self.scroll_mod(win_h); self.rebuild_sig_rows(); }
            k if k == XK_PAGE_UP   => {
                let mod_h = ((win_h - TOP_H - STATUS_H - MARKER_H) as f32 * self.mod_split) as i16;
                let vr = self.mod_vis_rows(mod_h);
                self.mod_sel = self.mod_sel.saturating_sub(vr); self.scroll_mod(win_h); self.rebuild_sig_rows();
            }
            k if k == XK_PAGE_DOWN => {
                let mod_h = ((win_h - TOP_H - STATUS_H - MARKER_H) as f32 * self.mod_split) as i16;
                let vr = self.mod_vis_rows(mod_h);
                self.mod_sel = (self.mod_sel + vr).min(n.saturating_sub(1)); self.scroll_mod(win_h); self.rebuild_sig_rows();
            }
            // Enter — expand/collapse
            k if k == XK_RETURN || k == 0x65 => {
                if let Some(path) = self.selected_scope() {
                    if let Some(node) = self.mod_nodes.get_mut(&path) {
                        if !node.children.is_empty() {
                            node.expanded = !node.expanded;
                        }
                    }
                    self.rebuild_mod_rows();
                }
            }
            // A — add all signals in module (recursive)
            0x41 => {
                if let Some(path) = self.selected_scope() {
                    let p = path.clone();
                    self.pin_scope(&p);
                    self.status = format!("Added all in: {}", self.scope_label(&p));
                }
            }
            _ => {}
        }
        if let Some(p) = self.selected_scope() {
            self.status = format!("Module: {}", self.scope_label(&p));
        }
    }

    fn key_sig(&mut self, ks: u32, win_h: i16) {
        let n = self.sig_rows.len();
        match ks {
            k if k == XK_UP   || k == 0x6B => { if self.sig_sel > 0 { self.sig_sel -= 1; } self.scroll_sig(win_h); }
            k if k == XK_DOWN || k == 0x6A => { if n > 0 && self.sig_sel + 1 < n { self.sig_sel += 1; } self.scroll_sig(win_h); }
            k if k == XK_PAGE_UP   => {
                let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
                let mod_h  = (body_h as f32 * self.mod_split) as i16;
                let vr     = self.sig_vis_rows(body_h - mod_h);
                self.sig_sel = self.sig_sel.saturating_sub(vr); self.scroll_sig(win_h);
            }
            k if k == XK_PAGE_DOWN => {
                let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
                let mod_h  = (body_h as f32 * self.mod_split) as i16;
                let vr     = self.sig_vis_rows(body_h - mod_h);
                self.sig_sel = (self.sig_sel + vr).min(n.saturating_sub(1)); self.scroll_sig(win_h);
            }
            // a / Enter — toggle pin
            k if k == XK_RETURN || k == 0x61 => {
                if let Some(&si) = self.sig_rows.get(self.sig_sel) {
                    self.toggle_pin(si);
                    let fname = self.vcd.as_ref().map(|v| v.signals[si].full_name.clone()).unwrap_or_default();
                    self.status = format!("{}: {}", if self.is_pinned(si) {"Added"} else {"Removed"}, fname);
                }
            }
            // A — add all signals in current module
            0x41 => {
                if let Some(scope) = self.selected_scope() {
                    let p = scope.clone();
                    self.pin_scope(&p);
                    self.status = format!("Added all in: {}", self.scope_label(&p));
                }
            }
            _ => {}
        }
    }

    fn key_wave(&mut self, ks: u32, win_h: i16) {
        let n = self.wave_rows.len();
        match ks {
            k if k == XK_UP   || k == 0x6B => { if self.wave_sel > 0 { self.wave_sel -= 1; } self.scroll_wave(win_h); }
            k if k == XK_DOWN || k == 0x6A => { if n > 0 && self.wave_sel + 1 < n { self.wave_sel += 1; } self.scroll_wave(win_h); }
            k if k == XK_PAGE_UP   => { let vr = self.wave_vis_rows(win_h); self.wave_sel = self.wave_sel.saturating_sub(vr); self.scroll_wave(win_h); }
            k if k == XK_PAGE_DOWN => { let vr = self.wave_vis_rows(win_h); self.wave_sel = (self.wave_sel + vr).min(n.saturating_sub(1)); self.scroll_wave(win_h); }
            // d / Del / Backspace — remove
            k if k == 0x64 || k == XK_DELETE || k == XK_BACKSPACE => {
                match self.wave_rows.get(self.wave_sel).cloned() {
                    Some(WaveRow::Signal { sig_idx }) => { self.unpin(sig_idx); self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1)); }
                    Some(WaveRow::BitSlice { sig_idx, .. }) => { self.wave_expanded.remove(&sig_idx); self.rebuild_wave(); self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1)); }
                    None => {}
                }
            }
            // J — move down
            0x4A => {
                if let Some(WaveRow::Signal { sig_idx }) = self.wave_rows.get(self.wave_sel).cloned() {
                    if let Some(pos) = self.pinned.iter().position(|&s| s == sig_idx) {
                        if pos + 1 < self.pinned.len() {
                            self.pinned.swap(pos, pos+1); self.rebuild_wave();
                            if let Some(p) = self.wave_rows.iter().position(|r| matches!(r, WaveRow::Signal{sig_idx:s} if *s==sig_idx)) { self.wave_sel = p; }
                        }
                    }
                }
            }
            // K — move up
            0x4B => {
                if let Some(WaveRow::Signal { sig_idx }) = self.wave_rows.get(self.wave_sel).cloned() {
                    if let Some(pos) = self.pinned.iter().position(|&s| s == sig_idx) {
                        if pos > 0 {
                            self.pinned.swap(pos, pos-1); self.rebuild_wave();
                            if let Some(p) = self.wave_rows.iter().position(|r| matches!(r, WaveRow::Signal{sig_idx:s} if *s==sig_idx)) { self.wave_sel = p; }
                        }
                    }
                }
            }
            // e / Enter — expand bus
            k if k == XK_RETURN || k == 0x65 || k == 0x45 => {
                match self.wave_rows.get(self.wave_sel).cloned() {
                    Some(WaveRow::Signal { sig_idx }) => {
                        if let Some(vcd) = &self.vcd {
                            if vcd.signals[sig_idx].width > 1 {
                                if self.wave_expanded.contains(&sig_idx) { self.wave_expanded.remove(&sig_idx); }
                                else { self.wave_expanded.insert(sig_idx); }
                                self.rebuild_wave();
                            }
                        }
                    }
                    Some(WaveRow::BitSlice { sig_idx, .. }) => { self.wave_expanded.remove(&sig_idx); self.rebuild_wave(); }
                    None => {}
                }
            }
            _ => {
                // Show path
                if let Some(vcd) = &self.vcd {
                    let s = match self.wave_rows.get(self.wave_sel) {
                        Some(WaveRow::Signal{sig_idx})       => vcd.signals[*sig_idx].full_name.clone(),
                        Some(WaveRow::BitSlice{sig_idx,bit}) => format!("{}[{}]", vcd.signals[*sig_idx].full_name, bit),
                        None => String::new(),
                    };
                    if !s.is_empty() { self.status = format!("◆ {}", s); }
                }
            }
        }
    }

    fn handle_button(&mut self, button: u8, state: KeyButMask, mouse_x: i16, mouse_y: i16, win_w: i16, win_h: i16) -> MenuAction {
        let (_, _, sig_y, marker_y) = self.body_layout(win_h);
        if button == 1 {
            self.drag_sig = None;
            if let Some(item_idx) = self.hit_file_menu(mouse_x, mouse_y) {
                return self.activate_file_menu(item_idx);
            }
            if let Some(menu_idx) = self.hit_menu(mouse_x, mouse_y) {
                self.select_menu(menu_idx);
                return MenuAction::None;
            }
            if (mouse_x - self.left_split_x()).abs() <= 3 {
                self.drag_mode = DragMode::LeftPanel;
                self.selected_menu = None;
                return MenuAction::None;
            }
            if (mouse_x - self.name_split_x()).abs() <= 3 {
                self.drag_mode = DragMode::NameColumn;
                self.selected_menu = None;
                return MenuAction::None;
            }
            let split_y = sig_y;
            if mouse_x < self.left_w && (mouse_y - split_y).abs() <= 3 {
                self.drag_mode = DragMode::ModuleSplit;
                self.selected_menu = None;
                return MenuAction::None;
            }

            self.selected_menu = None;

            if self.file_browser_active {
                if let Some(row_idx) = self.hit_file_browser_row(mouse_x, mouse_y, win_h) {
                    self.file_sel = row_idx;
                    self.scroll_file(win_h);
                    self.open_selected_file_entry();
                }
                return MenuAction::None;
            }

            if mouse_y >= TOP_H && mouse_y < marker_y {
                if mouse_x < self.left_w {
                    if mouse_y < sig_y {
                        self.focus = Focus::ModTree;
                        if mouse_y >= TOP_H + ROW_H {
                            let vis_row = ((mouse_y - (TOP_H + ROW_H)) / ROW_H) as usize;
                            let row_idx = self.mod_scroll + vis_row;
                            if row_idx < self.mod_rows.len() {
                                self.select_mod_row(row_idx, win_h);
                                let depth = self.mod_nodes.get(&self.mod_rows[row_idx]).map(|n| n.depth).unwrap_or(0);
                                let marker_x = depth as i16 * INDENT + 4 + FW * 2;
                                if mouse_x <= marker_x {
                                    self.toggle_selected_scope_expanded();
                                }
                            }
                        }
                        return MenuAction::None;
                    }

                    self.focus = Focus::SigList;
                    if self.hit_sig_vscroll(mouse_x, mouse_y, win_h) {
                        self.drag_mode = DragMode::SigVScroll;
                        self.set_sig_vscroll_from_mouse(mouse_y, win_h);
                        return MenuAction::None;
                    }
                    if self.hit_sig_hscroll(mouse_x, mouse_y, win_h) {
                        self.drag_mode = DragMode::SigHScroll;
                        self.set_sig_hscroll_from_mouse(mouse_x);
                        return MenuAction::None;
                    }
                    if let Some(row_idx) = self.hit_sig_row(mouse_x, mouse_y, win_h) {
                        self.select_sig_row(row_idx, win_h);
                        if let Some(&sig_idx) = self.sig_rows.get(row_idx) {
                            self.drag_sig = Some(sig_idx);
                            self.drag_mode = DragMode::SigToWave;
                        }
                    }
                    return MenuAction::None;
                }

                self.focus = Focus::Wave;
                if mouse_y < TOP_H + RULER_H {
                    if mouse_x >= self.left_w + self.name_w + VALUE_W {
                        self.set_cursor_from_wave_x(mouse_x, win_w);
                    }
                    return MenuAction::None;
                }

                let vis_row = ((mouse_y - (TOP_H + RULER_H)) / WAVE_H) as usize;
                let row_idx = self.wave_scroll + vis_row;
                if row_idx < self.wave_rows.len() {
                    self.select_wave_row(row_idx, win_h);
                    if mouse_x >= self.left_w + self.name_w - FW * 3 && mouse_x < self.left_w + self.name_w {
                        self.toggle_wave_row_expanded(row_idx);
                    } else if mouse_x >= self.left_w + self.name_w + VALUE_W {
                        self.set_cursor_from_wave_x(mouse_x, win_w);
                    }
                }
                return MenuAction::None;
            }
        }

        // Wheel in signal list: vertical scroll (4/5), horizontal (shift+4/5 or 6/7).
        let in_sig_panel = mouse_x >= 0 && mouse_x < self.left_w && mouse_y >= sig_y && mouse_y < marker_y;
        if in_sig_panel {
            let shift = state.contains(KeyButMask::SHIFT);
            let body_h = win_h - TOP_H - STATUS_H - MARKER_H;
            let mod_h = (body_h as f32 * self.mod_split) as i16;
            let sig_h = body_h - mod_h;
            let vis_rows = self.sig_vis_rows(sig_h);
            let max_scroll = self.sig_rows.len().saturating_sub(vis_rows);
            match button {
                4 if shift => self.scroll_sig_h_by(-4),
                5 if shift => self.scroll_sig_h_by(4),
                6 => self.scroll_sig_h_by(-4),
                7 => self.scroll_sig_h_by(4),
                4 => self.sig_scroll = self.sig_scroll.saturating_sub(1),
                5 => self.sig_scroll = (self.sig_scroll + 1).min(max_scroll),
                _ => {}
            }
            if matches!(button, 4 | 5 | 6 | 7) {
                return MenuAction::None;
            }
        }

        let wx = self.left_w + self.name_w + VALUE_W;
        let in_wave_panel = mouse_x >= wx && mouse_y >= TOP_H && mouse_y < marker_y;
        if in_wave_panel {
            let ww = (win_w - wx).max(1) as f64;
            let rel = (mouse_x - wx).max(0);
            let frac = (rel as f64).clamp(0.0, ww - 1.0) / ww;
            let pivot = self.view_start + frac * (self.max_time() / self.zoom);
            match button {
                4 => self.zoom_by(2.0, Some(pivot)),
                5 => self.zoom_by(0.5, Some(pivot)),
                _ => {}
            }
        }
        MenuAction::None
    }

    fn handle_motion(&mut self, mouse_x: i16, mouse_y: i16, win_w: i16, win_h: i16) {
        match self.drag_mode {
            DragMode::None => {}
            DragMode::LeftPanel => self.set_left_w(mouse_x, win_w),
            DragMode::NameColumn => self.set_name_w(mouse_x, win_w),
            DragMode::ModuleSplit => self.set_mod_split_from_y(mouse_y, win_h),
            DragMode::SigVScroll => self.set_sig_vscroll_from_mouse(mouse_y, win_h),
            DragMode::SigHScroll => self.set_sig_hscroll_from_mouse(mouse_x),
            DragMode::SigToWave => {}
        }
    }

    fn handle_button_release(&mut self, mouse_x: i16, mouse_y: i16, win_h: i16) {
        if self.drag_mode == DragMode::SigToWave {
            if let Some(sig_idx) = self.drag_sig {
                let marker_y = win_h - STATUS_H - MARKER_H;
                let drop_in_wave = mouse_x >= self.left_w && mouse_y >= TOP_H + RULER_H && mouse_y < marker_y;
                if drop_in_wave {
                    self.pin(sig_idx);
                    self.select_wave_signal(sig_idx, win_h);
                    if let Some(vcd) = &self.vcd {
                        self.status = format!("Added by drag: {}", vcd.signals[sig_idx].full_name);
                    }
                }
            }
        }
        self.drag_mode = DragMode::None;
        self.drag_sig = None;
    }
}

// ── X11 primitives ────────────────────────────────────────────────────────────

fn fill(conn: &RustConnection, d: u32, gc: u32, color: u32, x: i16, y: i16, w: u16, h: u16) {
    if w == 0 || h == 0 { return; }
    let _ = conn.change_gc(gc, &ChangeGCAux::new().foreground(color).fill_style(FillStyle::SOLID));
    let _ = conn.poly_fill_rectangle(d, gc, &[Rectangle { x, y, width: w, height: h }]);
}

fn seg(conn: &RustConnection, d: u32, gc: u32, color: u32, x1: i16, y1: i16, x2: i16, y2: i16) {
    let _ = conn.change_gc(gc, &ChangeGCAux::new().foreground(color).line_style(LineStyle::SOLID));
    let _ = conn.poly_segment(d, gc, &[Segment { x1, y1, x2, y2 }]);
}

fn segs(conn: &RustConnection, d: u32, gc: u32, color: u32, s: &[Segment]) {
    if s.is_empty() { return; }
    let _ = conn.change_gc(gc, &ChangeGCAux::new().foreground(color).line_style(LineStyle::SOLID));
    let _ = conn.poly_segment(d, gc, s);
}

fn dashed(conn: &RustConnection, d: u32, gc: u32, color: u32, x1: i16, y: i16, x2: i16) {
    if x2 <= x1 { return; }
    let _ = conn.change_gc(gc, &ChangeGCAux::new().foreground(color).line_style(LineStyle::ON_OFF_DASH));
    let _ = conn.set_dashes(gc, 0, &[4u8, 3u8]);
    let _ = conn.poly_segment(d, gc, &[Segment { x1, y1: y, x2, y2: y }]);
    let _ = conn.change_gc(gc, &ChangeGCAux::new().line_style(LineStyle::SOLID));
}

fn txt(conn: &RustConnection, d: u32, gc: u32, font: u32, fg: u32, bg: u32, x: i16, y: i16, s: &str) {
    if s.is_empty() { return; }
    let _ = conn.change_gc(gc, &ChangeGCAux::new().foreground(fg).background(bg).font(font));
    let _ = conn.image_text8(d, gc, x, y + FA, &s.bytes().take(255).collect::<Vec<u8>>());
}

fn tw(s: &str) -> i16 { s.len() as i16 * FW }

fn trunc_l(s: &str, max_c: usize) -> String {
    if s.len() <= max_c { s.to_string() } else { format!("~{}", &s[s.len()-max_c+1..]) }
}

fn trunc_r(s: &str, max_c: usize) -> String {
    if s.len() <= max_c { s.to_string() } else { s[..max_c].to_string() }
}

fn slice_text(s: &str, start: usize, max_c: usize) -> String {
    s.chars().skip(start).take(max_c).collect()
}

fn fmt_val(val: &str, width: usize) -> String {
    if width == 1 { return val.to_uppercase(); }
    if val.chars().any(|c| c=='x'||c=='X') { return "X".into(); }
    match u64::from_str_radix(val, 2) {
        Ok(n) => format!("{:#X}", n),
        Err(_) => val[..val.len().min(8)].to_string(),
    }
}

fn render_overview(conn: &RustConnection, pix: u32, gc: u32, font: u32,
    x: i16, y: i16, w: i16, h: i16, app: &App) {
    fill(conn, pix, gc, C_OVERVIEW_ALL, x, y, w as u16, h as u16);
    if app.vcd.is_none() {
        txt(conn, pix, gc, font, C_DIM, C_OVERVIEW_ALL, x+4, y+4, "Overview");
        return;
    }
    let max_t = app.max_time().max(1.0);
    let vx0 = x + ((app.view_start / max_t) * w as f64).clamp(0.0, w as f64 - 1.0) as i16;
    let vx1 = x + ((app.view_end() / max_t) * w as f64).clamp(0.0, w as f64) as i16;
    fill(conn, pix, gc, C_OVERVIEW_WIN, vx0, y+2, (vx1-vx0).max(2) as u16, (h-4).max(2) as u16);
    seg(conn, pix, gc, C_BDR_FOCUS, vx0, y+1, vx0, y+h-2);
    seg(conn, pix, gc, C_BDR_FOCUS, vx1, y+1, vx1, y+h-2);
    if let Some(ct) = app.cursor {
        let cx = x + ((ct / max_t) * w as f64).clamp(0.0, w as f64 - 1.0) as i16;
        seg(conn, pix, gc, C_CUR, cx, y, cx, y+h-1);
    }
    txt(conn, pix, gc, font, C_RUL, C_OVERVIEW, x+4, y+4, "Overview");
    let right = format!("0 .. {:.0}", max_t);
    txt(conn, pix, gc, font, C_DIM, C_OVERVIEW, (x + w - tw(&right) - 4).max(x+4), y+4, &right);
}

fn fmt_marker(vcd: &VcdData, t: Option<f64>) -> String {
    match t {
        Some(tv) => format!("{:.0}{}", tv, vcd.timescale),
        None => "--".into(),
    }
}

fn render_marker_dock(conn: &RustConnection, pix: u32, gc: u32, font: u32,
    x: i16, y: i16, w: i16, app: &App) {
    fill(conn, pix, gc, C_TOOLBAR, x, y, w as u16, MARKER_H as u16);
    seg(conn, pix, gc, C_BDR, x, y, x + w, y);
    let Some(vcd) = &app.vcd else {
        txt(conn, pix, gc, font, C_DIM, C_TOOLBAR, x+6, y+6, "Markers");
        return;
    };
    let a = app.markers[0];
    let b = app.markers[1];
    let c = app.cursor;
    let delta_ab = match (a, b) {
        (Some(ta), Some(tb)) => format!("{:.0}{}", (tb - ta).abs(), vcd.timescale),
        _ => "--".into(),
    };
    let delta_ca = match (c, a) {
        (Some(tc), Some(ta)) => format!("{:.0}{}", (tc - ta).abs(), vcd.timescale),
        _ => "--".into(),
    };
    let active = if app.active_marker == 0 { "A" } else { "B" };
    txt(conn, pix, gc, font, C_MOD_SEL, C_TOOLBAR, x+6, y+6, &format!("Markers  active {}", active));
    txt(conn, pix, gc, font, if app.active_marker == 0 { C_CUR } else { C_LBL }, C_TOOLBAR, x+6, y+20, &format!("A  {}", fmt_marker(vcd, a)));
    txt(conn, pix, gc, font, if app.active_marker == 1 { C_CUR } else { C_LBL }, C_TOOLBAR, x+120, y+20, &format!("B  {}", fmt_marker(vcd, b)));
    txt(conn, pix, gc, font, C_CUR, C_TOOLBAR, x+234, y+20, &format!("C  {}", fmt_marker(vcd, c)));
    txt(conn, pix, gc, font, C_BUS, C_TOOLBAR, x+360, y+20, &format!("|B-A|  {}", delta_ab));
    txt(conn, pix, gc, font, C_BUS, C_TOOLBAR, x+500, y+20, &format!("|C-A|  {}", delta_ca));
}

// ── Render ────────────────────────────────────────────────────────────────────

fn render(conn: &RustConnection, pix: u32, gc: u32, font: u32, w: u16, h: u16, app: &App) {
    let (w, h) = (w as i16, h as i16);
    fill(conn, pix, gc, C_BG, 0, 0, w as u16, h as u16);

    // Menu/header
    fill(conn, pix, gc, C_HEADER, 0, 0, w as u16, HEADER_H as u16);
    let mut menu_x = 6;
    for (idx, label) in MENU_ITEMS.iter().enumerate() {
        let item_w = tw(label) + 16;
        let selected = app.selected_menu == Some(idx);
        let fg = if selected { C_HEADER } else { C_LBL };
        let bg = if selected { C_MOD_SEL } else { C_HEADER };
        if selected {
            fill(conn, pix, gc, bg, menu_x - 4, 2, item_w as u16, (HEADER_H - 4) as u16);
        }
        txt(conn, pix, gc, font, fg, bg, menu_x, 4, label);
        menu_x += item_w;
    }
    // Toolbar
    fill(conn, pix, gc, C_TOOLBAR, 0, HEADER_H, w as u16, TOOLBAR_H as u16);
    let toolbar = if let Some(vcd) = &app.vcd {
        let cur = app.cursor.map(|t| format!("cursor {:.0}{}", t, vcd.timescale)).unwrap_or_else(|| "cursor off".into());
        format!(
            " {}   zoom {:.2}x   view {:.0}..{:.0}{}   {}   pinned {}   focus {}   filter {} ",
            if app.filename.is_empty() { "untitled" } else { &app.filename },
            app.zoom,
            app.view_start,
            app.view_end(),
            vcd.timescale,
            cur,
            app.pinned.len(),
            app.focus_name(),
            if app.filter_text.is_empty() { "all" } else { &app.filter_text },
        )
    } else {
        " No VCD loaded   s=sample   Tab=cycle focus   /=filter   drag splitters/signals   q=quit ".into()
    };
    txt(conn, pix, gc, font, C_MOD_SEL, C_TOOLBAR, 6, HEADER_H + 5, &toolbar);

    // Overview strip
    fill(conn, pix, gc, C_OVERVIEW, 0, HEADER_H + TOOLBAR_H, w as u16, OVERVIEW_H as u16);
    render_overview(conn, pix, gc, font, 6, HEADER_H + TOOLBAR_H + 3, w - 12, OVERVIEW_H - 6, app);

    // Status
    let sy = h - STATUS_H;
    fill(conn, pix, gc, C_HEADER, 0, sy, w as u16, STATUS_H as u16);
    let (fl, fc) = match app.focus {
        Focus::ModTree => (" MODULE ", C_MOD_SEL),
        Focus::SigList => (" SIGNALS ", C_PINNED),
        Focus::Wave    => (" WAVE ", C_HI),
    };
    txt(conn, pix, gc, font, fc, C_HEADER, 2, sy+2, fl);
    txt(conn, pix, gc, font, C_DIM, C_HEADER, tw(fl)+6, sy+2, &app.status);

    let marker_y = sy - MARKER_H;
    render_marker_dock(conn, pix, gc, font, 0, marker_y, w, app);

    let body_h  = h - TOP_H - STATUS_H - MARKER_H;
    let mod_h   = (body_h as f32 * app.mod_split) as i16;
    let sig_h   = body_h - mod_h;
    let mod_y   = TOP_H;
    let sig_y   = TOP_H + mod_h;
    let bds = if app.file_browser_active {
        C_BDR
    } else if app.focus == Focus::SigList {
        C_BDR_FOCUS
    } else {
        C_BDR
    };
    let bdr = if app.focus != Focus::Wave { C_BDR_FOCUS } else { C_BDR };

    // Left panel background
    fill(conn, pix, gc, C_PANEL, 0, TOP_H, app.left_w as u16, body_h as u16);

    if app.file_browser_active {
        fill(conn, pix, gc, C_MOD_BG, 0, TOP_H, app.left_w as u16, ROW_H as u16);
        let dir_label = format!(" Open  {}", app.file_dir.display());
        let max_hdr = ((app.left_w - 6) / FW).max(0) as usize;
        txt(conn, pix, gc, font, C_MOD_SEL, C_MOD_BG, 4, TOP_H + 5, &trunc_l(&dir_label, max_hdr));

        let rows_y = TOP_H + ROW_H;
        let marker_y = h - STATUS_H - MARKER_H;
        let avail = ((body_h - ROW_H) / ROW_H).max(0) as usize;
        for (ri, entry) in app.file_entries.iter().enumerate().skip(app.file_scroll).take(avail) {
            let ry = rows_y + (ri - app.file_scroll) as i16 * ROW_H;
            if ry + ROW_H > marker_y {
                break;
            }
            let is_sel = ri == app.file_sel;
            let bg = if is_sel { C_SEL_MOD } else { C_MOD_BG };
            fill(conn, pix, gc, bg, 0, ry, app.left_w as u16, ROW_H as u16);
            let prefix = if entry.is_dir { "▶ " } else { "  " };
            let label = format!("{}{}", prefix, entry.name);
            let max_c = ((app.left_w - 8) / FW).max(0) as usize;
            let col = if entry.is_dir { C_MOD_SEL } else { C_MOD_LBL };
            txt(conn, pix, gc, font, col, bg, 4, ry + 6, &trunc_r(&label, max_c));
            if is_sel {
                fill(conn, pix, gc, C_MOD_SEL, 0, ry, 2, ROW_H as u16);
            }
            seg(conn, pix, gc, C_BDR, 0, ry + ROW_H - 1, app.left_w, ry + ROW_H - 1);
        }

        seg(conn, pix, gc, bdr, app.left_w, TOP_H, app.left_w, marker_y);
    } else {
    // Divider between module tree and signal list
    seg(conn, pix, gc, bds, 0, sig_y, app.left_w, sig_y);
    // Right edge of left panel
    seg(conn, pix, gc, bdr, app.left_w, TOP_H, app.left_w, marker_y);

    // ── Module tree ───────────────────────────────────────────────────────────
    // Header bar
    fill(conn, pix, gc, C_MOD_BG, 0, mod_y, app.left_w as u16, ROW_H as u16);
    let scope_lbl = app.selected_scope_label().unwrap_or_else(|| "Scopes".into());
    let root_count = app.mod_roots.iter().filter(|p| p.as_str() != ALL_SCOPE_PATH).count();
    let hdr_txt = trunc_r(&format!(" Scope Browser  [{} roots]  {}", root_count, scope_lbl), ((app.left_w-6)/FW) as usize);
    txt(conn, pix, gc, font, if app.focus==Focus::ModTree { C_MOD_SEL } else { C_MOD_LBL }, C_MOD_BG, 4, mod_y+5, &hdr_txt);

    let mod_rows_y = mod_y + ROW_H;
    let mod_avail  = ((mod_h - ROW_H) / ROW_H).max(0) as usize;
    let mscroll    = app.mod_scroll.min(app.mod_rows.len().saturating_sub(1));

    for (ri, path) in app.mod_rows.iter().enumerate().skip(mscroll).take(mod_avail) {
        let ry     = mod_rows_y + (ri - mscroll) as i16 * ROW_H;
        if ry + ROW_H > sig_y { break; }
        let is_sel = ri == app.mod_sel;
        let node   = app.mod_nodes.get(path);
        let depth  = node.map(|n| n.depth).unwrap_or(0);
        let name   = node.map(|n| n.name.as_str()).unwrap_or(path.as_str());
        let has_ch = node.map(|n| !n.children.is_empty()).unwrap_or(false);
        let exp    = node.map(|n| n.expanded).unwrap_or(false);

        let bg = if is_sel { C_SEL_MOD } else { C_MOD_BG };
        fill(conn, pix, gc, bg, 0, ry, app.left_w as u16, ROW_H as u16);

        let px     = depth as i16 * INDENT + 4;
        let marker = if has_ch { if exp { "▼ " } else { "▶ " } } else { "• " };
        let col    = if is_sel { C_MOD_SEL } else { C_MOD_LBL };
        let lbl    = format!("{}{}", marker, name);
        let max_c  = ((app.left_w - px - 4) / FW).max(0) as usize;
        txt(conn, pix, gc, font, col, bg, px, ry+6, &trunc_r(&lbl, max_c));

        // Selection bar on left edge
        if is_sel { fill(conn, pix, gc, C_MOD_SEL, 0, ry, 2, ROW_H as u16); }
        seg(conn, pix, gc, C_BDR, 0, ry+ROW_H-1, app.left_w, ry+ROW_H-1);
    }

    // ── Signal list ───────────────────────────────────────────────────────────
    // Header bar
    fill(conn, pix, gc, C_PANEL, 0, sig_y, app.left_w as u16, ROW_H as u16);
    let filter_lbl = if app.filter_text.is_empty() { "all".to_string() } else { app.filter_text.clone() };
    let sig_mode = if app.selected_scope().as_deref() == Some(ALL_SCOPE_PATH) {
        "all-signals"
    } else {
        "direct-signals"
    };
    let sig_hdr = format!(" Objects  {}={}  filter={}", sig_mode, app.sig_rows.len(), filter_lbl);
    txt(conn, pix, gc, font, if app.focus==Focus::SigList {C_PINNED} else {C_DIM},
        C_PANEL, 2, sig_y+5, &sig_hdr);

    let sig_rows_y = sig_y + ROW_H;
    let sig_content_w = (app.left_w - SB).max(1);
    let sig_rows_h = (sig_h - ROW_H - SB).max(1);
    let sig_avail  = (sig_rows_h / SIG_H).max(0) as usize;
    let sscroll    = app.sig_scroll.min(app.sig_rows.len().saturating_sub(1));
    let shscroll   = app.sig_hscroll.min(app.sig_max_hscroll());
    let sig_text_cols = ((sig_content_w - 16) / FW).max(1) as usize;

    for (ri, &si) in app.sig_rows.iter().enumerate().skip(sscroll).take(sig_avail) {
        let ry     = sig_rows_y + (ri - sscroll) as i16 * SIG_H;
        if ry + SIG_H > sig_rows_y + sig_rows_h { break; }
        let is_sel = ri == app.sig_sel && app.focus == Focus::SigList;
        let pinned = app.is_pinned(si);
        let bg     = if is_sel { C_SEL_SIG } else { C_PANEL };
        fill(conn, pix, gc, bg, 0, ry, sig_content_w as u16, SIG_H as u16);

        if let Some(vcd) = &app.vcd {
            let sig   = &vcd.signals[si];
            let wstr  = if sig.width > 1 { format!("[{}:0]", sig.width-1) } else { String::new() };
            let name  = format!("{}{}", sig.name, wstr);
            let col   = if pinned { C_LBL } else { C_MOD_LBL };
            txt(conn, pix, gc, font, col, bg, 16, ry+(SIG_H-13)/2, &slice_text(&name, shscroll, sig_text_cols));

            // Pin marker
            if pinned { txt(conn, pix, gc, font, C_PINNED, bg, 4, ry+(SIG_H-13)/2, "◆"); }
            if is_sel { fill(conn, pix, gc, C_PINNED, 0, ry, 2, SIG_H as u16); }
        }
        seg(conn, pix, gc, C_BDR, 0, ry+SIG_H-1, sig_content_w, ry+SIG_H-1);
    }

    // Signal list scrollbars
    let vbar_x = sig_content_w;
    let hbar_y = sig_y + sig_h - SB;
    fill(conn, pix, gc, C_MOD_BG, vbar_x, sig_rows_y, SB as u16, sig_rows_h as u16);
    fill(conn, pix, gc, C_MOD_BG, 0, hbar_y, sig_content_w as u16, SB as u16);
    fill(conn, pix, gc, C_PANEL, vbar_x, hbar_y, SB as u16, SB as u16);

    let vis_rows = app.sig_vis_rows(sig_h);
    let total_rows = app.sig_rows.len().max(1);
    let max_vscroll = app.sig_rows.len().saturating_sub(vis_rows);
    let vthumb_h = if max_vscroll == 0 {
        sig_rows_h
    } else {
        ((sig_rows_h as f64 * vis_rows as f64 / total_rows as f64).round() as i16).clamp(10, sig_rows_h)
    };
    let vthumb_y = if max_vscroll == 0 {
        sig_rows_y
    } else {
        sig_rows_y + (((sig_rows_h - vthumb_h) as f64 * sscroll as f64 / max_vscroll as f64).round() as i16)
    };
    fill(conn, pix, gc, if app.focus == Focus::SigList { C_MOD_SEL } else { C_BDR }, vbar_x + 1, vthumb_y, (SB - 2) as u16, vthumb_h as u16);

    let vis_cols = app.sig_view_cols();
    let total_cols = app.sig_content_cols().max(1);
    let max_hscroll = app.sig_max_hscroll();
    let hthumb_w = if max_hscroll == 0 {
        sig_content_w
    } else {
        ((sig_content_w as f64 * vis_cols as f64 / total_cols as f64).round() as i16).clamp(16, sig_content_w)
    };
    let hthumb_x = if max_hscroll == 0 {
        0
    } else {
        (((sig_content_w - hthumb_w) as f64 * shscroll as f64 / max_hscroll as f64).round() as i16).max(0)
    };
    fill(conn, pix, gc, if app.focus == Focus::SigList { C_MOD_SEL } else { C_BDR }, hthumb_x, hbar_y + 1, hthumb_w as u16, (SB - 2) as u16);
    seg(conn, pix, gc, C_BDR, vbar_x, sig_rows_y, vbar_x, hbar_y + SB);
    seg(conn, pix, gc, C_BDR, 0, hbar_y, vbar_x + SB, hbar_y);
    }

    // ── Waveform area ─────────────────────────────────────────────────────────
    let wx     = app.left_w;
    let value_x = wx + app.name_w;
    let wave_x = value_x + VALUE_W;
    let wave_w = w - wave_x;

    let Some(vcd) = &app.vcd else {
        txt(conn, pix, gc, font, C_DIM, C_BG, wx+50, TOP_H+body_h/2, "Select a scope, add signals, then inspect waveforms");
        render_file_menu(conn, pix, gc, font, app);
        return;
    };

    // Name column divider
    let wbdr = if app.focus == Focus::Wave { C_BDR_FOCUS } else { C_BDR };
    seg(conn, pix, gc, wbdr, wave_x, TOP_H, wave_x, marker_y);
    seg(conn, pix, gc, C_BDR, value_x, TOP_H, value_x, marker_y);
    fill(conn, pix, gc, bdr, app.left_w-1, TOP_H, 3, body_h as u16);
    fill(conn, pix, gc, wbdr, wave_x-1, TOP_H, 3, body_h as u16);
    fill(conn, pix, gc, bds, 0, sig_y-1, app.left_w as u16, 3);

    // Ruler
    fill(conn, pix, gc, C_PANEL, wx, TOP_H, (app.name_w + VALUE_W) as u16, RULER_H as u16);
    txt(conn, pix, gc, font, C_MOD_LBL, C_PANEL, wx+4, TOP_H+6, "Name");
    txt(conn, pix, gc, font, C_MOD_LBL, C_PANEL, value_x+4, TOP_H+6, "Value");
    render_ruler(conn, pix, gc, font, wave_x, TOP_H, wave_w,
        app.view_start, app.view_end(), app.max_time(), &vcd.timescale, app.cursor);

    // End-of-sim marker
    let t0e = app.view_start; let t1e = app.view_end();
    let rng = (t1e - t0e).max(1.0);
    let mt  = app.max_time();
    if mt >= t0e && mt <= t1e {
        let mx = wave_x + ((mt - t0e)/rng * wave_w as f64).clamp(0.0, wave_w as f64-1.0) as i16;
        seg(conn, pix, gc, C_HI, mx, TOP_H+RULER_H, mx, TOP_H+body_h);
    }

    let base_y   = TOP_H + RULER_H;
    let avail_h  = body_h - RULER_H;
    let max_rows = (avail_h / WAVE_H).max(1) as usize;
    let wscroll  = app.wave_scroll.min(app.wave_rows.len().saturating_sub(1));

    for (ri, row) in app.wave_rows.iter().enumerate().skip(wscroll).take(max_rows) {
        let ry     = base_y + (ri - wscroll) as i16 * WAVE_H;
        if ry + WAVE_H > base_y + avail_h { break; }
        let is_sel = ri == app.wave_sel;
        let wbg    = if ri%2==0 { C_BG } else { C_WAVE_ALT };

        match row {
            WaveRow::Signal { sig_idx } => {
                let si  = *sig_idx;
                let sig = &vcd.signals[si];

                // Name column
                let nbg = if is_sel && app.focus==Focus::Wave { C_SEL_WAVE } else { C_PANEL };
                fill(conn, pix, gc, nbg, wx, ry, app.name_w as u16, WAVE_H as u16);
                fill(conn, pix, gc, C_VALUE_BG, value_x, ry, VALUE_W as u16, WAVE_H as u16);

                // Full path (top, dim, small)
                let max_c   = ((app.name_w - 8) / FW).max(0) as usize;
                let full    = trunc_l(&sig.full_name, max_c);
                txt(conn, pix, gc, font, C_PATH, nbg, wx+4, ry+2, &full);

                // Short name (bottom, bright)
                let wstr   = if sig.width > 1 { format!("[{}:0]", sig.width-1) } else { String::new() };
                let short  = format!("{}{}", sig.name, wstr);
                let smax   = ((app.name_w - 8) / FW).max(0) as usize;
                txt(conn, pix, gc, font, C_LBL, nbg, wx+4, ry+WAVE_H/2, &trunc_r(&short, smax));

                let sample_t = app.cursor.unwrap_or(app.view_start);
                let val = vcd.get_value_at(&sig.id, sample_t as u64);
                let dv  = fmt_val(&val, sig.width);
                let vmax = ((VALUE_W - 8) / FW).max(0) as usize;
                txt(conn, pix, gc, font, if app.cursor.is_some() { C_CUR } else { C_BUS }, C_VALUE_BG, value_x+4, ry+WAVE_H/2, &trunc_l(&dv, vmax));

                // Expand marker
                if sig.width > 1 {
                    let e = app.wave_expanded.contains(&si);
                    txt(conn, pix, gc, font, C_BIT_LBL, nbg, wx+app.name_w-FW*2-2, ry+2, if e {"▼"} else {"▶"});
                }

                // Waveform
                fill(conn, pix, gc, wbg, wave_x, ry, wave_w as u16, WAVE_H as u16);
                if is_sel { fill(conn, pix, gc, if app.focus==Focus::Wave{C_HI}else{C_DIM}, wave_x, ry, 2, WAVE_H as u16); }
                let changes = vcd.changes.get(&sig.id).map(|v|v.as_slice()).unwrap_or(&[]);
                render_wave(conn, pix, gc, font, wave_x, ry, wave_w, changes, sig.width,
                    app.view_start, app.view_end(), app.cursor, wbg);
            }
            WaveRow::BitSlice { sig_idx, bit } => {
                let si  = *sig_idx;
                let sig = &vcd.signals[si];
                let nbg = if is_sel && app.focus==Focus::Wave { C_SEL_WAVE } else { C_PANEL };
                fill(conn, pix, gc, nbg, wx, ry, app.name_w as u16, WAVE_H as u16);
                fill(conn, pix, gc, C_VALUE_BG, value_x, ry, VALUE_W as u16, WAVE_H as u16);

                let path = format!("{}[{}]", sig.full_name, bit);
                let max_c = ((app.name_w-8)/FW).max(0) as usize;
                txt(conn, pix, gc, font, C_BIT_LBL, nbg, wx+4, ry+(WAVE_H-13)/2, &trunc_l(&path, max_c));

                let sample_t = app.cursor.unwrap_or(app.view_start);
                let raw = vcd.get_value_at(&sig.id, sample_t as u64);
                let bv  = extract_bit(&raw, *bit);
                txt(conn, pix, gc, font, if app.cursor.is_some() { C_CUR } else { C_BIT_LBL }, C_VALUE_BG, value_x+4, ry+(WAVE_H-13)/2, &bv);

                fill(conn, pix, gc, wbg, wave_x, ry, wave_w as u16, WAVE_H as u16);
                if is_sel { fill(conn, pix, gc, if app.focus==Focus::Wave{C_BIT_LBL}else{C_DIM}, wave_x, ry, 2, WAVE_H as u16); }
                let raw_ch = vcd.changes.get(&sig.id).map(|v|v.as_slice()).unwrap_or(&[]);
                let bit_ch = synth_bit_changes(raw_ch, *bit);
                render_wave(conn, pix, gc, font, wave_x, ry, wave_w, &bit_ch, 1,
                    app.view_start, app.view_end(), app.cursor, wbg);
            }
        }
        seg(conn, pix, gc, C_BDR, value_x, ry, value_x + VALUE_W, ry);
        seg(conn, pix, gc, C_BDR, wx, ry+WAVE_H-1, w, ry+WAVE_H-1);
    }

    render_file_menu(conn, pix, gc, font, app);
}

fn render_file_menu(conn: &RustConnection, pix: u32, gc: u32, font: u32, app: &App) {
    if let Some((mx, my, mw, mh)) = app.file_menu_geometry() {
        fill(conn, pix, gc, C_PANEL, mx, my, mw as u16, mh as u16);
        fill(conn, pix, gc, C_BDR, mx, my, mw as u16, 1);
        fill(conn, pix, gc, C_BDR, mx, my + mh - 1, mw as u16, 1);
        fill(conn, pix, gc, C_BDR, mx, my, 1, mh as u16);
        fill(conn, pix, gc, C_BDR, mx + mw - 1, my, 1, mh as u16);
        for (idx, item) in FILE_MENU_ITEMS.iter().enumerate() {
            let iy = my + idx as i16 * ROW_H;
            let disabled = *item == "Reload" && app.file_path.is_none();
            let fg = if disabled { C_DIM } else { C_LBL };
            txt(conn, pix, gc, font, fg, C_PANEL, mx + 8, iy + 6, item);
            if idx + 1 < FILE_MENU_ITEMS.len() {
                seg(conn, pix, gc, C_BDR, mx, iy + ROW_H - 1, mx + mw, iy + ROW_H - 1);
            }
        }
    }
}

// ── Ruler ─────────────────────────────────────────────────────────────────────
fn render_ruler(conn: &RustConnection, pix: u32, gc: u32, font: u32,
    x: i16, y: i16, w: i16, t0: f64, t1: f64, max_time: f64, timescale: &str, cursor: Option<f64>) {
    fill(conn, pix, gc, C_HEADER, x, y, w as u16, RULER_H as u16);
    let range = (t1-t0).max(1.0);
    let steps = ((w/70) as usize).max(4).min(20);
    let step  = (range/steps as f64).ceil().max(1.0);
    let to_x  = |t: f64| -> i16 { x + ((t-t0)/range*w as f64).clamp(0.0,w as f64-1.0) as i16 };
    txt(conn, pix, gc, font, C_DIM, C_HEADER, x+2, y+2, timescale);
    let mut last_x = i16::MIN;
    let mut t = (t0/step).ceil()*step;
    while t <= t1 {
        let tx = to_x(t);
        if tx >= x && tx < x+w {
            seg(conn, pix, gc, C_DIM, tx, y+RULER_H-8, tx, y+RULER_H-1);
            txt(conn, pix, gc, font, C_RUL, C_HEADER, tx+1, y+2, &format!("{}", t as u64));
            last_x = tx;
        }
        t += step;
    }
    if max_time >= t0 && max_time <= t1 {
        let mx  = to_x(max_time);
        let lbl = format!("{}|", max_time as u64);
        fill(conn, pix, gc, C_HI, mx, y, 2, RULER_H as u16);
        if mx > last_x + tw(&lbl) + 2 { txt(conn, pix, gc, font, C_HI, C_HEADER, (mx-tw(&lbl)-1).max(x), y+2, &lbl); }
    }
    if let Some(ct) = cursor {
        if ct >= t0 && ct <= t1 { fill(conn, pix, gc, C_CUR, to_x(ct)-1, y, 3, RULER_H as u16); }
    }
    seg(conn, pix, gc, C_BDR, x, y+RULER_H-1, x+w, y+RULER_H-1);
}

// ── Wave renderer ─────────────────────────────────────────────────────────────
fn render_wave(conn: &RustConnection, pix: u32, gc: u32, font: u32,
    x: i16, y: i16, w: i16, changes: &[ValueChange], width: usize,
    t0: f64, t1: f64, cursor: Option<f64>, _bg: u32) {
    let range = (t1-t0).max(1.0);
    let to_x  = |t: f64| -> i16 { x + ((t-t0)/range*w as f64).clamp(0.0,w as f64-1.0) as i16 };
    let pad: i16 = 5;
    let mid = y + WAVE_H/2;
    let hi  = y + pad;
    let lo  = y + WAVE_H - pad - 1;
    let grid_steps = ((w / 90) as usize).max(3).min(16);
    let grid_step = range / grid_steps as f64;

    for gi in 0..=grid_steps {
        let gx = to_x(t0 + gi as f64 * grid_step);
        seg(conn, pix, gc, C_GRID, gx, y, gx, y + WAVE_H - 1);
    }

    if changes.is_empty() {
        dashed(conn, pix, gc, C_X, x, mid, x+w-1);
    } else if width == 1 {
        let vy  = |v:char| -> i16  { match v {'1'=>hi,'0'=>lo,_=>mid} };
        let vc  = |v:char| -> u32  { match v {'1'=>C_HI,'0'=>C_LO,'z'=>C_Z,_=>C_X} };
        let xz  = |v:char| -> bool { v=='x'||v=='z' };
        let mut cur:char = 'x';
        for c in changes { if (c.time as f64)<=t0 { cur=c.value.chars().next().unwrap_or('x'); } else {break;} }
        let mut ci = changes.partition_point(|c|(c.time as f64)<=t0);
        let mut hs: Vec<Segment> = Vec::new();
        let mut hc = vc(cur);
        let mut xs: Vec<(u32,i16,i16,i16)> = Vec::new();
        let fh = |conn:&RustConnection, col:u32, ss:&mut Vec<Segment>| { if !ss.is_empty() { segs(conn,pix,gc,col,ss); ss.clear(); } };
        for px in 0..(w as usize) {
            let tx1 = t0 + (px+1) as f64/w as f64*range;
            let sx  = x + px as i16;
            let mut n=0usize; let mut nc=cur;
            while ci<changes.len() && (changes[ci].time as f64)<=tx1 { nc=changes[ci].value.chars().next().unwrap_or('x'); n+=1; ci+=1; }
            if n==0 {
                if xz(cur) { xs.push((vc(cur),sx,vy(cur),sx)); }
                else { let c=vc(cur); if c!=hc{fh(conn,hc,&mut hs);hc=c;} hs.push(Segment{x1:sx,y1:vy(cur),x2:sx,y2:vy(cur)}); }
            } else if n==1 {
                fh(conn,hc,&mut hs);
                segs(conn,pix,gc,vc(nc),&[Segment{x1:sx,y1:vy(cur),x2:sx,y2:vy(nc)}]);
                hc=vc(nc);
            } else {
                fh(conn,hc,&mut hs);
                seg(conn,pix,gc,C_DIM,sx,hi,sx,lo);
                hc=vc(nc);
            }
            cur=nc;
        }
        fh(conn,hc,&mut hs);
        for (col,x1,yy,x2) in xs { dashed(conn,pix,gc,col,x1,yy,x2); }
    } else {
        let ii = changes.partition_point(|c|(c.time as f64)<=t0);
        let mut pv = if ii>0 { changes[ii-1].value.clone() } else { "x".repeat(width) };
        let mut pt = t0;
        for c in &changes[ii..] {
            let vt = c.time as f64; if vt>t1{break;}
            draw_bus(conn,pix,gc,font,x,y,w,pt,vt,&pv,t0,t1,hi,mid,lo);
            pv=c.value.clone(); pt=vt;
        }
        draw_bus(conn,pix,gc,font,x,y,w,pt,t1,&pv,t0,t1,hi,mid,lo);
    }
    if let Some(t)=cursor { if t>=t0&&t<=t1 { let cx=to_x(t); seg(conn,pix,gc,C_CUR,cx,y,cx,y+WAVE_H-1); } }
}

fn draw_bus(conn: &RustConnection, pix: u32, gc: u32, font: u32,
    x: i16, y: i16, w: i16, ts: f64, te: f64, val: &str,
    t0: f64, t1: f64, hi: i16, mid: i16, lo: i16) {
    let range = (t1-t0).max(1.0);
    let to_x  = |t:f64| -> i16 { x+((t-t0)/range*w as f64).clamp(0.0,w as f64-1.0) as i16 };
    let x0=to_x(ts); let x1=to_x(te); let sw=x1-x0; if sw<2{return;}
    let n=4i16.min(sw/2);
    let is_x=val.chars().any(|c|c=='x'||c=='X');
    let col=if is_x{C_X}else{C_BUS};
    segs(conn,pix,gc,col,&[
        Segment{x1:x0+n,y1:hi, x2:x1-n,y2:hi}, Segment{x1:x0+n,y1:lo, x2:x1-n,y2:lo},
        Segment{x1:x0,  y1:mid,x2:x0+n,y2:hi}, Segment{x1:x0,  y1:mid,x2:x0+n,y2:lo},
        Segment{x1:x1,  y1:mid,x2:x1-n,y2:hi}, Segment{x1:x1,  y1:mid,x2:x1-n,y2:lo},
    ]);
    if sw>16 {
        let lbl=if is_x{"X".into()}else{u64::from_str_radix(val,2).map(|n|format!("{:#X}",n)).unwrap_or_else(|_|val[..val.len().min(6)].to_string())};
        let lw=tw(&lbl); let lx=x0+n+(sw-n*2-lw)/2;
        if lx>x0&&lx+lw<x1 { txt(conn,pix,gc,font,col,C_BG,lx,mid-6,&lbl); }
    }
}

fn extract_bit(val: &str, bit: usize) -> String {
    let chars: Vec<char> = val.chars().collect();
    let len = chars.len();
    let ch  = if bit<len { chars[len-1-bit] } else { match chars.first().copied().unwrap_or('x') {'0'|'1'=>'0','z'|'Z'=>'z',_=>'x'} };
    match ch {'0'=>"0",'1'=>"1",'z'|'Z'=>"z",_=>"x"}.into()
}

fn synth_bit_changes(changes: &[ValueChange], bit: usize) -> Vec<ValueChange> {
    changes.iter().map(|vc| ValueChange { time: vc.time, value: extract_bit(&vc.value, bit) }).collect()
}

// ── Entry point ───────────────────────────────────────────────────────────────
fn get_keysym(keysyms: &[u32], kpc: u8, keycode: u8, min_kc: u8, state: KeyButMask) -> u32 {
    let base = (keycode.saturating_sub(min_kc)) as usize * kpc as usize;
    if state.contains(KeyButMask::SHIFT) && kpc > 1 {
        let shifted = keysyms.get(base + 1).copied().unwrap_or(0);
        if shifted != 0 {
            return shifted;
        }
    }
    keysyms.get(base).copied().unwrap_or(0)
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut display: Option<String> = None;
    let mut file_arg: Option<String> = None;
    let mut i=1;
    while i<args.len() {
        match args[i].as_str() {
            "-d"|"--display" => { i+=1; display=args.get(i).cloned(); }
            "-h"|"--help" => {
                eprintln!("claudeV [-d DISPLAY] [file.vcd]");
                eprintln!("Tab=cycle focus (Browser→Objects→Wave)");
                eprintln!("/=edit filter  X=clear filter  drag splitters/signals with mouse");
                eprintln!("m=set marker  M=toggle active marker  D=clear markers");
                eprintln!("MODULE:  j/k=nav  Enter=expand/collapse  A=add all in selected scope");
                eprintln!("SIGNALS: j/k=nav  a/Enter=add/remove  A=add all in selected scope  drag->wave");
                eprintln!("          (select '(all scopes)' in module tree for global signal selection)");
                eprintln!("WAVE:    j/k=nav  d/Del=remove  J/K=reorder  e=expand bus");
                eprintln!("VIEW:    +/-/f=zoom  h/l=pan  c=cursor  n/N=edge  q=quit");
                std::process::exit(0);
            }
            other if !other.starts_with('-') => file_arg=Some(other.to_string()),
            other => eprintln!("Unknown: {}", other),
        }
        i+=1;
    }

    let (conn, sn) = x11rb::connect(display.as_deref())?;
    let screen     = &conn.setup().roots[sn].clone();
    let (mut ww, mut wh) = (1280u16, 800u16);

    let wmp = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wmd = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    let win: u32 = conn.generate_id()?;
    conn.create_window(COPY_DEPTH_FROM_PARENT, win, screen.root, 0,0, ww, wh, 0,
        WindowClass::INPUT_OUTPUT, screen.root_visual,
        &CreateWindowAux::new().background_pixel(C_BG)
            .event_mask(EventMask::EXPOSURE|EventMask::KEY_PRESS|EventMask::BUTTON_PRESS|EventMask::BUTTON_RELEASE|EventMask::POINTER_MOTION|EventMask::STRUCTURE_NOTIFY))?;
    conn.change_property32(PropMode::REPLACE, win, wmp, AtomEnum::ATOM, &[wmd])?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"claudeV")?;

    let font: u32 = conn.generate_id()?;
    conn.open_font(font, b"fixed")?;
    let gc: u32 = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new().foreground(C_HI).background(C_BG).font(font))?;
    let mut pix: u32 = conn.generate_id()?;
    conn.create_pixmap(screen.root_depth, pix, win, ww, wh)?;

    let mkc = conn.setup().min_keycode;
    let xkc = conn.setup().max_keycode;
    let kbd = conn.get_keyboard_mapping(mkc, xkc-mkc+1)?.reply()?;

    conn.map_window(win)?;
    conn.flush()?;

    let mut app = App::new();
    if let Some(p) = &file_arg { app.load_file(p); }

    let mut dirty = true;
    loop {
        let ev = conn.wait_for_event()?;
        match ev {
            Event::Expose(e) if e.count==0 => dirty=true,
            Event::ConfigureNotify(e) => {
                if e.width!=ww || e.height!=wh {
                    ww=e.width; wh=e.height;
                    conn.free_pixmap(pix)?;
                    pix=conn.generate_id()?;
                    conn.create_pixmap(screen.root_depth, pix, win, ww, wh)?;
                    dirty=true;
                }
            }
            Event::KeyPress(e) => {
                let ks = get_keysym(&kbd.keysyms, kbd.keysyms_per_keycode, e.detail, mkc, e.state);
                if app.handle_keysym(ks, wh as i16) { break; }
                dirty=true;
            }
            Event::ButtonPress(e) => {
                if app.handle_button(e.detail, e.state, e.event_x, e.event_y, ww as i16, wh as i16) == MenuAction::Quit {
                    break;
                }
                dirty=true;
            }
            Event::MotionNotify(e) => { app.handle_motion(e.event_x, e.event_y, ww as i16, wh as i16); dirty=true; }
            Event::ButtonRelease(e) => { app.handle_button_release(e.event_x, e.event_y, wh as i16); dirty=true; }
            Event::ClientMessage(e) => { if e.data.as_data32().first().copied()==Some(wmd) { break; } }
            Event::Error(e) => eprintln!("X11: {:?}", e),
            _ => {}
        }
        if dirty {
            render(&conn, pix, gc, font, ww, wh, &app);
            conn.copy_area(pix, win, gc, 0,0,0,0, ww, wh)?;
            conn.flush()?;
            dirty=false;
        }
    }
    conn.close_font(font)?; conn.free_pixmap(pix)?; conn.free_gc(gc)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_keysym_uses_shift_level_when_shift_is_pressed() {
        let keysyms = [0x61, 0x41]; // 'a', 'A'
        assert_eq!(get_keysym(&keysyms, 2, 8, 8, KeyButMask::from(0u16)), 0x61);
        assert_eq!(get_keysym(&keysyms, 2, 8, 8, KeyButMask::SHIFT), 0x41);
    }

    #[test]
    fn top_level_scopes_show_direct_signals() {
        let data = parse_vcd(SAMPLE_VCD).expect("sample VCD should parse");
        let mut app = App::new();
        app.build_mod_tree(&data);
        app.vcd = Some(data);
        app.rebuild_mod_rows();
        app.rebuild_sig_rows();

        assert_eq!(
            app.mod_roots,
            vec!["tb".to_string(), "dut".to_string(), ALL_SCOPE_PATH.to_string()]
        );
        assert_eq!(app.selected_scope().as_deref(), Some("tb"));
        assert_eq!(app.sig_rows.len(), 5);
    }

    #[test]
    fn all_scope_lists_every_signal() {
        let data = parse_vcd(SAMPLE_VCD).expect("sample VCD should parse");
        let mut app = App::new();
        app.build_mod_tree(&data);
        app.vcd = Some(data);
        app.rebuild_mod_rows();

        let all_idx = app.mod_rows.iter().position(|path| path == ALL_SCOPE_PATH)
            .expect("all scope should be present");
        app.mod_sel = all_idx;
        app.rebuild_sig_rows();

        let total_signals = app.vcd.as_ref().map(|v| v.signals.len()).unwrap_or(0);
        assert_eq!(app.sig_rows.len(), total_signals);
    }
}
