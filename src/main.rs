// Scanner: shows what takes the space on a drive as a sunburst chart. Plain Win32 and GDI+,
// so the exe needs nothing installed.
#![windows_subsystem = "windows"]

mod chart;
mod gfx;
mod lang;
mod panel;
mod scan;
mod settings;
mod theme;
mod util;

use std::{
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicPtr, Ordering::Relaxed},
    },
    time::Instant,
};
use windows_sys::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES},
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
        UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
    core::w,
};

use chart::Chart;
use gfx::{G, contains, rect};
use lang::{t, tf};
use scan::{Arena, NONE, Stats};
use settings::Settings;
use theme::{Mode, Theme};
use util::wide;

const WM_SCAN_DONE: u32 = WM_APP + 1;
const WM_START: u32 = WM_APP + 2;
const TIMER_BUSY: usize = 1;
const MAIN_W: f32 = 486.0; // the chart side never gets smaller than this
const MAIN_H: f32 = 407.0;
const SPLIT: f32 = 5.0; // the splitter between the chart and the folder panel

// tooltip ids besides the drives (0..)
const TIP_SUMMARY: usize = 100;
const TIP_SETTINGS: usize = 101;
const TIP_UP: usize = 102;
const TIP_BACK: usize = 103;
const TIP_RESCAN: usize = 104;
const TIP_NOTE: usize = 105;

// LPSTR_TEXTCALLBACKW: the tooltip asks for its text when it shows
const TEXT_CALLBACK: *mut u16 = -1isize as *mut u16;

static APP: AtomicPtr<App> = AtomicPtr::new(null_mut());

// The one window's state. Win32 calls back into the window procedure from inside other
// calls, so it is reached through this pointer rather than passed around.
pub fn app() -> &'static mut App {
    unsafe { &mut *APP.load(Relaxed) }
}

// what is under the mouse, apart from the chart
#[derive(Clone, Copy, PartialEq)]
enum Hot {
    None,
    Drive(usize),
    Summary,
    Settings,
    Up,
    Back,
    Rescan,
    Cancel,
    Split,
}

struct DriveItem {
    drive: scan::Drive,
    root: u32, // NONE until scanned
    rect: RECT,
}

// what to show when a scan has finished
enum After {
    Drive(Option<String>), // the drive, or this folder on it
    Summary,
    Folder(u32), // the folder that was scanned again
}

pub struct Job {
    id: usize,
    stats: Arc<Stats>,
    result: Arc<Mutex<Option<Vec<Arena>>>>,
    drives: Vec<usize>,
    after: After,
    started: Instant,
}

// the last scan, for the note at the bottom
struct Note {
    cancelled: bool,
    secs: f64,
    files: u64,
    errors: u64,
}

pub struct App {
    hwnd: HWND,
    pub tree: HWND,
    tip: HWND,
    dpi: u32,
    pub s: f32, // pixels per design pixel (DPI / 96)
    set: Settings,
    pub t: Theme,
    pub a: Arena,
    pub chart: Chart,
    garbage: usize, // nodes no longer reachable after rescans and deletions
    drives: Vec<DriveItem>,
    active: usize,
    summary_on: bool,
    pub focus: u32,
    history: Vec<u32>,
    pub job: Option<Job>,
    next_job: usize,
    phase: i32, // the moving bar while scanning
    note: Option<Note>,
    info: String,
    panel_w: f32,
    dragging: bool,
    tracking: bool,
    hot: Hot,
    pressed: Hot,
    main_w: i32,
    r_summary: RECT,
    r_settings: RECT,
    r_up: RECT,
    r_back: RECT,
    r_rescan: RECT,
    r_cancel: RECT,
    r_prog: RECT,
    r_status: RECT,
    r_note: RECT,
    pub ui: HFONT,
    head: HFONT,
    icon_big: HFONT,
    icon_mid: HFONT,
    pub icon_small: HFONT,
    icon_face: String,
    pub tree_hot: isize,
    pub filling: bool,
    tip_buf: Vec<u16>,
}

fn main() {
    // a crash is written to error.log next to the exe, as the C# version did
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::current_exe().unwrap_or_default().with_file_name("error.log")) {
            let _ = writeln!(f, "{info}");
        }
    }));
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES };
        InitCommonControlsEx(&icc);
    }
    gfx::startup();
    let mut set = Settings::load();
    lang::load(&set.lang);
    // the language file is gone or broken: English from now on
    if lang::code() != set.lang {
        set.lang = lang::code();
        set.save();
    }
    scan::BY_SIZE.store(set.by_size, Relaxed);

    unsafe {
        let inst = GetModuleHandleW(null());
        let class = w!("ScannerRs");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: inst,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            // icon 1 from res/app.rc; the small one is picked from the same file at its own size
            hIcon: LoadIconW(inst, 1 as _),
            hIconSm: LoadImageW(inst, 1 as _, IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), 0),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        let hwnd = CreateWindowExW(0, class, w!("Scanner"), style, CW_USEDEFAULT, CW_USEDEFAULT, 100, 100, null_mut(), null_mut(), inst, null());
        APP.store(Box::into_raw(Box::new(App::new(hwnd, set))), Relaxed);
        app().init();
        ShowWindow(hwnd, SW_SHOW);
        PostMessageW(hwnd, WM_START, 0, 0);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn lo(v: isize) -> i32 {
    (v & 0xffff) as i16 as i32
}

fn hi(v: isize) -> i32 {
    (v >> 16 & 0xffff) as i16 as i32
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if APP.load(Relaxed).is_null() {
        return unsafe { DefWindowProcW(h, msg, wp, lp) };
    }
    let app = app();
    match msg {
        WM_PAINT => app.paint(),
        WM_ERASEBKGND => return 1,
        WM_SIZE => app.layout(),
        WM_GETMINMAXINFO => app.min_size(lp as *mut MINMAXINFO),
        WM_MOUSEMOVE => app.mouse_move(lo(lp), hi(lp)),
        WM_MOUSELEAVE => app.mouse_leave(),
        WM_LBUTTONDOWN => app.mouse_down(),
        WM_LBUTTONUP => app.mouse_up(),
        WM_RBUTTONUP => app.right_click(),
        // the capture is gone (Alt+Tab, a dialog): the button-up will not arrive here
        WM_CAPTURECHANGED => app.dragging = false,
        WM_SETCURSOR if lo(lp) == HTCLIENT as i32 => {
            app.set_cursor();
            return 1;
        }
        WM_NOTIFY => {
            let nm = lp as *const NMHDR;
            if unsafe { (*nm).hwndFrom } == app.tree {
                if let Some(r) = panel::notify(app, nm) {
                    return r;
                }
            } else if unsafe { (*nm).code } == TTN_GETDISPINFOW {
                app.tip_text(nm as *mut NMTTDISPINFOW);
            }
        }
        WM_TIMER => app.tick(),
        WM_SCAN_DONE => app.scan_done(wp),
        WM_START => app.auto_start(),
        WM_SETTINGCHANGE => app.setting_change(lp),
        WM_DPICHANGED => app.dpi_changed(hi(wp as isize) as u32, lp as *const RECT),
        WM_DESTROY => {
            app.stop_job();
            unsafe { PostQuitMessage(0) };
        }
        _ => return unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
    if msg == WM_NOTIFY || msg == WM_SETTINGCHANGE { unsafe { DefWindowProcW(h, msg, wp, lp) } } else { 0 }
}

impl App {
    fn new(hwnd: HWND, set: Settings) -> App {
        let r = RECT::default();
        App {
            hwnd,
            tree: null_mut(),
            tip: null_mut(),
            dpi: 96,
            s: 1.0,
            t: theme::build(set.theme),
            set,
            a: Arena::default(),
            chart: Chart::new(),
            garbage: 0,
            drives: scan::drives().into_iter().map(|drive| DriveItem { drive, root: NONE, rect: r }).collect(),
            active: 0,
            summary_on: false,
            focus: NONE,
            history: Vec::new(),
            job: None,
            next_job: 0,
            phase: 0,
            note: None,
            info: String::new(),
            panel_w: 300.0,
            dragging: false,
            tracking: false,
            hot: Hot::None,
            pressed: Hot::None,
            main_w: 0,
            r_summary: r,
            r_settings: r,
            r_up: r,
            r_back: r,
            r_rescan: r,
            r_cancel: r,
            r_prog: r,
            r_status: r,
            r_note: r,
            ui: null_mut(),
            head: null_mut(),
            icon_big: null_mut(),
            icon_mid: null_mut(),
            icon_small: null_mut(),
            icon_face: String::new(),
            tree_hot: 0,
            filling: false,
            tip_buf: Vec::new(),
        }
    }

    fn init(&mut self) {
        // Segoe Fluent Icons on Windows 11, Segoe MDL2 Assets on Windows 10: both ship with Windows
        self.icon_face = if has_font("Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" }.into();
        self.chart.rings = self.set.rings;
        self.chart.show_free = self.set.free;
        self.tree = panel::create(self.hwnd);
        unsafe {
            self.tip = CreateWindowExW(WS_EX_TOPMOST, TOOLTIPS_CLASSW, null(), WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX, 0, 0, 0, 0, self.hwnd, null_mut(), null_mut(), null());
            SendMessageW(self.tip, TTM_SETMAXTIPWIDTH, 0, 600);
            for id in (0..self.drives.len()).chain(TIP_SUMMARY..=TIP_NOTE) {
                let ti = self.tool(id, RECT::default());
                SendMessageW(self.tip, TTM_ADDTOOLW, 0, &ti as *const _ as LPARAM);
            }
            self.dpi = GetDpiForWindow(self.hwnd);
        }
        self.s = self.dpi as f32 / 96.0;
        self.make_fonts();
        self.apply_theme();
        // the first size: the chart side, and the panel when it is on; centred on the screen
        unsafe {
            let (w, h) = self.window_size(self.min_client());
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            GetMonitorInfoW(MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST), &mut mi);
            let wa = mi.rcWork;
            SetWindowPos(self.hwnd, null_mut(), wa.left + (wa.right - wa.left - w) / 2, wa.top + (wa.bottom - wa.top - h) / 2, w, h, SWP_NOZORDER);
        }
        self.layout();
    }

    fn px(&self, v: f32) -> i32 {
        (v * self.s).round() as i32
    }

    fn make_fonts(&mut self) {
        for f in [self.ui, self.head, self.icon_big, self.icon_mid, self.icon_small] {
            if !f.is_null() {
                unsafe { DeleteObject(f) };
            }
        }
        // point sizes at this DPI: 9 pt text, 9.75 pt headings, 13, 11 and 7 pt icons
        let pt = |p: f32| (p * self.dpi as f32 / 72.0).round() as i32;
        self.ui = gfx::font("Segoe UI", pt(9.0), false);
        self.head = gfx::font("Segoe UI", pt(9.75), true);
        self.icon_big = gfx::font(&self.icon_face, pt(13.0), false);
        self.icon_mid = gfx::font(&self.icon_face, pt(11.0), false);
        self.icon_small = gfx::font(&self.icon_face, pt(7.0), false);
        unsafe {
            SendMessageW(self.tree, WM_SETFONT, self.ui as usize, 0);
            SendMessageW(self.tree, TVM_SETITEMHEIGHT, self.px(26.0) as usize, 0);
        }
    }

    fn tool(&self, id: usize, rect: RECT) -> TTTOOLINFOW {
        TTTOOLINFOW { cbSize: size_of::<TTTOOLINFOW>() as u32, uFlags: TTF_SUBCLASS, hwnd: self.hwnd, uId: id, rect, lpszText: TEXT_CALLBACK, ..Default::default() }
    }

    fn panel_space(&self) -> i32 {
        if self.set.panel { self.px(self.panel_w) + self.px(SPLIT) } else { 0 }
    }

    fn min_client(&self) -> (i32, i32) {
        (self.px(MAIN_W) + self.panel_space(), self.px(MAIN_H))
    }

    fn window_size(&self, (w, h): (i32, i32)) -> (i32, i32) {
        let mut r = rect(0, 0, w, h);
        unsafe { AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, 0, 0, self.dpi) };
        (r.right - r.left, r.bottom - r.top)
    }

    fn min_size(&self, mmi: *mut MINMAXINFO) {
        let (w, h) = self.window_size(self.min_client());
        unsafe { (*mmi).ptMinTrackSize = POINT { x: w, y: h } };
    }

    fn client(&self) -> RECT {
        let mut c = RECT::default();
        unsafe { GetClientRect(self.hwnd, &mut c) };
        c
    }

    // the places of everything, kept as they were in the original window
    fn layout(&mut self) {
        let c = self.client();
        let h = c.bottom;
        self.main_w = (c.right - self.panel_space()).max(0);
        let (mw, b) = (self.main_w, self.px(36.0));
        for i in 0..self.drives.len() {
            self.drives[i].rect = rect(self.px(4.0), self.px(8.0 + 30.0 * i as f32), self.px(60.0), self.px(28.0));
        }
        self.r_summary = rect(self.px(4.0), h - self.px(32.0), self.px(110.0), self.px(28.0));
        let bx = mw - self.px(42.0);
        self.r_settings = rect(bx, self.px(8.0), b, b);
        self.r_up = rect(bx, h - self.px(151.0), b, b);
        self.r_back = rect(bx, h - self.px(111.0), b, b);
        self.r_rescan = rect(bx, h - self.px(71.0), b, b);
        self.chart.rect = rect(self.px(68.0), self.px(6.0), mw - self.px(116.0), h - self.px(39.0));
        let cr = self.chart.rect;
        let (cx, cy) = ((cr.left + cr.right) / 2, (cr.top + cr.bottom) / 2);
        self.r_status = rect(cx - self.px(160.0), cy - self.px(24.0), self.px(320.0), self.px(20.0));
        self.r_prog = rect(cx - self.px(66.0), cy, self.px(132.0), self.px(6.0));
        self.r_cancel = rect(cx - self.px(50.0), cy + self.px(18.0), self.px(100.0), self.px(28.0));
        unsafe {
            let x = mw + self.px(SPLIT);
            MoveWindow(self.tree, x, self.px(34.0), c.right - x, h - self.px(34.0), 1);
            ShowWindow(self.tree, if self.set.panel { SW_SHOW } else { SW_HIDE });
            let tools = [(TIP_SUMMARY, self.r_summary), (TIP_SETTINGS, self.r_settings), (TIP_UP, self.r_up), (TIP_BACK, self.r_back), (TIP_RESCAN, self.r_rescan)];
            for (id, r) in self.drives.iter().map(|d| d.rect).enumerate().chain(tools) {
                let ti = self.tool(id, r);
                SendMessageW(self.tip, TTM_NEWTOOLRECTW, 0, &ti as *const _ as LPARAM);
            }
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    fn enabled(&self, h: Hot) -> bool {
        let idle = self.job.is_none();
        match h {
            Hot::Up => idle && self.focus != NONE && self.a.node(self.focus).parent != NONE,
            Hot::Back => idle && !self.history.is_empty(),
            Hot::Rescan => idle && self.focus != NONE && !self.summary_on,
            Hot::Cancel => !idle,
            _ => true,
        }
    }

    fn note_text(&self) -> String {
        match &self.note {
            None => String::new(),
            Some(n) if n.cancelled => t("scan.cancelled"),
            Some(n) => {
                let (secs, files) = (util::dec(n.secs, 1, false), util::num(n.files));
                if n.errors > 0 { tf("scan.done.errors", &[&secs, &files, &util::num(n.errors)]) } else { tf("scan.done", &[&secs, &files]) }
            }
        }
    }

    fn paint(&mut self) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            let c = self.client();
            // drawn off screen first, so nothing flickers
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, c.right, c.bottom);
            let old = SelectObject(mem, bmp);
            self.draw(mem, &c);
            BitBlt(hdc, 0, 0, c.right, c.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            EndPaint(self.hwnd, &ps);
        }
    }

    fn draw(&mut self, hdc: HDC, c: &RECT) {
        let th = self.t.clone();
        let busy = self.job.is_some();
        let s = self.s;
        gfx::fill_rect(hdc, c, th.back);
        let buttons = [(Hot::Settings, self.r_settings, "\u{E713}"), (Hot::Up, self.r_up, "\u{E74A}"), (Hot::Back, self.r_back, "\u{E72B}"), (Hot::Rescan, self.r_rescan, "\u{E72C}")];
        let mut items: Vec<(Hot, RECT, bool, String, String)> = self.drives.iter().enumerate()
            .map(|(i, d)| (Hot::Drive(i), d.rect, !self.summary_on && i == self.active, drive_glyph(d.drive.kind).into(), d.drive.root[..2].to_string()))
            .collect();
        items.push((Hot::Summary, self.r_summary, self.summary_on, "\u{EB05}".into(), t("summary")));

        // shapes first, anti-aliased
        {
            let g = G::new(hdc);
            for (h, r, selected, ..) in &items {
                if *selected || self.hot == *h {
                    g.round(r, 4.0 * s, if *selected { th.select } else { th.row_hover });
                }
            }
            for (h, r, _) in buttons {
                if self.enabled(h) && (self.hot == h || self.pressed == h) {
                    g.round(&r, 4.0 * s, if self.pressed == h { th.select } else { th.row_hover });
                }
            }
            if busy {
                // a bar that runs back and forth while the scan goes on
                let p = self.r_prog;
                let (w, chunk) = (p.right - p.left, self.px(40.0));
                let x = p.left + (self.phase * self.px(6.0)) % (w + chunk) - chunk;
                g.round(&p, 3.0 * s, th.row_hover);
                g.round(&RECT { left: x.max(p.left), right: (x + chunk).min(p.right), ..p }, 3.0 * s, th.accent);
                g.round(&self.r_cancel, 4.0 * s, if self.hot == Hot::Cancel { th.select } else { th.row_hover });
            } else {
                self.chart.paint_shapes(&g, &self.a, &th);
            }
        }

        // then the text, in ClearType
        for (_, r, selected, glyph, text) in &items {
            gfx::text(hdc, glyph, &RECT { left: r.left + self.px(6.0), right: r.left + self.px(26.0), ..*r }, if *selected { th.accent } else { th.muted }, self.icon_mid, gfx::LINE | DT_CENTER);
            gfx::text(hdc, text, &RECT { left: r.left + self.px(30.0), right: r.right - self.px(2.0), ..*r }, th.text, self.ui, gfx::LINE | DT_END_ELLIPSIS);
        }
        for (h, r, glyph) in buttons {
            gfx::text(hdc, glyph, &r, if self.enabled(h) { th.text } else { th.disabled }, self.icon_big, gfx::LINE | DT_CENTER);
        }
        if let Some(job) = &self.job {
            let st = &job.stats;
            let status = tf("scan.progress", &[&util::num(st.files.load(Relaxed)), &util::size(st.bytes.load(Relaxed))]);
            gfx::text(hdc, &status, &self.r_status, th.text, self.ui, gfx::LINE | DT_CENTER | DT_END_ELLIPSIS);
            gfx::text(hdc, &t("button.cancel"), &self.r_cancel, th.text, self.ui, gfx::LINE | DT_CENTER);
        } else {
            self.chart.paint_text(hdc, &self.a, &th, &self.icon_face);
        }

        // bottom line: the item (or folder) on the left, the last scan on the right
        let note = self.note_text();
        let (y, lh) = (c.bottom - self.px(26.0), self.px(22.0));
        let nw = if note.is_empty() { 0 } else { gfx::text_width(hdc, &note, self.ui) + self.px(4.0) };
        let r_note = rect(self.main_w - self.px(8.0) - nw, y, nw, lh);
        if !gfx::same(&r_note, &self.r_note) {
            self.r_note = r_note;
            let ti = self.tool(TIP_NOTE, r_note);
            unsafe { SendMessageW(self.tip, TTM_NEWTOOLRECTW, 0, &ti as *const _ as LPARAM) };
        }
        gfx::text(hdc, &note, &r_note, th.muted, self.ui, gfx::LINE);
        let info = rect(self.px(122.0), y, r_note.left - self.px(134.0), lh);
        gfx::text(hdc, &self.info, &info, th.text, self.ui, gfx::LINE | DT_PATH_ELLIPSIS);

        if self.set.panel {
            let x = self.main_w + self.px(SPLIT / 2.0);
            gfx::fill_rect(hdc, &rect(x, 0, 1, c.bottom), th.border);
            let head = if self.focus != NONE && !busy { format!("{}   ·   {}", self.a.label(self.focus), util::size(self.a.node(self.focus).size)) } else { String::new() };
            let r = rect(self.main_w + self.px(SPLIT + 10.0), 0, c.right - self.main_w - self.px(SPLIT + 20.0), self.px(34.0));
            gfx::text(hdc, &head, &r, th.text, self.head, gfx::LINE | DT_END_ELLIPSIS);
        }
    }

    fn invalidate(&self, r: Option<&RECT>) {
        unsafe { InvalidateRect(self.hwnd, r.map_or(null(), |r| r as *const RECT), 0) };
    }

    fn invalidate_bottom(&self) {
        let c = self.client();
        self.invalidate(Some(&rect(0, c.bottom - self.px(28.0), self.main_w, self.px(28.0))));
    }

    // ---------- mouse ----------

    fn hot_at(&self, x: i32, y: i32) -> Hot {
        if self.set.panel && x >= self.main_w && x < self.main_w + self.px(SPLIT) {
            return Hot::Split;
        }
        let mut places: Vec<(Hot, RECT)> = self.drives.iter().enumerate().map(|(i, d)| (Hot::Drive(i), d.rect)).collect();
        places.extend([(Hot::Summary, self.r_summary), (Hot::Settings, self.r_settings), (Hot::Up, self.r_up), (Hot::Back, self.r_back), (Hot::Rescan, self.r_rescan), (Hot::Cancel, self.r_cancel)]);
        places.into_iter().find(|(h, r)| self.enabled(*h) && contains(r, x, y)).map_or(Hot::None, |(h, _)| h)
    }

    fn mouse_move(&mut self, x: i32, y: i32) {
        if !self.tracking {
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
            unsafe { TrackMouseEvent(&mut tme) };
            self.tracking = true;
        }
        if self.dragging {
            // the panel takes what the mouse leaves, but never the chart side's minimum
            let w = self.client().right as f32 / self.s;
            self.panel_w = (w - x as f32 / self.s - SPLIT / 2.0).clamp(160.0, (w - MAIN_W - SPLIT).max(160.0));
            self.layout();
            return;
        }
        let hot = self.hot_at(x, y);
        if hot != self.hot {
            self.hot = hot;
            self.invalidate(None);
        }
        if self.job.is_none() && self.chart.mouse_move(&self.a, x, y) {
            self.invalidate(Some(&self.chart.rect));
            self.chart_hover();
        }
    }

    fn mouse_leave(&mut self) {
        self.tracking = false;
        self.hot = Hot::None;
        if self.chart.leave() {
            self.chart_hover();
        }
        self.invalidate(None);
    }

    fn mouse_down(&mut self) {
        if self.hot == Hot::Split {
            self.dragging = true;
            unsafe { SetCapture(self.hwnd) };
            return;
        }
        self.pressed = self.hot;
        self.invalidate(None);
        if self.hot != Hot::None || self.job.is_some() {
            return;
        }
        // a folder on the chart opens; the centre goes a level up
        match self.chart.hovered() {
            Some(seg) if seg.node != NONE => self.zoom(seg.node),
            None if self.chart.center_hot() => self.up(),
            _ => {}
        }
    }

    fn mouse_up(&mut self) {
        if self.dragging {
            self.dragging = false;
            unsafe { ReleaseCapture() };
            return;
        }
        let pressed = std::mem::replace(&mut self.pressed, Hot::None);
        self.invalidate(None);
        if pressed == Hot::None || pressed != self.hot {
            return;
        }
        match pressed {
            Hot::Drive(i) => self.select_drive(i, i == self.active && !self.summary_on, None),
            Hot::Summary => self.toggle_summary(),
            Hot::Settings => self.settings_menu(),
            Hot::Up => self.up(),
            Hot::Back => self.back(),
            Hot::Rescan => self.rescan(self.focus),
            Hot::Cancel => self.cancel_scan(),
            _ => {}
        }
    }

    fn right_click(&mut self) {
        if let Some(n) = self.chart.hovered().map(|s| s.node).filter(|&n| n != NONE && self.job.is_none()) {
            self.context_menu(n);
        }
    }

    fn set_cursor(&self) {
        let clickable = self.job.is_none()
            && match self.chart.hovered() {
                Some(seg) => seg.node != NONE && self.a.node(seg.node).dir && !self.a.node(seg.node).kids.is_empty(),
                None => self.chart.center_hot() && self.enabled(Hot::Up),
            };
        let cursor = match self.hot {
            Hot::Split => IDC_SIZEWE,
            Hot::None if !clickable => IDC_ARROW,
            _ => IDC_HAND,
        };
        unsafe { SetCursor(LoadCursorW(null_mut(), cursor)) };
    }

    // ---------- what is shown ----------

    fn set_focus(&mut self, n: u32, push: bool) {
        if push && self.focus != NONE {
            self.history.push(self.focus);
        }
        self.focus = n;
        self.chart.show(&self.a, n);
        panel::show(self);
        self.info = self.header();
        self.invalidate(None);
    }

    // the bottom line without an item: the folder and the free space of its drive
    fn header(&self) -> String {
        let f = self.a.node(self.focus);
        if f.summary {
            let names: Vec<String> = f.kids.iter().map(|&k| self.a.path(k).trim_end_matches('\\').to_string()).collect();
            return tf("summary.of", &[&names.join(", ")]);
        }
        let mut root = self.focus;
        while self.a.node(root).parent != NONE {
            root = self.a.node(root).parent;
        }
        match self.a.space.get(&root) {
            Some(&(free, _)) => tf("info.free", &[&self.a.path(self.focus), &util::size(free), self.a.path(root).trim_end_matches('\\')]),
            None => self.a.path(self.focus),
        }
    }

    // the bottom line names an item, or the current folder for NONE
    fn show_info(&mut self, n: u32) {
        if self.focus == NONE || self.job.is_some() {
            return;
        }
        self.info = if n == NONE { self.header() } else { self.a.path(n) };
        self.invalidate_bottom();
    }

    fn chart_hover(&mut self) {
        match self.chart.hovered().map(|s| (s.node, s.label.clone(), s.size)) {
            Some((NONE, label, size)) => {
                self.info = format!("{label}   ·   {}", util::size(size));
                self.invalidate_bottom();
            }
            hover => self.show_info(hover.map_or(NONE, |h| h.0)),
        }
    }

    // a row picked in the folder panel lights up on the chart
    pub fn tree_pick(&mut self, n: u32) {
        self.chart.highlight = n;
        self.show_info(n);
        self.invalidate(Some(&self.chart.rect));
    }

    // the row under the mouse, or the picked one when the mouse leaves
    pub fn tree_hover(&mut self, n: u32) {
        let picked = panel::node_of(self.tree, panel::selected(self.tree));
        self.tree_pick(if n != NONE { n } else { picked });
    }

    pub fn zoom(&mut self, n: u32) {
        if self.job.is_none() && n != NONE && self.a.node(n).dir && !self.a.node(n).kids.is_empty() {
            self.set_focus(n, true);
        }
    }

    pub fn up(&mut self) {
        if self.enabled(Hot::Up) {
            self.set_focus(self.a.node(self.focus).parent, true);
        }
    }

    fn back(&mut self) {
        if let Some(n) = self.history.pop().filter(|_| self.job.is_none()) {
            self.set_focus(n, false);
        }
    }

    fn select_drive(&mut self, i: usize, force: bool, goto: Option<String>) {
        // a drive picked while scanning (for Summary, say) stops that scan
        self.stop_job();
        (self.summary_on, self.active) = (false, i);
        self.history.clear();
        let root = self.drives[i].root;
        if !force && root != NONE {
            self.set_focus(root, false);
        } else {
            self.start_job(vec![i], After::Drive(goto));
        }
    }

    // Summary shows every drive in one chart, scanning first the drives not in memory yet;
    // clicking it again goes back to the drive
    fn toggle_summary(&mut self) {
        if self.job.is_some() {
            return;
        }
        self.history.clear();
        self.summary_on = !self.summary_on;
        if !self.summary_on {
            if self.drives[self.active].root != NONE {
                self.set_focus(self.drives[self.active].root, false);
            }
            return;
        }
        let missing: Vec<usize> = (0..self.drives.len()).filter(|&i| self.drives[i].root == NONE).collect();
        if missing.is_empty() { self.show_summary() } else { self.start_job(missing, After::Summary) }
    }

    fn show_summary(&mut self) {
        self.history.clear();
        let kids: Vec<u32> = self.drives.iter().map(|d| d.root).filter(|&r| r != NONE).collect();
        let root = self.a.summary(&t("summary"), kids);
        self.garbage += 1;
        self.set_focus(root, false);
    }

    fn find(&self, root: u32, path: &str) -> Option<u32> {
        let want = path.trim_end_matches('\\').to_lowercase();
        let mut n = root;
        'down: loop {
            let p = self.a.path(n).trim_end_matches('\\').to_lowercase();
            if p == want {
                return Some(n);
            }
            for &k in &self.a.node(n).kids {
                let kp = self.a.path(k).to_lowercase();
                if self.a.node(k).dir && (want == kp || want.starts_with(&(kp + "\\"))) {
                    n = k;
                    continue 'down;
                }
            }
            return None;
        }
    }

    // ---------- scanning ----------

    fn start_job(&mut self, drives: Vec<usize>, after: After) {
        self.stop_job();
        let paths: Vec<(String, String)> = match after {
            After::Folder(n) => vec![(self.a.path(n), self.a.name(n))],
            _ => drives.iter().map(|&i| (self.drives[i].drive.root.clone(), self.drives[i].drive.root.clone())).collect(),
        };
        let (id, stats, result) = (self.next_job, Arc::new(Stats::default()), Arc::new(Mutex::new(None)));
        self.next_job += 1;
        let (st, res, hwnd) = (stats.clone(), result.clone(), self.hwnd as usize);
        // a big stack: the scan goes down the folders recursively
        let _ = std::thread::Builder::new().stack_size(64 << 20).spawn(move || {
            let arenas: Vec<Arena> = paths.iter().map(|(p, n)| scan::scan(p, n, &st)).collect();
            *res.lock().unwrap() = Some(arenas);
            unsafe { PostMessageW(hwnd as HWND, WM_SCAN_DONE, id, 0) };
        });
        self.job = Some(Job { id, stats, result, drives, after, started: Instant::now() });
        unsafe { SetTimer(self.hwnd, TIMER_BUSY, 150, None) };
        self.info.clear();
        panel::show(self);
        self.invalidate(None);
    }

    pub fn stop_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.stats.cancel.store(true, Relaxed);
            unsafe { KillTimer(self.hwnd, TIMER_BUSY) };
            self.invalidate(None);
        }
    }

    fn tick(&mut self) {
        if let Some(job) = &self.job {
            self.info = job.stats.current.lock().unwrap().clone();
            self.phase += 1;
            self.invalidate(None);
        }
    }

    fn scan_done(&mut self, id: usize) {
        let Some(job) = self.job.take_if(|j| j.id == id) else { return };
        unsafe { KillTimer(self.hwnd, TIMER_BUSY) };
        let arenas = job.result.lock().unwrap().take().unwrap_or_default();
        let st = &job.stats;
        self.note = Some(Note { cancelled: false, secs: job.started.elapsed().as_secs_f64(), files: st.files.load(Relaxed), errors: st.errors.load(Relaxed) });
        match job.after {
            After::Folder(old) => {
                let new = self.a.merge(arenas.into_iter().next().unwrap());
                let parent = self.a.node(old).parent;
                self.a.node_mut(new).parent = parent;
                if let Some(pos) = self.a.node(parent).kids.iter().position(|&k| k == old) {
                    self.a.node_mut(parent).kids[pos] = new;
                }
                let (n, o) = (self.a.node(new), self.a.node(old));
                let (d_size, d_files) = (n.size as i64 - o.size as i64, n.files as i64 - o.files as i64);
                self.a.propagate(parent, d_size, d_files);
                self.garbage += count(&self.a, old);
                self.history = replaced_in_history(&self.a, &self.history, old, new);
                // the view the rescan was started from stays reachable with Back
                let from = self.focus != old;
                self.set_focus(new, from);
            }
            after => {
                for (arena, &d) in arenas.into_iter().zip(&job.drives) {
                    if self.drives[d].root != NONE {
                        self.garbage += count(&self.a, self.drives[d].root);
                    }
                    let root = self.a.merge(arena);
                    if let Some(space) = scan::space(&self.drives[d].drive.root) {
                        self.a.space.insert(root, space);
                    }
                    self.drives[d].root = root;
                }
                match after {
                    After::Drive(goto) => {
                        let root = self.drives[job.drives[0]].root;
                        let target = goto.and_then(|g| self.find(root, &g)).unwrap_or(root);
                        self.set_focus(target, false);
                    }
                    _ => self.show_summary(),
                }
            }
        }
        self.collect();
    }

    // stops the running scan and goes back to what was shown before it
    fn cancel_scan(&mut self) {
        self.stop_job();
        self.note = Some(Note { cancelled: true, secs: 0.0, files: 0, errors: 0 });
        if self.focus == NONE {
            self.info.clear();
            panel::show(self);
            return;
        }
        let mut top = self.focus;
        while self.a.node(top).parent != NONE {
            top = self.a.node(top).parent;
        }
        self.summary_on = self.a.node(top).summary;
        if let Some(i) = self.drives.iter().position(|d| d.root == top) {
            self.active = i;
        }
        self.set_focus(self.focus, false);
    }

    fn rescan(&mut self, n: u32) {
        if n == NONE || self.job.is_some() || self.summary_on {
            return;
        }
        if self.a.node(n).parent == NONE {
            self.start_job(vec![self.active], After::Drive(None));
        } else {
            self.start_job(Vec::new(), After::Folder(n));
        }
    }

    // When rescans and deletions have left more dead nodes than live ones, the tree is
    // copied without them, and the places that point into it follow.
    fn collect(&mut self) {
        if self.garbage * 2 <= self.a.nodes.len() {
            return;
        }
        let roots: Vec<u32> = self.drives.iter().map(|d| d.root).filter(|&r| r != NONE).collect();
        let map = self.a.compact(&roots);
        let moved = |n: u32| if n == NONE { NONE } else { map[n as usize] };
        for d in &mut self.drives {
            d.root = moved(d.root);
        }
        self.history = self.history.iter().map(|&n| moved(n)).filter(|&n| n != NONE).collect();
        self.garbage = 0;
        if self.summary_on {
            self.show_summary();
        } else {
            let focus = moved(self.focus);
            self.focus = NONE;
            if focus != NONE {
                self.set_focus(focus, false);
            }
        }
    }

    fn auto_start(&mut self) {
        if self.drives.is_empty() {
            return;
        }
        // a folder given on the command line opens on its drive
        let goto = std::env::args_os().nth(1).map(|a| a.to_string_lossy().into_owned());
        let roots: Vec<&str> = self.drives.iter().map(|d| d.drive.root.as_str()).collect();
        let i = goto.as_deref().and_then(|g| drive_of(&roots, g)).unwrap_or(0);
        self.select_drive(i, true, goto);
    }

    // ---------- menus and actions ----------

    fn track(&self, menu: HMENU, x: i32, y: i32, flags: TRACK_POPUP_MENU_FLAGS) -> u32 {
        unsafe {
            let id = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | flags, x, y, 0, self.hwnd, null());
            DestroyMenu(menu);
            id as u32
        }
    }

    pub fn context_menu(&mut self, n: u32) {
        let node = self.a.node(n);
        let menu = unsafe { CreatePopupMenu() };
        add(menu, 1, &t("menu.zoom"), node.dir && !node.kids.is_empty(), false);
        add(menu, 2, &t("menu.rescan"), node.dir && self.job.is_none() && !self.summary_on, false);
        add(menu, 3, &t("menu.open"), true, false);
        add(menu, 4, &t("menu.recycle"), node.parent != NONE, false);
        let mut pt = POINT::default();
        unsafe { GetCursorPos(&mut pt) };
        match self.track(menu, pt.x, pt.y, 0) {
            1 => self.zoom(n),
            2 => self.rescan(n),
            3 => unsafe {
                ShellExecuteW(self.hwnd, w!("open"), wide(&self.a.path(n)).as_ptr(), null(), null(), SW_SHOWNORMAL);
            },
            4 => self.recycle(n),
            _ => {}
        }
    }

    fn settings_menu(&mut self) {
        let langs = lang::available();
        unsafe {
            let menu = CreatePopupMenu();
            let sub = |key: &str, items: &[(u32, String, bool)]| {
                let m = CreatePopupMenu();
                for (id, text, checked) in items {
                    add(m, *id, text, true, *checked);
                }
                AppendMenuW(menu, MF_POPUP, m as usize, wide(&t(key)).as_ptr());
            };
            let themes = [(Mode::System, "theme.system"), (Mode::Light, "theme.light"), (Mode::Dark, "theme.dark")];
            sub("settings.theme", &themes.iter().enumerate().map(|(i, (m, k))| (10 + i as u32, t(k), self.set.theme == *m)).collect::<Vec<_>>());
            sub("settings.depth", &[(20, t("depth.min"), self.set.rings != 0), (21, t("depth.all"), self.set.rings == 0)]);
            sub("settings.order", &[(30, t("order.name"), !self.set.by_size), (31, t("order.size"), self.set.by_size)]);
            sub("settings.language", &langs.iter().enumerate().map(|(i, (code, name))| (40 + i as u32, name.clone(), *code == self.set.lang)).collect::<Vec<_>>());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            add(menu, 90, &t("settings.free"), true, self.set.free);
            add(menu, 91, &t("settings.panel"), true, self.set.panel);
            let mut pt = POINT { x: self.r_settings.right, y: self.r_settings.bottom };
            ClientToScreen(self.hwnd, &mut pt);
            match self.track(menu, pt.x, pt.y, TPM_RIGHTALIGN | TPM_TOPALIGN) {
                id @ 10..=12 => {
                    self.set.theme = themes[(id - 10) as usize].0;
                    self.apply_theme();
                }
                id @ 20..=21 => {
                    self.set.rings = if id == 20 { chart::MIN_RINGS } else { 0 };
                    self.chart.rings = self.set.rings;
                    self.refresh();
                }
                id @ 30..=31 => {
                    self.set.by_size = id == 31;
                    scan::BY_SIZE.store(self.set.by_size, Relaxed);
                    self.refresh();
                }
                id @ 40..=89 => {
                    // a file deleted or broken since the list was read gives English
                    lang::load(&langs[(id - 40) as usize].0);
                    self.set.lang = lang::code();
                    self.refresh();
                }
                90 => {
                    self.set.free = !self.set.free;
                    self.chart.show_free = self.set.free;
                    self.refresh();
                }
                91 => self.toggle_panel(),
                _ => return,
            }
        }
        self.set.save();
    }

    // shows the current view again after a setting changed what it looks like
    fn refresh(&mut self) {
        if self.job.is_some() || self.focus == NONE {
            self.invalidate(None);
        } else if self.a.node(self.focus).summary {
            self.show_summary();
        } else {
            self.set_focus(self.focus, false);
        }
    }

    // the window grows by the panel's width, so the chart keeps its size
    fn toggle_panel(&mut self) {
        let dw = self.px(self.panel_w) + self.px(SPLIT);
        self.set.panel = !self.set.panel;
        unsafe {
            if IsZoomed(self.hwnd) == 0 {
                let mut r = RECT::default();
                GetWindowRect(self.hwnd, &mut r);
                let w = r.right - r.left + if self.set.panel { dw } else { -dw };
                SetWindowPos(self.hwnd, null_mut(), 0, 0, w, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER);
            }
        }
        self.layout();
        panel::show(self);
    }

    fn recycle(&mut self, n: u32) {
        let path = self.a.path(n);
        unsafe {
            let ask = wide(&tf("recycle.ask", &[&path]));
            if MessageBoxW(self.hwnd, ask.as_ptr(), w!("Scanner"), MB_YESNO | MB_ICONQUESTION) != IDYES {
                return;
            }
            let from: Vec<u16> = path.encode_utf16().chain([0, 0]).collect();
            let mut op = SHFILEOPSTRUCTW {
                hwnd: self.hwnd,
                wFunc: FO_DELETE,
                pFrom: from.as_ptr(),
                fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING) as u16,
                ..Default::default()
            };
            // failed or cancelled: the shell has said why. The file is also looked for with the
            // \\?\ prefix, since without it a path over 260 characters is never found.
            if SHFileOperationW(&mut op) != 0 || op.fAnyOperationsAborted != 0 {
                return;
            }
            if GetFileAttributesW(wide(&format!("\\\\?\\{path}")).as_ptr()) != INVALID_FILE_ATTRIBUTES {
                return;
            }
        }
        let parent = self.a.node(n).parent;
        self.history.retain(|&h| !self.a.under(h, n));
        self.a.node_mut(parent).kids.retain(|&k| k != n);
        let (size, files) = (self.a.node(n).size as i64, self.a.node(n).files as i64);
        self.a.propagate(parent, -size, -files);
        self.garbage += count(&self.a, n);
        self.refresh();
        self.collect();
    }

    // ---------- theme, language, DPI ----------

    fn apply_theme(&mut self) {
        self.t = theme::build(self.set.theme);
        let dark = self.t.dark;
        unsafe {
            let on: i32 = dark as i32;
            DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, (&on as *const i32).cast(), 4);
            dark_menus(dark);
            let scheme = if dark { w!("DarkMode_Explorer") } else { w!("Explorer") };
            SetWindowTheme(self.tree, scheme, null());
            SetWindowTheme(self.tip, if dark { scheme } else { null() }, null());
            SendMessageW(self.tree, TVM_SETBKCOLOR, 0, gfx::cref(self.t.back) as LPARAM);
            // the title bar takes the new mode when the frame is drawn again
            SetWindowPos(self.hwnd, null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
            InvalidateRect(self.tree, null(), 1);
        }
        self.invalidate(None);
    }

    // Windows says the app mode or the accent changed: the System theme follows
    fn setting_change(&mut self, lp: LPARAM) {
        if lp == 0 || self.set.theme != Mode::System {
            return;
        }
        let name = unsafe { std::slice::from_raw_parts(lp as *const u16, (0..).take_while(|&i| *(lp as *const u16).add(i) != 0).count()) };
        if String::from_utf16_lossy(name) == "ImmersiveColorSet" && theme::build(Mode::System) != self.t {
            self.apply_theme();
        }
    }

    fn dpi_changed(&mut self, dpi: u32, r: *const RECT) {
        self.dpi = dpi;
        self.s = dpi as f32 / 96.0;
        self.make_fonts();
        unsafe {
            let r = &*r;
            SetWindowPos(self.hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.layout();
    }

    fn tip_text(&mut self, di: *mut NMTTDISPINFOW) {
        let id = unsafe { (*di).hdr.idFrom };
        let text = match id {
            TIP_SUMMARY => t("summary.tip"),
            TIP_SETTINGS => t("button.settings"),
            TIP_UP => t("button.up"),
            TIP_BACK => t("button.back"),
            TIP_RESCAN => t("button.rescan"),
            TIP_NOTE => match &self.note {
                Some(n) if !n.cancelled => {
                    let secs = util::dec(n.secs, 1, false);
                    if n.errors > 0 { tf("scan.time.errors", &[&secs, &util::num(n.errors)]) } else { tf("scan.time", &[&secs]) }
                }
                _ => String::new(),
            },
            i if i < self.drives.len() => {
                let root = &self.drives[i].drive.root;
                let name = format!("{} {}", root.trim_end_matches('\\'), scan::volume_label(root)).trim_end().to_string();
                match scan::space(root) {
                    Some((free, total)) => tf("drive.tip", &[&name, &util::size(free), &util::size(total)]),
                    None => name,
                }
            }
            _ => String::new(),
        };
        self.tip_buf = wide(&text);
        unsafe { (*di).lpszText = self.tip_buf.as_mut_ptr() };
    }
}

// a menu line; `checked` puts a tick in front of it
fn add(menu: HMENU, id: u32, text: &str, enabled: bool, checked: bool) {
    let flags = MF_STRING | if enabled { 0 } else { MF_GRAYED } | if checked { MF_CHECKED } else { 0 };
    unsafe { AppendMenuW(menu, flags, id as usize, wide(text).as_ptr()) };
}

// the drive a path starts on ("c:\x" is on "C:\"), by its first character: the path may
// start with anything, so it is never cut at a byte
fn drive_of(roots: &[&str], path: &str) -> Option<usize> {
    let mut c = path.chars();
    let (letter, colon) = (c.next()?, c.next()?);
    if colon != ':' {
        return None;
    }
    roots.iter().position(|r| r.starts_with(letter.to_ascii_uppercase()))
}

#[cfg(test)]
mod tests {
    use super::{count, drive_of};
    use crate::scan::{Arena, NONE};

    #[test]
    fn history_after_rescan() {
        let mut a = Arena::default();
        let root = a.add(&[b'C' as u16], NONE, true);
        let old = a.add(&[b'a' as u16], root, true);
        let inside = a.add(&[b'b' as u16], old, true);
        let other = a.add(&[b'c' as u16], root, true);
        let new = a.add(&[b'a' as u16], NONE, true);
        let h = super::replaced_in_history(&a, &[root, old, inside, other], old, new);
        assert_eq!(h, vec![root, new, other]);
    }

    #[test]
    fn count_deep_tree() {
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                let mut a = Arena::default();
                let mut n = a.add(&[b'C' as u16], NONE, true);
                let root = n;
                for _ in 0..16_000 {
                    n = a.add(&[b'a' as u16], n, true);
                }
                assert_eq!(count(&a, root), 16_001);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn drive_of_path() {
        let roots = ["C:\\", "D:\\"];
        assert_eq!(drive_of(&roots, "d:\\Users"), Some(1));
        assert_eq!(drive_of(&roots, "C:"), Some(0));
        assert_eq!(drive_of(&roots, "E:\\"), None);
        assert_eq!(drive_of(&roots, "\\\\server\\share"), None);
        assert_eq!(drive_of(&roots, ""), None);
        assert_eq!(drive_of(&roots, "a"), None);
        // the old byte slice [..2] panicked on these
        assert_eq!(drive_of(&roots, "aé\\x"), None);
        assert_eq!(drive_of(&roots, "é:\\"), None);
    }
}

// The views Back goes to after a folder was scanned again: the old folder is the new one
// now, and what was inside it is gone.
fn replaced_in_history(a: &Arena, history: &[u32], old: u32, new: u32) -> Vec<u32> {
    history.iter().filter_map(|&h| if h == old { Some(new) } else { (!a.under(h, old)).then_some(h) }).collect()
}

// how many nodes a subtree holds
fn count(a: &Arena, n: u32) -> usize {
    let (mut todo, mut total) = (vec![n], 0);
    while let Some(i) = todo.pop() {
        total += 1;
        todo.extend_from_slice(&a.node(i).kids);
    }
    total
}

fn drive_glyph(kind: u32) -> &'static str {
    match kind {
        2 => "\u{E88E}", // removable
        4 => "\u{E8CE}", // network
        5 => "\u{E958}", // CD
        _ => "\u{EDA2}",
    }
}

fn has_font(name: &str) -> bool {
    unsafe {
        let f = gfx::font(name, 12, false);
        let dc = GetDC(null_mut());
        SelectObject(dc, f);
        let mut buf = [0u16; 64];
        let n = GetTextFaceW(dc, buf.len() as i32, buf.as_mut_ptr());
        ReleaseDC(null_mut(), dc);
        DeleteObject(f);
        String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim_end_matches('\0') == name
    }
}

// Popup menus have no theme of their own: uxtheme's unnamed SetPreferredAppMode (ordinal 135)
// and FlushMenuThemes (136) switch them to dark, as Explorer does.
fn dark_menus(dark: bool) {
    unsafe {
        let lib = LoadLibraryW(w!("uxtheme.dll"));
        if let Some(f) = GetProcAddress(lib, 135usize as *const u8) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(if dark { 2 } else { 3 });
        }
        if let Some(f) = GetProcAddress(lib, 136usize as *const u8) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}
