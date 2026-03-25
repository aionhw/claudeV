mod vcd_parser;
use vcd_parser::{VcdData, ValueChange, SAMPLE_VCD, parse_vcd};

use std::collections::{HashSet, HashMap};
use std::error::Error;
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
const C_BG:         u32 = 0x060A06;
const C_WAVE_ALT:   u32 = 0x080C08;
const C_PANEL:      u32 = 0x050905;
const C_HEADER:     u32 = 0x030603;
const C_HI:         u32 = 0x00FF70;
const C_LO:         u32 = 0x008840;
const C_X:          u32 = 0xFF6B35;
const C_Z:          u32 = 0x4DAAFF;
const C_BUS:        u32 = 0x00D860;
const C_CUR:        u32 = 0xFFDC00;
const C_LBL:        u32 = 0x8AFF9A;
const C_DIM:        u32 = 0x2A5A2A;
const C_BDR:        u32 = 0x0F2A0F;
const C_BDR_FOCUS:  u32 = 0x00AA50;
const C_SEP:        u32 = 0xFFFFFF; // white section separator
const C_GRP_BG:     u32 = 0x080E10; // group header background
const C_GRP_LBL:    u32 = 0x40C0E0; // group header label (cyan)
const C_GRP_SEL:    u32 = 0x0C1C20; // group header selected
const C_SPACER:     u32 = 0x0C120C; // spacer row background
const C_COMP_BG:    u32 = 0x0A0A14; // computed signal name bg
const C_COMP_LBL:   u32 = 0xA080FF; // computed signal label (purple)
const C_COMP_SEL:   u32 = 0x141420; // computed signal selected bg
const C_PENDING:    u32 = 0xFFAA00; // pending-op highlight
const C_SEL_MOD:    u32 = 0x0A1A0A;
const C_SEL_SIG:    u32 = 0x0C200C;
const C_SEL_WAVE:   u32 = 0x142814;
const C_RUL:        u32 = 0x40E060;
const C_MOD_BG:     u32 = 0x030703;
const C_MOD_LBL:    u32 = 0x00A060;
const C_MOD_SEL:    u32 = 0x00CC66;
const C_BIT_LBL:    u32 = 0x406040;
const C_PINNED:     u32 = 0x00CC55;
const C_PATH:       u32 = 0x507850;

// ── Layout ────────────────────────────────────────────────────────────────────
const LEFT_W:    i16 = 230; // total left panel width
const NAME_W:    i16 = 210; // signal name column in waveform area
const ROW_H:     i16 = 26;  // module tree row height
const SIG_H:     i16 = 30;  // signal list row height
const WAVE_H:    i16 = 36;  // waveform row height
const RULER_H:   i16 = 24;
const HEADER_H:  i16 = 20;
const STATUS_H:  i16 = 16;
const FW:        i16 = 6;
const FA:        i16 = 10;
const INDENT:    i16 = 16;
const MOD_SPLIT: f32 = 0.35; // fraction of body height for module tree

// ── Module tree node ──────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
struct ModNode {
    path:     String,         // full dot-path e.g. "testbench.uut"
    name:     String,         // just "uut"
    depth:    usize,
    children: Vec<String>,    // child scope paths
    expanded: bool,
}

// ── Pin item (persistent waveform list model) ────────────────────────────────
#[derive(Clone, Debug)]
enum PinItem {
    Sig(usize),
    Computed(usize),   // index into app.computed
    Spacer,
    Group { name: String, collapsed: bool, sigs: Vec<usize> },
}
impl PinItem {
    fn is_sig(&self, si: usize) -> bool { matches!(self, PinItem::Sig(s) if *s == si) }
    fn contains(&self, si: usize) -> bool {
        match self {
            PinItem::Sig(s)          => *s == si,
            PinItem::Group{sigs,..}  => sigs.contains(&si),
            PinItem::Spacer | PinItem::Computed(_) => false,
        }
    }
}

// ── Wave row (derived, for rendering) ─────────────────────────────────────────
#[derive(Clone, Debug)]
enum WaveRow {
    GroupHeader { name: String, collapsed: bool, pin_idx: usize },
    Signal      { sig_idx: usize, pin_idx: usize, grp_idx: Option<usize> },
    BitSlice    { sig_idx: usize, bit: usize },
    Spacer      { pin_idx: usize },
    Computed    { comp_idx: usize, pin_idx: usize },
}

// ── Computed signals ──────────────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq)]
enum BinOp  { And, Or, Xor, Nand, Nor, Xnor, ShiftL(u8), ShiftR(u8) }
#[derive(Clone, Debug, PartialEq)]
enum UnaryOp { Not, ShiftL(u8), ShiftR(u8) }

#[derive(Clone, Debug)]
enum CompOp {
    Unary  { op: UnaryOp,  src: usize },           // src = wave_row index at creation
    Binary { op: BinOp,    src_a: usize, src_b: usize },
}

#[derive(Clone, Debug)]
struct Computed {
    name:    String,
    op:      CompOp,
    width:   usize,
    changes: Vec<vcd_parser::ValueChange>,
}

impl BinOp {
    fn symbol(&self) -> &str {
        match self { BinOp::And=>"&", BinOp::Or=>"|", BinOp::Xor=>"^",
            BinOp::Nand=>"~&", BinOp::Nor=>"~|", BinOp::Xnor=>"~^",
            BinOp::ShiftL(_)=>"<<", BinOp::ShiftR(_)=>">>" }
    }
}
impl UnaryOp { fn symbol(&self) -> &str { match self { UnaryOp::Not=>"~", UnaryOp::ShiftL(_)=>"<<", UnaryOp::ShiftR(_)=>">>" } } }

// ── Pending operation state ────────────────────────────────────────────────────
#[derive(Clone, Debug)]
enum Pending {
    SelectOp  { src_row: usize, src_name: String }, // chose first signal, waiting for op
    SelectB   { op: BinOp, src_a: usize, src_name: String }, // chose op, picking second signal
    ShiftAmt  { unary: bool, src_row: usize, src_name: String, op_sym: String, amt: String }, // typing shift amount
}

// ── Focus ─────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq)]
enum Focus { ModTree, SigList, Wave }

// ── App ───────────────────────────────────────────────────────────────────────
struct App {
    vcd:           Option<VcdData>,
    filename:      String,

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

    // Waveform
    items:         Vec<PinItem>, // ordered waveform list
    wave_expanded: HashSet<usize>,
    wave_rows:     Vec<WaveRow>,
    wave_sel:      usize,
    wave_scroll:   usize,

    // Computed signals
    computed:  Vec<Computed>,
    pending:   Option<Pending>,

    // View
    zoom:       f64,
    view_start: f64,
    cursor:     Option<f64>,
    focus:      Focus,
    status:     String,
}

impl App {
    fn new() -> Self {
        App {
            vcd: None, filename: String::new(),
            mod_nodes: HashMap::new(), mod_roots: Vec::new(),
            mod_rows: Vec::new(), mod_sel: 0, mod_scroll: 0,
            sig_rows: Vec::new(), sig_sel: 0, sig_scroll: 0,
            items: Vec::new(), wave_expanded: HashSet::new(),
            computed: Vec::new(), pending: None,
            wave_rows: Vec::new(), wave_sel: 0, wave_scroll: 0,
            zoom: 1.0, view_start: 0.0, cursor: None,
            focus: Focus::ModTree,
            status: "Tab=switch panel  a=add signal  A=add module  d=remove  q=quit  ?=help".into(),
        }
    }

    fn max_time(&self) -> f64 { self.vcd.as_ref().map(|v| v.max_time as f64).unwrap_or(100.0) }
    fn view_end(&self)   -> f64 { self.view_start + self.max_time() / self.zoom }

    fn clamp_view(&mut self) {
        let r = self.max_time() / self.zoom;
        self.view_start = self.view_start.clamp(0.0, (self.max_time() - r).max(0.0));
    }

    fn wave_vis_rows(&self, win_h: i16) -> usize {
        ((win_h - HEADER_H - STATUS_H - RULER_H) / WAVE_H).max(1) as usize
    }

    fn mod_vis_rows(&self, mod_panel_h: i16) -> usize {
        ((mod_panel_h - ROW_H) / ROW_H).max(1) as usize   // subtract header
    }

    fn sig_vis_rows(&self, sig_panel_h: i16) -> usize {
        ((sig_panel_h - ROW_H) / SIG_H).max(1) as usize
    }

    // ── Load ──────────────────────────────────────────────────────────────────
    fn load_text(&mut self, text: &str, name: &str) {
        match parse_vcd(text) {
            Ok(data) => {
                let (n, mt, ts) = (data.signals.len(), data.max_time, data.timescale.clone());
                self.filename = name.to_string();
                self.zoom = 1.0; self.view_start = 0.0; self.cursor = None;
                self.mod_sel = 0; self.mod_scroll = 0;
                self.sig_sel = 0; self.sig_scroll = 0;
                self.wave_sel = 0; self.wave_scroll = 0;
                self.wave_expanded.clear();
                self.items  = (0..data.signals.len()).map(PinItem::Sig).collect();
                self.computed.clear();
                self.build_mod_tree(&data);
                self.vcd = Some(data);
                self.rebuild_mod_rows();
                self.rebuild_sig_rows();
                self.rebuild_wave();
                self.status = format!("'{}' — {} signals, end={}{}", name, n, mt, ts);
            }
            Err(e) => self.status = format!("Parse error: {}", e),
        }
    }

    fn load_file(&mut self, path: &str) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let fname = std::path::Path::new(path)
                    .file_name().unwrap_or_default().to_string_lossy().to_string();
                self.load_text(&text, &fname);
            }
            Err(e) => self.status = format!("File error: {}", e),
        }
    }

    // ── Build module tree from VCD signals ────────────────────────────────────
    fn build_mod_tree(&mut self, vcd: &VcdData) {
        self.mod_nodes.clear();
        self.mod_roots.clear();

        // Collect all unique scope paths from signal full_names
        let mut seen: HashSet<String> = HashSet::new();
        for sig in &vcd.signals {
            let parts: Vec<&str> = sig.full_name.split('.').collect();
            for depth in 1..parts.len() {
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

    fn rebuild_sig_rows(&mut self) {
        self.sig_rows.clear();
        let Some(vcd) = &self.vcd else { return };
        let scope = match self.selected_scope() {
            Some(s) => s,
            None    => return,
        };
        for (si, sig) in vcd.signals.iter().enumerate() {
            let parts: Vec<&str> = sig.full_name.split('.').collect();
            let sig_scope = parts[..parts.len()-1].join(".");
            if sig_scope == scope {
                self.sig_rows.push(si);
            }
        }
        self.sig_sel    = 0;
        self.sig_scroll = 0;
    }

    // ── Pinning ───────────────────────────────────────────────────────────────
    fn is_pinned(&self, si: usize) -> bool { self.items.iter().any(|it| it.contains(si)) }

    fn pin_count(&self) -> usize {
        self.items.iter().map(|it| match it {
            PinItem::Sig(_) | PinItem::Computed(_) => 1,
            PinItem::Group{sigs,..}  => sigs.len(),
            PinItem::Spacer          => 0,
        }).sum()
    }

    fn pin(&mut self, si: usize) {
        if !self.is_pinned(si) { self.items.push(PinItem::Sig(si)); self.rebuild_wave(); }
    }

    fn unpin(&mut self, si: usize) {
        self.items.retain(|it| !it.is_sig(si));
        for it in &mut self.items {
            if let PinItem::Group { sigs, .. } = it { sigs.retain(|&s| s != si); }
        }
        self.wave_expanded.remove(&si);
        self.rebuild_wave();
    }

    fn toggle_pin(&mut self, si: usize) {
        if self.is_pinned(si) { self.unpin(si); } else { self.pin(si); }
    }

    fn pin_scope(&mut self, scope: &str) {
        let Some(vcd) = &self.vcd else { return };
        let to_add: Vec<usize> = vcd.signals.iter().enumerate()
            .filter(|(_, sig)| {
                let parts: Vec<&str> = sig.full_name.split('.').collect();
                let sp = parts[..parts.len()-1].join(".");
                sp == scope || sp.starts_with(&format!("{}.", scope))
            })
            .map(|(i, _)| i).collect();
        for si in to_add { if !self.is_pinned(si) { self.items.push(PinItem::Sig(si)); } }
        self.rebuild_wave();
    }

    // ── Wave rows ─────────────────────────────────────────────────────────────
    fn rebuild_wave(&mut self) {
        let Some(vcd) = &self.vcd else { self.wave_rows.clear(); return };
        self.wave_rows.clear();
        for (pi, item) in self.items.iter().enumerate() {
            match item {
                PinItem::Computed(ci) => {
                    self.wave_rows.push(WaveRow::Computed { comp_idx: *ci, pin_idx: pi });
                }
                PinItem::Spacer => {
                    self.wave_rows.push(WaveRow::Spacer { pin_idx: pi });
                }
                PinItem::Sig(si) => {
                    let si = *si;
                    self.wave_rows.push(WaveRow::Signal { sig_idx: si, pin_idx: pi, grp_idx: None });
                    if self.wave_expanded.contains(&si) {
                        for bit in (0..vcd.signals[si].width).rev() {
                            self.wave_rows.push(WaveRow::BitSlice { sig_idx: si, bit });
                        }
                    }
                }
                PinItem::Group { name, collapsed, sigs } => {
                    self.wave_rows.push(WaveRow::GroupHeader {
                        name: name.clone(), collapsed: *collapsed, pin_idx: pi,
                    });
                    if !collapsed {
                        for (gi, &si) in sigs.iter().enumerate() {
                            self.wave_rows.push(WaveRow::Signal { sig_idx: si, pin_idx: pi, grp_idx: Some(gi) });
                            if self.wave_expanded.contains(&si) {
                                for bit in (0..vcd.signals[si].width).rev() {
                                    self.wave_rows.push(WaveRow::BitSlice { sig_idx: si, bit });
                                }
                            }
                        }
                    }
                }
            }
        }
        self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
    }

    // ── Group & spacer helpers ─────────────────────────────────────────────────
    fn insert_spacer(&mut self) {
        let pos = self.wave_insert_pos();
        self.items.insert(pos, PinItem::Spacer);
        self.rebuild_wave();
        self.wave_sel = (self.wave_sel + 1).min(self.wave_rows.len().saturating_sub(1));
    }

    fn wave_insert_pos(&self) -> usize {
        match self.wave_rows.get(self.wave_sel) {
            Some(WaveRow::Signal   { pin_idx, grp_idx: None, .. }) => pin_idx + 1,
            Some(WaveRow::GroupHeader { pin_idx, .. })             => pin_idx + 1,
            Some(WaveRow::Spacer   { pin_idx })                    => pin_idx + 1,
            _ => self.items.len(),
        }
    }

    fn create_group(&mut self, name: String) {
        match self.wave_rows.get(self.wave_sel).cloned() {
            Some(WaveRow::Signal { sig_idx, pin_idx, grp_idx: None }) => {
                self.items.remove(pin_idx);
                self.items.insert(pin_idx, PinItem::Group { name, collapsed: false, sigs: vec![sig_idx] });
                self.rebuild_wave();
            }
            _ => {
                let pos = self.wave_insert_pos();
                self.items.insert(pos, PinItem::Group { name, collapsed: false, sigs: vec![] });
                self.rebuild_wave();
            }
        }
    }

    fn move_to_group(&mut self) {
        let (si, pi) = match self.wave_rows.get(self.wave_sel) {
            Some(WaveRow::Signal { sig_idx, pin_idx, grp_idx: None }) => (*sig_idx, *pin_idx),
            _ => { self.status = "Select a top-level signal to move into a group".into(); return; }
        };
        let grp_pi = (0..pi).rev().find(|&i| matches!(self.items.get(i), Some(PinItem::Group{..})));
        let Some(grp_pi) = grp_pi else {
            self.status = "No group above — press 'g' to create one first".into(); return;
        };
        self.items.remove(pi);
        if let Some(PinItem::Group { sigs, .. }) = self.items.get_mut(grp_pi) { sigs.push(si); }
        self.rebuild_wave();
    }

    // ── Scroll helpers ────────────────────────────────────────────────────────
    fn scroll_mod(&mut self, win_h: i16) {
        let mod_h  = ((win_h - HEADER_H - STATUS_H) as f32 * MOD_SPLIT) as i16;
        let vr     = self.mod_vis_rows(mod_h);
        if self.mod_sel < self.mod_scroll { self.mod_scroll = self.mod_sel; }
        else if self.mod_sel >= self.mod_scroll + vr { self.mod_scroll = self.mod_sel + 1 - vr; }
    }

    fn scroll_sig(&mut self, win_h: i16) {
        let body_h = win_h - HEADER_H - STATUS_H;
        let mod_h  = (body_h as f32 * MOD_SPLIT) as i16;
        let sig_h  = body_h - mod_h;
        let vr     = self.sig_vis_rows(sig_h);
        if self.sig_sel < self.sig_scroll { self.sig_scroll = self.sig_sel; }
        else if self.sig_sel >= self.sig_scroll + vr { self.sig_scroll = self.sig_sel + 1 - vr; }
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
            Some(WaveRow::Signal   { sig_idx, .. }) |
            Some(WaveRow::BitSlice { sig_idx, .. }) => *sig_idx,
            _ => return,
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

    // ── Key handler ───────────────────────────────────────────────────────────
    fn handle_keysym(&mut self, ks: u32, win_h: i16) -> bool {
        // Global
        match ks {
            k if k == 0x71 || k == 0x51 || k == XK_ESCAPE => return true,
            0x73 | 0x53 => { self.load_text(SAMPLE_VCD, "sample.vcd"); return false; }
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
            0x5B => { if let Some(t) = self.cursor { self.cursor = Some((t - self.max_time()/self.zoom*0.02).max(0.0)); } return false; }
            0x5D => { if let Some(t) = self.cursor { self.cursor = Some((t + self.max_time()/self.zoom*0.02).min(self.max_time())); } return false; }
            0x6E => { self.jump_edge(true);  return false; }
            0x4E => { self.jump_edge(false); return false; }
            0x3F => {
                self.status = "Tab=panels | MOD: j/k Enter=expand A=add-scope | SIG: j/k a=add/remove A=add-scope | WAVE: j/k=nav J/K=reorder d=remove e=expand  g=group i=spacer m=move-to-group Enter=toggle-group | +/-/f=zoom h/l c=cursor n/N q".into();
                return false;
            }
            _ => {}
        }
        // If a pending operation is active, route keys there first
        if self.pending.is_some() && self.focus == Focus::Wave {
            self.key_pending(ks, win_h);
            return false;
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
                let mod_h = ((win_h - HEADER_H - STATUS_H) as f32 * MOD_SPLIT) as i16;
                let vr = self.mod_vis_rows(mod_h);
                self.mod_sel = self.mod_sel.saturating_sub(vr); self.scroll_mod(win_h); self.rebuild_sig_rows();
            }
            k if k == XK_PAGE_DOWN => {
                let mod_h = ((win_h - HEADER_H - STATUS_H) as f32 * MOD_SPLIT) as i16;
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
                    self.status = format!("Added all signals in: {}", p);
                }
            }
            _ => {}
        }
        if let Some(p) = self.selected_scope() { self.status = format!("Module: {}", p); }
    }

    fn key_sig(&mut self, ks: u32, win_h: i16) {
        let n = self.sig_rows.len();
        match ks {
            k if k == XK_UP   || k == 0x6B => { if self.sig_sel > 0 { self.sig_sel -= 1; } self.scroll_sig(win_h); }
            k if k == XK_DOWN || k == 0x6A => { if n > 0 && self.sig_sel + 1 < n { self.sig_sel += 1; } self.scroll_sig(win_h); }
            k if k == XK_PAGE_UP   => {
                let body_h = win_h - HEADER_H - STATUS_H;
                let mod_h  = (body_h as f32 * MOD_SPLIT) as i16;
                let vr     = self.sig_vis_rows(body_h - mod_h);
                self.sig_sel = self.sig_sel.saturating_sub(vr); self.scroll_sig(win_h);
            }
            k if k == XK_PAGE_DOWN => {
                let body_h = win_h - HEADER_H - STATUS_H;
                let mod_h  = (body_h as f32 * MOD_SPLIT) as i16;
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
                    self.status = format!("Added all in: {}", p);
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
                    Some(WaveRow::Signal { sig_idx, .. }) => {
                        self.unpin(sig_idx);
                        self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
                    }
                    Some(WaveRow::BitSlice { sig_idx, .. }) => {
                        self.wave_expanded.remove(&sig_idx); self.rebuild_wave();
                        self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
                    }
                    Some(WaveRow::GroupHeader { pin_idx, .. }) => {
                        // Dissolve group — promote signals to top-level
                        if let Some(PinItem::Group { sigs, .. }) = self.items.get(pin_idx).cloned() {
                            self.items.remove(pin_idx);
                            for (i, si) in sigs.into_iter().enumerate() {
                                self.items.insert(pin_idx + i, PinItem::Sig(si));
                            }
                        }
                        self.rebuild_wave();
                        self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
                    }
                    Some(WaveRow::Computed { pin_idx, .. }) => {
                        self.items.remove(pin_idx);
                        self.rebuild_wave();
                        self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
                    }
                    Some(WaveRow::Spacer { pin_idx }) => {
                        self.items.remove(pin_idx);
                        self.rebuild_wave();
                        self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
                    }
                    None => {}
                }
            }
            // J — move down
            0x4A => {
                match self.wave_rows.get(self.wave_sel).cloned() {
                    Some(WaveRow::Signal { sig_idx, pin_idx, grp_idx: None }) => {
                        if pin_idx + 1 < self.items.len() {
                            self.items.swap(pin_idx, pin_idx+1); self.rebuild_wave();
                            if let Some(p) = self.wave_rows.iter().position(|r| matches!(r, WaveRow::Signal{sig_idx:s,grp_idx:None,..} if *s==sig_idx)) { self.wave_sel = p; }
                        }
                    }
                    Some(WaveRow::Signal { sig_idx, pin_idx, grp_idx: Some(gi) }) => {
                        if let Some(PinItem::Group{sigs,..}) = self.items.get_mut(pin_idx) {
                            if gi + 1 < sigs.len() { sigs.swap(gi, gi+1); self.rebuild_wave();
                                if let Some(p) = self.wave_rows.iter().position(|r| matches!(r, WaveRow::Signal{sig_idx:s,..} if *s==sig_idx)) { self.wave_sel = p; }
                            }
                        }
                    }
                    Some(WaveRow::GroupHeader { pin_idx, .. }) => {
                        if pin_idx + 1 < self.items.len() {
                            self.items.swap(pin_idx, pin_idx+1); self.rebuild_wave();
                            self.wave_sel = self.wave_sel.min(self.wave_rows.len().saturating_sub(1));
                        }
                    }
                    _ => {}
                }
            }
            // K — move up
            0x4B => {
                match self.wave_rows.get(self.wave_sel).cloned() {
                    Some(WaveRow::Signal { sig_idx, pin_idx, grp_idx: None }) => {
                        if pin_idx > 0 {
                            self.items.swap(pin_idx, pin_idx-1); self.rebuild_wave();
                            if let Some(p) = self.wave_rows.iter().position(|r| matches!(r, WaveRow::Signal{sig_idx:s,grp_idx:None,..} if *s==sig_idx)) { self.wave_sel = p; }
                        }
                    }
                    Some(WaveRow::Signal { sig_idx, pin_idx, grp_idx: Some(gi) }) => {
                        if let Some(PinItem::Group{sigs,..}) = self.items.get_mut(pin_idx) {
                            if gi > 0 { sigs.swap(gi, gi-1); self.rebuild_wave();
                                if let Some(p) = self.wave_rows.iter().position(|r| matches!(r, WaveRow::Signal{sig_idx:s,..} if *s==sig_idx)) { self.wave_sel = p; }
                            }
                        }
                    }
                    Some(WaveRow::GroupHeader { pin_idx, .. }) => {
                        if pin_idx > 0 {
                            self.items.swap(pin_idx, pin_idx-1); self.rebuild_wave();
                            self.wave_sel = self.wave_sel.saturating_sub(1);
                        }
                    }
                    _ => {}
                }
            }
            // e / Enter — expand bus OR collapse/expand group
            k if k == XK_RETURN || k == 0x65 || k == 0x45 => {
                match self.wave_rows.get(self.wave_sel).cloned() {
                    Some(WaveRow::Signal { sig_idx, .. }) => {
                        if let Some(vcd) = &self.vcd {
                            if vcd.signals[sig_idx].width > 1 {
                                if self.wave_expanded.contains(&sig_idx) { self.wave_expanded.remove(&sig_idx); }
                                else { self.wave_expanded.insert(sig_idx); }
                                self.rebuild_wave();
                            }
                        }
                    }
                    Some(WaveRow::BitSlice { sig_idx, .. }) => { self.wave_expanded.remove(&sig_idx); self.rebuild_wave(); }
                    Some(WaveRow::GroupHeader { pin_idx, collapsed, .. }) => {
                        if let Some(PinItem::Group { collapsed: c, .. }) = self.items.get_mut(pin_idx) { *c = !collapsed; }
                        self.rebuild_wave();
                    }
                    _ => {}
                }
            }
            // i — insert spacer
            0x69 => { self.insert_spacer(); self.status = "Spacer inserted  (d to remove)".into(); }
            // g — create/wrap group
            0x67 => { self.create_group("Group".into()); self.status = "Group created  (Enter=collapse  d=dissolve  m=add signal)".into(); }
            // m — move signal into nearest group above
            0x6D => { self.move_to_group(); }
            // o — start operation on selected signal
            0x6F => {
                match self.wave_rows.get(self.wave_sel).cloned() {
                    Some(WaveRow::Signal{..}) | Some(WaveRow::Computed{..}) => {
                        let src_row  = self.wave_sel;
                        let src_name = self.row_short_name(src_row);
                        self.status = format!("Op on '{}'  →  &=AND  |=OR  ^=XOR  ~=NOT  !=NAND  %=NOR  @=XNOR  <=SHL  >=SHR  Esc=cancel", src_name);
                        self.pending = Some(Pending::SelectOp { src_row, src_name });
                    }
                    _ => { self.status = "Select a signal first, then press 'o'".into(); }
                }
            }
            _ => {
                let s = match self.wave_rows.get(self.wave_sel) {
                    Some(WaveRow::Signal{sig_idx,..}) =>
                        self.vcd.as_ref().map(|v| v.signals[*sig_idx].full_name.clone()).unwrap_or_default(),
                    Some(WaveRow::BitSlice{sig_idx,bit,..}) =>
                        self.vcd.as_ref().map(|v| format!("{}[{}]", v.signals[*sig_idx].full_name, bit)).unwrap_or_default(),
                    Some(WaveRow::Computed{comp_idx,..}) => format!("= {}", self.computed[*comp_idx].name),
                    Some(WaveRow::GroupHeader{name,..}) => format!("▣ Group: {}", name),
                    Some(WaveRow::Spacer{..}) => "— spacer  (d=remove  i=add another)".into(),
                    None => String::new(),
                };
                if !s.is_empty() { self.status = s; }
            }
        }
    }

    // ── Computed signal engine ────────────────────────────────────────────────
    fn sig_changes_for_row(&self, row_idx: usize) -> (Vec<vcd_parser::ValueChange>, usize) {
        match self.wave_rows.get(row_idx) {
            Some(WaveRow::Signal { sig_idx, .. }) => {
                let vcd = self.vcd.as_ref().unwrap();
                let sig = &vcd.signals[*sig_idx];
                (vcd.changes.get(&sig.id).cloned().unwrap_or_default(), sig.width)
            }
            Some(WaveRow::Computed { comp_idx, .. }) => {
                let c = &self.computed[*comp_idx];
                (c.changes.clone(), c.width)
            }
            _ => (vec![], 1),
        }
    }

    fn compute_unary(op: &UnaryOp, src: &[vcd_parser::ValueChange], width: usize) -> Vec<vcd_parser::ValueChange> {
        src.iter().map(|vc| {
            let v = val_to_u64(&vc.value, width);
            let result = match op {
                UnaryOp::Not       => !v & ((1u64.wrapping_shl(width as u32)).wrapping_sub(1)),
                UnaryOp::ShiftL(n) => (v << n) & ((1u64.wrapping_shl(width as u32)).wrapping_sub(1)),
                UnaryOp::ShiftR(n) => v >> n,
            };
            vcd_parser::ValueChange { time: vc.time, value: u64_to_bin(result, width) }
        }).collect()
    }

    fn compute_binary(op: &BinOp, a: &[vcd_parser::ValueChange], b: &[vcd_parser::ValueChange], width: usize) -> Vec<vcd_parser::ValueChange> {
        // Merge all timestamps
        let mut times: Vec<u64> = a.iter().map(|v| v.time).chain(b.iter().map(|v| v.time)).collect();
        times.sort_unstable(); times.dedup();
        times.into_iter().map(|t| {
            let av = val_from_changes(a, t, width);
            let bv = val_from_changes(b, t, width);
            let mask = (1u64.wrapping_shl(width as u32)).wrapping_sub(1);
            let r = match op {
                BinOp::And  => av & bv,  BinOp::Or   => av | bv,
                BinOp::Xor  => av ^ bv,  BinOp::Nand => !(av & bv) & mask,
                BinOp::Nor  => !(av | bv) & mask, BinOp::Xnor => !(av ^ bv) & mask,
                BinOp::ShiftL(n) => (av << n) & mask,
                BinOp::ShiftR(n) => av >> n,
            };
            vcd_parser::ValueChange { time: t, value: u64_to_bin(r, width) }
        }).collect()
    }

    fn create_computed_unary(&mut self, op: UnaryOp, src_row: usize) {
        let (src_ch, width) = self.sig_changes_for_row(src_row);
        let src_name = self.row_short_name(src_row);
        let name = format!("{}({})", op.symbol(), src_name);
        let changes = Self::compute_unary(&op, &src_ch, width);
        let ci = self.computed.len();
        self.computed.push(Computed { name, op: CompOp::Unary { op, src: src_row }, width, changes });
        self.items.push(PinItem::Computed(ci));
        self.rebuild_wave();
        self.status = format!("Created computed signal #{}", ci);
    }

    fn create_computed_binary(&mut self, op: BinOp, src_a: usize, src_b: usize) {
        let (a_ch, wa) = self.sig_changes_for_row(src_a);
        let (b_ch, wb) = self.sig_changes_for_row(src_b);
        let width = wa.max(wb);
        let a_name = self.row_short_name(src_a);
        let b_name = self.row_short_name(src_b);
        let name = format!("({} {} {})", a_name, op.symbol(), b_name);
        let changes = Self::compute_binary(&op, &a_ch, &b_ch, width);
        let ci = self.computed.len();
        self.computed.push(Computed { name, op: CompOp::Binary { op, src_a, src_b }, width, changes });
        self.items.push(PinItem::Computed(ci));
        self.rebuild_wave();
        self.status = format!("Created computed signal #{}", ci);
    }

    fn row_short_name(&self, row_idx: usize) -> String {
        match self.wave_rows.get(row_idx) {
            Some(WaveRow::Signal { sig_idx, .. }) =>
                self.vcd.as_ref().map(|v| v.signals[*sig_idx].name.clone()).unwrap_or_default(),
            Some(WaveRow::Computed { comp_idx, .. }) =>
                self.computed[*comp_idx].name.clone(),
            _ => format!("row{}", row_idx),
        }
    }

    // ── Pending operation key handler ─────────────────────────────────────────
    fn key_pending(&mut self, ks: u32, win_h: i16) -> bool {
        let pending = match self.pending.take() { Some(p) => p, None => return false };

        if ks == XK_ESCAPE { self.status = "Cancelled".into(); return true; }

        match pending {
            Pending::SelectOp { src_row, src_name } => {
                let op: Option<BinOp> = match ks {
                    0x26 => Some(BinOp::And),   // &
                    0x7C => Some(BinOp::Or),    // |
                    0x5E => Some(BinOp::Xor),   // ^
                    0x21 => Some(BinOp::Nand),  // !  (nand shorthand)
                    0x25 => Some(BinOp::Nor),   // %  (nor shorthand)
                    0x40 => Some(BinOp::Xnor),  // @  (xnor shorthand)
                    _ => None,
                };
                if let Some(op) = op {
                    let sym = op.symbol().to_string();
                    self.status = format!("Op: {}  |  Navigate j/k to second signal, Enter to confirm, Esc=cancel", sym);
                    self.pending = Some(Pending::SelectB { op, src_a: src_row, src_name });
                    return true;
                }
                // Unary ops
                match ks {
                    0x7E => { // ~ NOT
                        self.create_computed_unary(UnaryOp::Not, src_row); return true;
                    }
                    0x3C => { // < start shift-left entry
                        self.status = "Shift left by: type amount (0-63) then Enter".into();
                        self.pending = Some(Pending::ShiftAmt { unary: true, src_row, src_name, op_sym: "<<".into(), amt: String::new() });
                        return true;
                    }
                    0x3E => { // > start shift-right entry
                        self.status = "Shift right by: type amount (0-63) then Enter".into();
                        self.pending = Some(Pending::ShiftAmt { unary: true, src_row, src_name, op_sym: ">>".into(), amt: String::new() });
                        return true;
                    }
                    _ => {
                        self.status = format!("Unknown op (Esc=cancel)  &=AND  |=OR  ^=XOR  ~=NOT  !=NAND  %=NOR  @=XNOR  <=SHL  >=SHR");
                        self.pending = Some(Pending::SelectOp { src_row, src_name });
                        return true;
                    }
                }
            }
            Pending::SelectB { op, src_a, src_name } => {
                // navigation still works normally; Enter confirms
                let vr = self.wave_vis_rows(win_h);
                let n  = self.wave_rows.len();
                match ks {
                    k if k == XK_UP   || k == 0x6B => { if self.wave_sel > 0 { self.wave_sel -= 1; } self.scroll_wave(win_h); }
                    k if k == XK_DOWN || k == 0x6A => { if n>0&&self.wave_sel+1<n{self.wave_sel+=1;} self.scroll_wave(win_h); }
                    k if k == XK_PAGE_UP   => { self.wave_sel = self.wave_sel.saturating_sub(vr); self.scroll_wave(win_h); }
                    k if k == XK_PAGE_DOWN => { self.wave_sel = (self.wave_sel+vr).min(n.saturating_sub(1)); self.scroll_wave(win_h); }
                    k if k == XK_RETURN => {
                        let src_b = self.wave_sel;
                        self.create_computed_binary(op, src_a, src_b);
                        return true;
                    }
                    _ => {}
                }
                let b_name = self.row_short_name(self.wave_sel);
                self.status = format!("({} {} ?)  →  navigate to second signal, Enter to confirm  |  current: {}",
                    src_name, op.symbol(), b_name);
                self.pending = Some(Pending::SelectB { op, src_a, src_name });
                return true;
            }
            Pending::ShiftAmt { unary, src_row, src_name, op_sym, mut amt } => {
                match ks {
                    k if k == XK_RETURN => {
                        let n: u8 = amt.parse().unwrap_or(1).min(63);
                        let is_left = op_sym == "<<";
                        if unary {
                            let op = if is_left { UnaryOp::ShiftL(n) } else { UnaryOp::ShiftR(n) };
                            self.create_computed_unary(op, src_row);
                        } else {
                            let op = if is_left { BinOp::ShiftL(n) } else { BinOp::ShiftR(n) };
                            // No: shift-as-binary doesn't make sense, treat as unary
                            self.create_computed_unary(if is_left { UnaryOp::ShiftL(n) } else { UnaryOp::ShiftR(n) }, src_row);
                        }
                        return true;
                    }
                    k if k == XK_BACKSPACE => { amt.pop(); }
                    // digit keys 0-9
                    k if k >= 0x30 && k <= 0x39 => {
                        if amt.len() < 2 { amt.push((k as u8 - 0x30 + b'0') as char); }
                    }
                    _ => {}
                }
                self.status = format!("{} {} {} — type amount (Enter to confirm, Esc=cancel)", src_name, op_sym, amt);
                self.pending = Some(Pending::ShiftAmt { unary, src_row, src_name, op_sym, amt });
                return true;
            }
        }
    }

    fn handle_button(&mut self, button: u8, mouse_x: i16, win_w: i16) {
        let wx    = LEFT_W + NAME_W;
        let ww    = (win_w - wx).max(1) as f64;
        let rel   = (mouse_x - wx).max(0);
        let frac  = (rel as f64).clamp(0.0, ww-1.0) / ww;
        let pivot = self.view_start + frac * (self.max_time() / self.zoom);
        match button {
            4 => self.zoom_by(2.0, Some(pivot)),
            5 => self.zoom_by(0.5, Some(pivot)),
            _ => {}
        }
    }
}

// ── Value arithmetic helpers ─────────────────────────────────────────────────

fn val_to_u64(val: &str, width: usize) -> u64 {
    if val.chars().any(|c| c=='x'||c=='X'||c=='z'||c=='Z') { return 0; }
    u64::from_str_radix(val, 2).unwrap_or(0)
}

fn u64_to_bin(v: u64, width: usize) -> String {
    if width == 0 { return "0".into(); }
    let w = width.min(64);
    (0..w).rev().map(|i| if (v >> i) & 1 == 1 { '1' } else { '0' }).collect()
}

fn val_from_changes(changes: &[vcd_parser::ValueChange], time: u64, width: usize) -> u64 {
    let idx = changes.partition_point(|vc| vc.time <= time);
    if idx == 0 { return 0; }
    val_to_u64(&changes[idx-1].value, width)
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

fn fmt_val(val: &str, width: usize) -> String {
    if width == 1 { return val.to_uppercase(); }
    if val.chars().any(|c| c=='x'||c=='X') { return "X".into(); }
    match u64::from_str_radix(val, 2) {
        Ok(n) => format!("{:#X}", n),
        Err(_) => val[..val.len().min(8)].to_string(),
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

fn render(conn: &RustConnection, pix: u32, gc: u32, font: u32, w: u16, h: u16, app: &App) {
    let (w, h) = (w as i16, h as i16);
    fill(conn, pix, gc, C_BG, 0, 0, w as u16, h as u16);

    // Header
    fill(conn, pix, gc, C_HEADER, 0, 0, w as u16, HEADER_H as u16);
    let hdr = if let Some(vcd) = &app.vcd {
        let cur = app.cursor.map(|t| format!("  T={:.0}{}", t, vcd.timescale)).unwrap_or_default();
        format!(" VCD  {}  z={:.1}x  {:.0}..{:.0}{}  pinned={}",
            app.filename, app.zoom, app.view_start, app.view_end(), cur, app.pin_count())
    } else {
        " VCD VIEWER — s=sample  Tab=cycle panels  q=quit  ?=help".into()
    };
    txt(conn, pix, gc, font, C_HI, C_HEADER, 2, 4, &hdr);
    seg(conn, pix, gc, C_SEP, 0, HEADER_H-1, w, HEADER_H-1); // header separator

    // Status
    let sy = h - STATUS_H;
    seg(conn, pix, gc, C_SEP, 0, sy, w, sy);              // status separator
    fill(conn, pix, gc, C_HEADER, 0, sy, w as u16, STATUS_H as u16);
    let (fl, fc) = match app.focus {
        Focus::ModTree => (" MODULE ", C_MOD_SEL),
        Focus::SigList => (" SIGNALS ", C_PINNED),
        Focus::Wave    => (" WAVE ", C_HI),
    };
    txt(conn, pix, gc, font, fc, C_HEADER, 2, sy+2, fl);
    txt(conn, pix, gc, font, C_DIM, C_HEADER, tw(fl)+6, sy+2, &app.status);

    let body_h  = h - HEADER_H - STATUS_H;
    let mod_h   = (body_h as f32 * MOD_SPLIT) as i16;
    let sig_h   = body_h - mod_h;
    let mod_y   = HEADER_H;
    let sig_y   = HEADER_H + mod_h;

    // Left panel background
    fill(conn, pix, gc, C_PANEL, 0, HEADER_H, LEFT_W as u16, body_h as u16);

    // Divider between module tree and signal list
    let bdl = if app.focus == Focus::ModTree { C_BDR_FOCUS } else { C_BDR };
    let bds = if app.focus == Focus::SigList { C_BDR_FOCUS } else { C_BDR };
    seg(conn, pix, gc, C_SEP, 0, sig_y, LEFT_W, sig_y);  // module/signal list separator
    // Right edge of left panel
    let bdr = if app.focus != Focus::Wave { C_BDR_FOCUS } else { C_BDR };
    seg(conn, pix, gc, C_SEP, LEFT_W, HEADER_H, LEFT_W, h - STATUS_H); // panel/wave separator

    // ── Module tree ───────────────────────────────────────────────────────────
    // Header bar
    fill(conn, pix, gc, C_MOD_BG, 0, mod_y, LEFT_W as u16, ROW_H as u16);
    let scope_lbl = app.selected_scope().unwrap_or_else(|| "Modules".into());
    let hdr_txt = trunc_r(&format!(" ▸ {} ", scope_lbl), ((LEFT_W-4)/FW) as usize);
    txt(conn, pix, gc, font, C_MOD_LBL, C_MOD_BG, 2, mod_y+5, &hdr_txt);

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
        fill(conn, pix, gc, bg, 0, ry, LEFT_W as u16, ROW_H as u16);

        let px     = depth as i16 * INDENT + 4;
        let marker = if has_ch { if exp { "▼ " } else { "▶ " } } else { "  " };
        let col    = if is_sel { C_MOD_SEL } else { C_MOD_LBL };
        let lbl    = format!("{}{}", marker, name);
        let max_c  = ((LEFT_W - px - 4) / FW).max(0) as usize;
        txt(conn, pix, gc, font, col, bg, px, ry+6, &trunc_r(&lbl, max_c));

        // Selection bar on left edge
        if is_sel { fill(conn, pix, gc, C_MOD_SEL, 0, ry, 2, ROW_H as u16); }
        seg(conn, pix, gc, C_BDR, 0, ry+ROW_H-1, LEFT_W, ry+ROW_H-1);
    }

    // ── Signal list ───────────────────────────────────────────────────────────
    // Header bar
    fill(conn, pix, gc, C_PANEL, 0, sig_y, LEFT_W as u16, ROW_H as u16);
    let scope = app.selected_scope().unwrap_or_default();
    let sig_hdr = format!(" Signals ({}) ", app.sig_rows.len());
    txt(conn, pix, gc, font, if app.focus==Focus::SigList {C_PINNED} else {C_DIM},
        C_PANEL, 2, sig_y+5, &sig_hdr);

    let sig_rows_y = sig_y + ROW_H;
    let sig_avail  = ((sig_h - ROW_H) / SIG_H).max(0) as usize;
    let sscroll    = app.sig_scroll.min(app.sig_rows.len().saturating_sub(1));

    for (ri, &si) in app.sig_rows.iter().enumerate().skip(sscroll).take(sig_avail) {
        let ry     = sig_rows_y + (ri - sscroll) as i16 * SIG_H;
        if ry + SIG_H > h - STATUS_H { break; }
        let is_sel = ri == app.sig_sel && app.focus == Focus::SigList;
        let pinned = app.is_pinned(si);
        let bg     = if is_sel { C_SEL_SIG } else { C_PANEL };
        fill(conn, pix, gc, bg, 0, ry, LEFT_W as u16, SIG_H as u16);

        if let Some(vcd) = &app.vcd {
            let sig   = &vcd.signals[si];
            let wstr  = if sig.width > 1 { format!("[{}:0]", sig.width-1) } else { String::new() };
            let name  = format!("{}{}", sig.name, wstr);
            let max_c = ((LEFT_W - 16) / FW).max(0) as usize;
            let col   = if pinned { C_LBL } else { C_DIM };
            txt(conn, pix, gc, font, col, bg, 16, ry+(SIG_H-13)/2, &trunc_r(&name, max_c));

            // Pin marker
            if pinned { txt(conn, pix, gc, font, C_PINNED, bg, 4, ry+(SIG_H-13)/2, "►"); }
            if is_sel { fill(conn, pix, gc, C_PINNED, 0, ry, 2, SIG_H as u16); }
        }
        seg(conn, pix, gc, C_BDR, 0, ry+SIG_H-1, LEFT_W, ry+SIG_H-1);
    }

    // ── Waveform area ─────────────────────────────────────────────────────────
    let wx     = LEFT_W;
    let wave_x = wx + NAME_W;
    let wave_w = w - wave_x;

    let Some(vcd) = &app.vcd else {
        txt(conn, pix, gc, font, C_DIM, C_BG, wx+50, HEADER_H+body_h/2, "Select module, then press 'a' to add signals");
        return;
    };

    // Name column divider
    let wbdr = if app.focus == Focus::Wave { C_BDR_FOCUS } else { C_BDR };
    seg(conn, pix, gc, C_SEP, wave_x, HEADER_H, wave_x, h-STATUS_H); // name/wave separator

    // Ruler
    render_ruler(conn, pix, gc, font, wave_x, HEADER_H, wave_w,
        app.view_start, app.view_end(), app.max_time(), &vcd.timescale, app.cursor);

    // End-of-sim marker
    let t0e = app.view_start; let t1e = app.view_end();
    let rng = (t1e - t0e).max(1.0);
    let mt  = app.max_time();
    if mt >= t0e && mt <= t1e {
        let mx = wave_x + ((mt - t0e)/rng * wave_w as f64).clamp(0.0, wave_w as f64-1.0) as i16;
        seg(conn, pix, gc, C_HI, mx, HEADER_H+RULER_H, mx, HEADER_H+body_h);
    }

    let base_y   = HEADER_H + RULER_H;
    let avail_h  = body_h - RULER_H;
    let max_rows = (avail_h / WAVE_H).max(1) as usize;
    let wscroll  = app.wave_scroll.min(app.wave_rows.len().saturating_sub(1));

    for (ri, row) in app.wave_rows.iter().enumerate().skip(wscroll).take(max_rows) {
        let ry     = base_y + (ri - wscroll) as i16 * WAVE_H;
        if ry + WAVE_H > base_y + avail_h { break; }
        let is_sel = ri == app.wave_sel;
        let wbg    = if ri%2==0 { C_BG } else { C_WAVE_ALT };

        match row {
            WaveRow::Computed { comp_idx, .. } => {
                let comp = &app.computed[*comp_idx];
                let nbg  = if is_sel && app.focus==Focus::Wave { C_COMP_SEL } else { C_COMP_BG };
                fill(conn, pix, gc, nbg, wx, ry, NAME_W as u16, WAVE_H as u16);
                // Full expression (top)
                let max_c = ((NAME_W-8)/FW).max(0) as usize;
                txt(conn, pix, gc, font, C_COMP_LBL, nbg, wx+4, ry+2, &trunc_r(&comp.name, max_c));
                // Width (bottom right)
                let wstr = format!("[{}:0]", comp.width-1);
                txt(conn, pix, gc, font, C_DIM, nbg, wx+4, ry+WAVE_H/2, &trunc_r(&wstr, max_c));
                // Cursor value
                if let Some(t) = app.cursor {
                    let v = val_from_changes(&comp.changes, t as u64, comp.width);
                    let dv = if comp.width==1 { format!("{}", v) } else { format!("{:#X}", v) };
                    let vx = wx + NAME_W - 2 - tw(&dv);
                    if vx > wx+4 { txt(conn, pix, gc, font, C_CUR, nbg, vx, ry+WAVE_H/2, &dv); }
                }
                if is_sel { seg(conn, pix, gc, C_COMP_LBL, wx, ry, wx, ry+WAVE_H-1); }
                // Waveform
                fill(conn, pix, gc, wbg, wave_x, ry, wave_w as u16, WAVE_H as u16);
                if is_sel { fill(conn, pix, gc, if app.focus==Focus::Wave{C_COMP_LBL}else{C_DIM}, wave_x, ry, 2, WAVE_H as u16); }
                render_wave(conn, pix, gc, font, wave_x, ry, wave_w, &comp.changes, comp.width,
                    app.view_start, app.view_end(), app.cursor, wbg);
                seg(conn, pix, gc, C_BDR, wx, ry+WAVE_H-1, w, ry+WAVE_H-1);
                continue;
            }
            WaveRow::GroupHeader { name, collapsed, .. } => {
                let bg = if is_sel { C_GRP_SEL } else { C_GRP_BG };
                fill(conn, pix, gc, bg, wx, ry, (NAME_W + wave_w) as u16, WAVE_H as u16);
                let marker = if *collapsed { "▶ " } else { "▼ " };
                let lbl    = format!("{}{}", marker, name);
                let max_c  = ((NAME_W + wave_w - 8) / FW).max(0) as usize;
                txt(conn, pix, gc, font, C_GRP_LBL, bg, wx + 6, ry + (WAVE_H-13)/2, &trunc_r(&lbl, max_c));
                if is_sel { seg(conn, pix, gc, C_GRP_LBL, wx, ry, wx, ry+WAVE_H-1); }
                seg(conn, pix, gc, C_SEP, wx, ry+WAVE_H-1, wx+NAME_W+wave_w, ry+WAVE_H-1);
                continue;
            }
            WaveRow::Spacer { .. } => {
                fill(conn, pix, gc, C_SPACER, wx, ry, (NAME_W + wave_w) as u16, WAVE_H as u16);
                dashed(conn, pix, gc, C_DIM, wx, ry + WAVE_H/2, wx + NAME_W + wave_w);
                if is_sel {
                    seg(conn, pix, gc, C_GRP_LBL, wx, ry, wx+NAME_W+wave_w, ry);
                    seg(conn, pix, gc, C_GRP_LBL, wx, ry+WAVE_H-1, wx+NAME_W+wave_w, ry+WAVE_H-1);
                }
                continue;
            }
            WaveRow::Signal { sig_idx, grp_idx, .. } => {
                let in_group = grp_idx.is_some();
                let si  = *sig_idx;
                let sig = &vcd.signals[si];

                // Name column
                let nbg = if is_sel && app.focus==Focus::Wave { C_SEL_WAVE } else { C_PANEL };
                fill(conn, pix, gc, nbg, wx, ry, NAME_W as u16, WAVE_H as u16);
                if in_group { fill(conn, pix, gc, C_GRP_LBL, wx, ry, 3, WAVE_H as u16); }

                // Full path (top, dim, small)
                let max_c   = ((NAME_W - 8) / FW).max(0) as usize;
                let full    = trunc_l(&sig.full_name, max_c);
                txt(conn, pix, gc, font, C_PATH, nbg, wx+4, ry+2, &full);

                // Short name (bottom, bright)
                let wstr   = if sig.width > 1 { format!("[{}:0]", sig.width-1) } else { String::new() };
                let short  = format!("{}{}", sig.name, wstr);
                let smax   = ((NAME_W - 8) / FW).max(0) as usize;
                txt(conn, pix, gc, font, C_LBL, nbg, wx+4, ry+WAVE_H/2, &trunc_r(&short, smax));

                // Cursor value
                if let Some(t) = app.cursor {
                    let val = vcd.get_value_at(&sig.id, t as u64);
                    let dv  = fmt_val(&val, sig.width);
                    let vx  = wx + NAME_W - 2 - tw(&dv);
                    if vx > wx + 4 { txt(conn, pix, gc, font, C_CUR, nbg, vx, ry+WAVE_H/2, &dv); }
                }

                // Expand marker
                if sig.width > 1 {
                    let e = app.wave_expanded.contains(&si);
                    txt(conn, pix, gc, font, C_BIT_LBL, nbg, wx+NAME_W-FW*2-2, ry+2, if e {"▼"} else {"▶"});
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
                fill(conn, pix, gc, nbg, wx, ry, NAME_W as u16, WAVE_H as u16);

                let path = format!("{}[{}]", sig.full_name, bit);
                let max_c = ((NAME_W-8)/FW).max(0) as usize;
                txt(conn, pix, gc, font, C_BIT_LBL, nbg, wx+4, ry+(WAVE_H-13)/2, &trunc_l(&path, max_c));

                if let Some(t) = app.cursor {
                    let raw = vcd.get_value_at(&sig.id, t as u64);
                    let bv  = extract_bit(&raw, *bit);
                    let vx  = wx + NAME_W - 2 - tw(&bv);
                    txt(conn, pix, gc, font, C_CUR, nbg, vx, ry+(WAVE_H-13)/2, &bv);
                }

                fill(conn, pix, gc, wbg, wave_x, ry, wave_w as u16, WAVE_H as u16);
                if is_sel { fill(conn, pix, gc, if app.focus==Focus::Wave{C_BIT_LBL}else{C_DIM}, wave_x, ry, 2, WAVE_H as u16); }
                let raw_ch = vcd.changes.get(&sig.id).map(|v|v.as_slice()).unwrap_or(&[]);
                let bit_ch = synth_bit_changes(raw_ch, *bit);
                render_wave(conn, pix, gc, font, wave_x, ry, wave_w, &bit_ch, 1,
                    app.view_start, app.view_end(), app.cursor, wbg);
            }
        }
        seg(conn, pix, gc, C_BDR, wx, ry+WAVE_H-1, w, ry+WAVE_H-1);
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
            seg(conn, pix, gc, C_DIM, tx, y+RULER_H-7, tx, y+RULER_H-1);
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
    seg(conn, pix, gc, C_SEP, x, y+RULER_H-1, x+w, y+RULER_H-1); // ruler separator
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
fn get_keysym(keysyms: &[u32], kpc: u8, keycode: u8, min_kc: u8) -> u32 {
    keysyms.get((keycode.saturating_sub(min_kc)) as usize * kpc as usize).copied().unwrap_or(0)
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
                eprintln!("vcd-viewer [-d DISPLAY] [file.vcd]");
                eprintln!("Tab=cycle panels (Module→Signals→Wave)");
                eprintln!("MODULE:  j/k=nav  Enter=expand/collapse  A=add all signals");
                eprintln!("SIGNALS: j/k=nav  a/Enter=add/remove  A=add all in module");
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
            .event_mask(EventMask::EXPOSURE|EventMask::KEY_PRESS|EventMask::BUTTON_PRESS|EventMask::STRUCTURE_NOTIFY))?;
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
                let ks = get_keysym(&kbd.keysyms, kbd.keysyms_per_keycode, e.detail, mkc);
                if app.handle_keysym(ks, wh as i16) { break; }
                dirty=true;
            }
            Event::ButtonPress(e) => { app.handle_button(e.detail, e.event_x, ww as i16); dirty=true; }
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
