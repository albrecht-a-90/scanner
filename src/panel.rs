// The folder panel: a native tree view with every row drawn here. It lists the items of the
// current view in the chart's order, with their sizes, opened further down like a tree;
// the swatch is the colour the item has on the chart.
use std::{collections::HashSet, ptr::null};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{Controls::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

use crate::{
    App, app,
    gfx::{self, G},
    lang::tf,
    scan::{NONE, ordered},
    util,
};

// A folder like C:\Windows\WinSxS holds tens of thousands of items, and a tree view that
// gets them all stops the window for a long time: only the largest are listed, in the
// chart's order, and one last row sums up the rest.
const MAX_ROWS: usize = 300;

// what a row stands for, kept in its lParam: a node, or the closing row of a long folder
enum Row {
    Node(u32),
    More(u32),
}

fn decode(p: LPARAM) -> Row {
    if p >= 0 { Row::Node(p as u32) } else { Row::More((-p - 2) as u32) }
}

pub fn create(parent: HWND) -> HWND {
    unsafe {
        let style = WS_CHILD | WS_TABSTOP | TVS_SHOWSELALWAYS | TVS_FULLROWSELECT | TVS_NOHSCROLL | TVS_DISABLEDRAGDROP;
        let h = CreateWindowExW(0, WC_TREEVIEWW, null(), style, 0, 0, 0, 0, parent, std::ptr::null_mut(), std::ptr::null_mut(), null());
        SendMessageW(h, TVM_SETEXTENDEDSTYLE, TVS_EX_DOUBLEBUFFER as usize, TVS_EX_DOUBLEBUFFER as isize);
        SetWindowSubclass(h, Some(hot_proc), 1, 0);
        h
    }
}

// lists the items of the current view again
pub fn show(app: &mut App) {
    app.filling = true;
    app.tree_hot = 0;
    unsafe {
        SendMessageW(app.tree, WM_SETREDRAW, 0, 0);
        SendMessageW(app.tree, TVM_DELETEITEM, 0, TVI_ROOT);
    }
    if app.focus != NONE && app.job.is_none() {
        fill(app, TVI_ROOT, app.focus);
    }
    unsafe {
        SendMessageW(app.tree, WM_SETREDRAW, 1, 0);
        InvalidateRect(app.tree, null(), 1);
    }
    app.filling = false;
}

fn fill(app: &App, parent: HTREEITEM, n: u32) {
    let a = &app.a;
    let kids = ordered(a, n);
    let keep: Option<HashSet<u32>> = (kids.len() > MAX_ROWS).then(|| largest(app, n).into_iter().collect());
    for &k in kids.iter().filter(|k| keep.as_ref().is_none_or(|s| s.contains(k))) {
        insert(app.tree, parent, k as isize, a.node(k).dir && !a.node(k).kids.is_empty());
    }
    if keep.is_some() {
        insert(app.tree, parent, -(n as isize) - 2, false);
    }
}

fn largest(app: &App, n: u32) -> Vec<u32> {
    let mut by_size = app.a.node(n).kids.clone();
    by_size.sort_by(|&x, &y| app.a.node(y).size.cmp(&app.a.node(x).size));
    by_size.truncate(MAX_ROWS);
    by_size
}

fn insert(tree: HWND, parent: HTREEITEM, param: LPARAM, children: bool) {
    let mut ins = TVINSERTSTRUCTW { hParent: parent, hInsertAfter: TVI_LAST, ..Default::default() };
    // an expandable folder gets its rows when it is first opened
    ins.Anonymous.item = TVITEMW { mask: TVIF_PARAM | TVIF_CHILDREN, cChildren: children as i32, lParam: param, ..Default::default() };
    unsafe { SendMessageW(tree, TVM_INSERTITEMW, 0, &ins as *const _ as LPARAM) };
}

fn param(tree: HWND, item: HTREEITEM) -> LPARAM {
    let mut it = TVITEMW { mask: TVIF_PARAM, hItem: item, ..Default::default() };
    unsafe { SendMessageW(tree, TVM_GETITEMW, 0, &mut it as *mut _ as LPARAM) };
    it.lParam
}

pub fn node_of(tree: HWND, item: HTREEITEM) -> u32 {
    match (item != 0).then(|| decode(param(tree, item))) {
        Some(Row::Node(n)) => n,
        _ => NONE,
    }
}

pub fn selected(tree: HWND) -> HTREEITEM {
    unsafe { SendMessageW(tree, TVM_GETNEXTITEM, TVGN_CARET as usize, 0) }
}

fn level(tree: HWND, item: HTREEITEM) -> i32 {
    let (mut l, mut p) = (0, item);
    loop {
        p = unsafe { SendMessageW(tree, TVM_GETNEXTITEM, TVGN_PARENT as usize, p) };
        if p == 0 {
            return l;
        }
        l += 1;
    }
}

// the item under a point of the tree view, 0 for none
fn hit(tree: HWND, x: i32, y: i32) -> HTREEITEM {
    let mut info = TVHITTESTINFO { pt: POINT { x, y }, ..Default::default() };
    unsafe { SendMessageW(tree, TVM_HITTEST, 0, &mut info as *mut _ as LPARAM) }
}

fn cursor(tree: HWND) -> (i32, i32) {
    let pos = unsafe { GetMessagePos() };
    let mut pt = POINT { x: (pos & 0xffff) as i16 as i32, y: (pos >> 16) as i16 as i32 };
    unsafe { ScreenToClient(tree, &mut pt) };
    (pt.x, pt.y)
}

fn row_rect(tree: HWND, item: HTREEITEM) -> RECT {
    // TVM_GETITEMRECT takes the item in the first field of the rectangle
    let mut r = RECT::default();
    unsafe {
        *(&mut r as *mut RECT as *mut HTREEITEM) = item;
        SendMessageW(tree, TVM_GETITEMRECT, 0, &mut r as *mut _ as LPARAM);
    }
    r
}

fn glyph(app: &App, level: i32, row: &RECT) -> RECT {
    let s = app.s;
    gfx::rect(((4 + level * 16) as f32 * s) as i32, row.top, (16.0 * s) as i32, row.bottom - row.top)
}

// handles the tree view's notifications; None leaves the default answer
pub fn notify(app: &mut App, nm: *const NMHDR) -> Option<LRESULT> {
    let tree = app.tree;
    unsafe {
        match (*nm).code {
            NM_CUSTOMDRAW => {
                let cd = &*(nm as *const NMTVCUSTOMDRAW);
                return Some(match cd.nmcd.dwDrawStage {
                    CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as LRESULT,
                    CDDS_ITEMPREPAINT => {
                        draw(app, cd);
                        CDRF_SKIPDEFAULT as LRESULT
                    }
                    _ => CDRF_DODEFAULT as LRESULT,
                });
            }
            TVN_ITEMEXPANDINGW => {
                let tv = &*(nm as *const NMTREEVIEWW);
                let first = SendMessageW(tree, TVM_GETNEXTITEM, TVGN_CHILD as usize, tv.itemNew.hItem);
                if tv.action == TVE_EXPAND && first == 0 {
                    if let Row::Node(n) = decode(tv.itemNew.lParam) {
                        fill(app, tv.itemNew.hItem, n);
                    }
                }
            }
            TVN_SELCHANGEDW if !app.filling => {
                let tv = &*(nm as *const NMTREEVIEWW);
                app.tree_pick(node_of(tree, tv.itemNew.hItem));
            }
            NM_CLICK => {
                // the chevron opens and closes the folder
                let (x, y) = cursor(tree);
                let item = hit(tree, x, y);
                if item != 0 && x < glyph(app, level(tree, item), &row_rect(tree, item)).right {
                    SendMessageW(tree, TVM_EXPAND, TVE_TOGGLE as usize, item);
                }
            }
            NM_DBLCLK => {
                let (x, y) = cursor(tree);
                app.zoom(node_of(tree, hit(tree, x, y)));
            }
            NM_RCLICK => {
                let (x, y) = cursor(tree);
                let item = hit(tree, x, y);
                let n = node_of(tree, item);
                if n != NONE {
                    SendMessageW(tree, TVM_SELECTITEM, TVGN_CARET as usize, item);
                    app.context_menu(n);
                }
                return Some(1);
            }
            TVN_KEYDOWN => match (*(nm as *const NMTVKEYDOWN)).wVKey {
                VK_RETURN => app.zoom(node_of(tree, selected(tree))),
                VK_BACK => app.up(),
                _ => {}
            },
            _ => {}
        }
    }
    None
}

fn draw(app: &App, cd: &NMTVCUSTOMDRAW) {
    let (t, s, hdc, item) = (&app.t, app.s, cd.nmcd.hdc, cd.nmcd.dwItemSpec as HTREEITEM);
    let mut client = RECT::default();
    unsafe { GetClientRect(app.tree, &mut client) };
    let row = RECT { left: 0, right: client.right, ..cd.nmcd.rc };
    let px = |v: f32| (v * s) as i32;
    gfx::fill_rect(hdc, &row, t.back);
    let gl = glyph(app, cd.iLevel, &row);
    let one = gfx::LINE | DT_END_ELLIPSIS;
    match decode(cd.nmcd.lItemlParam) {
        Row::More(p) => {
            let node = app.a.node(p);
            let shown: u64 = largest(app, p).iter().map(|&k| app.a.node(k).size).sum();
            let text = tf("tree.more", &[&util::num((node.kids.len() - MAX_ROWS) as u64), &util::size(node.size - shown)]);
            let r = RECT { left: gl.right + px(20.0), ..row };
            gfx::text(hdc, &text, &r, t.muted, app.ui, one);
        }
        Row::Node(n) => {
            let node = app.a.node(n);
            let swatch = gfx::rect(gl.right + px(2.0), row.top + (row.bottom - row.top - px(10.0)) / 2, px(10.0), px(10.0));
            {
                let g = G::new(hdc);
                if item == selected(app.tree) || item == app.tree_hot {
                    let r = RECT { left: row.left + px(4.0), right: row.right - px(4.0), ..row };
                    g.round(&r, 4.0 * s, if item == selected(app.tree) { t.select } else { t.row_hover });
                }
                g.round(&swatch, 2.0 * s, app.chart.color_of(&app.a, n, t));
            }
            if node.dir && !node.kids.is_empty() {
                let open = unsafe { SendMessageW(app.tree, TVM_GETITEMSTATE, item as usize, TVIS_EXPANDED as isize) } != 0;
                gfx::text(hdc, if open { "\u{E70D}" } else { "\u{E76C}" }, &gl, t.muted, app.icon_small, gfx::LINE | DT_CENTER);
            }
            // size and share of the current view on the right, the name takes what is left
            let pct = RECT { left: row.right - px(56.0), right: row.right - px(10.0), ..row };
            let size = RECT { left: pct.left - px(74.0), right: pct.left - px(4.0), ..row };
            let whole = app.a.node(app.focus).size;
            gfx::text(hdc, &format!("{}%", util::pct(node.size, whole)), &pct, t.muted, app.ui, gfx::LINE | DT_RIGHT);
            gfx::text(hdc, &util::size(node.size), &size, t.text, app.ui, gfx::LINE | DT_RIGHT);
            let name = RECT { left: swatch.right + px(6.0), right: size.left - px(6.0), ..row };
            gfx::text(hdc, &app.a.name(n), &name, if node.dir { t.text } else { t.muted }, app.ui, one);
        }
    }
}

// follows the row under the mouse: it is drawn highlighted and lights up on the chart
unsafe extern "system" fn hot_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _: usize, _: usize) -> LRESULT {
    let item = match msg {
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            unsafe { TrackMouseEvent(&mut tme) };
            Some(hit(h, (lp & 0xffff) as i16 as i32, (lp >> 16 & 0xffff) as i16 as i32))
        }
        WM_MOUSELEAVE => Some(0),
        _ => None,
    };
    let app = app();
    if let Some(item) = item.filter(|&i| i != app.tree_hot) {
        for old in [app.tree_hot, item] {
            if old != 0 {
                unsafe { InvalidateRect(h, &row_rect(h, old), 0) };
            }
        }
        app.tree_hot = item;
        app.tree_hover(node_of(h, item));
    }
    unsafe { DefSubclassProc(h, msg, wp, lp) }
}
