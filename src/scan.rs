// The scanned tree and the scanner. Nodes live in one array and refer to each other by
// index; their names are packed into one UTF-16 buffer, NUL-terminated for the Win32 calls.
use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, GetLastError, INVALID_HANDLE_VALUE},
    Globalization::lstrcmpiW,
    Storage::FileSystem::*,
};

use crate::util::wide;

pub const NONE: u32 = u32::MAX;

pub struct Node {
    name: u32, // where the name starts in Arena::names
    pub parent: u32,
    pub kids: Vec<u32>,
    pub size: u64,
    pub files: u64,
    pub dir: bool,
    pub summary: bool, // the made-up root that holds all scanned drives
}

#[derive(Default)]
pub struct Arena {
    pub nodes: Vec<Node>,
    names: Vec<u16>,
    pub space: HashMap<u32, (u64, u64)>, // free and total bytes, for drive roots
}

impl Arena {
    pub fn add(&mut self, name: &[u16], parent: u32, dir: bool) -> u32 {
        let i = self.nodes.len() as u32;
        self.nodes.push(Node { name: self.names.len() as u32, parent, kids: Vec::new(), size: 0, files: 0, dir, summary: false });
        self.names.extend_from_slice(name);
        self.names.push(0);
        if parent != NONE {
            self.nodes[parent as usize].kids.push(i);
        }
        i
    }

    pub fn node(&self, i: u32) -> &Node {
        &self.nodes[i as usize]
    }

    pub fn node_mut(&mut self, i: u32) -> &mut Node {
        &mut self.nodes[i as usize]
    }

    fn name_ptr(&self, i: u32) -> *const u16 {
        self.names[self.node(i).name as usize..].as_ptr()
    }

    pub fn name(&self, i: u32) -> String {
        let s = &self.names[self.node(i).name as usize..];
        String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
    }

    pub fn path(&self, i: u32) -> String {
        // a loop, not a recursion: a path can be 16,000 folders deep
        let mut chain = Vec::new();
        let mut p = i;
        while p != NONE {
            chain.push(p);
            p = self.node(p).parent;
        }
        let mut out = String::new();
        for (j, &n) in chain.iter().rev().enumerate() {
            if j > 0 && !out.ends_with('\\') {
                out.push('\\');
            }
            out += &self.name(n);
        }
        out
    }

    // the top-level name: a drive root keeps its path ("C:\"), a folder its own name
    pub fn label(&self, i: u32) -> String {
        if self.node(i).parent == NONE { self.path(i) } else { self.name(i) }
    }

    // the made-up root of Summary; the drives keep no parent, so they stay where they are
    pub fn summary(&mut self, name: &str, kids: Vec<u32>) -> u32 {
        let i = self.add(&name.encode_utf16().collect::<Vec<_>>(), NONE, true);
        let (size, files) = kids.iter().fold((0, 0), |(s, f), &k| (s + self.node(k).size, f + self.node(k).files));
        let n = self.node_mut(i);
        (n.kids, n.size, n.files, n.summary) = (kids, size, files, true);
        i
    }

    // Moves the nodes of a finished scan in; returns where its root (node 0) now is.
    pub fn merge(&mut self, other: Arena) -> u32 {
        let (base, names) = (self.nodes.len() as u32, self.names.len() as u32);
        for mut n in other.nodes {
            n.name += names;
            if n.parent != NONE {
                n.parent += base;
            }
            n.kids.iter_mut().for_each(|k| *k += base);
            self.nodes.push(n);
        }
        self.names.extend(other.names);
        base
    }

    // A copy that holds only what hangs under the given roots; returns where every old node
    // went (NONE: dropped).
    pub fn compact(&mut self, roots: &[u32]) -> Vec<u32> {
        let mut map = vec![NONE; self.nodes.len()];
        let mut out = Arena::default();
        for &r in roots {
            self.copy_into(r, NONE, &mut out, &mut map);
        }
        out.space = self.space.iter().filter(|(k, _)| map[**k as usize] != NONE).map(|(k, v)| (map[*k as usize], *v)).collect();
        *self = out;
        map
    }

    // depth first, in the order of the tree, with a stack of its own: the folders can be very deep
    fn copy_into(&self, i: u32, parent: u32, out: &mut Arena, map: &mut [u32]) {
        let mut todo = vec![(i, parent)];
        while let Some((i, parent)) = todo.pop() {
            let n = self.node(i);
            let s = &self.names[n.name as usize..];
            let k = out.add(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())], parent, n.dir);
            (out.node_mut(k).size, out.node_mut(k).files) = (n.size, n.files);
            map[i as usize] = k;
            todo.extend(n.kids.iter().rev().map(|&c| (c, k)));
        }
    }

    // folders first, then files, each group by name the way Explorer compares them
    fn sort_kids(&mut self, i: u32) {
        let mut kids = std::mem::take(&mut self.node_mut(i).kids);
        kids.sort_by(|&a, &b| {
            let (na, nb) = (self.node(a), self.node(b));
            nb.dir.cmp(&na.dir).then_with(|| unsafe { lstrcmpiW(self.name_ptr(a), self.name_ptr(b)) }.cmp(&0))
        });
        self.node_mut(i).kids = kids;
    }

    // whether a node is the given one or hangs anywhere below it
    pub fn under(&self, n: u32, ancestor: u32) -> bool {
        let mut p = n;
        while p != NONE {
            if p == ancestor {
                return true;
            }
            p = self.node(p).parent;
        }
        false
    }

    // Applies a size change to a node and every node above it.
    pub fn propagate(&mut self, from: u32, d_size: i64, d_files: i64) {
        let mut p = from;
        while p != NONE {
            let n = self.node_mut(p);
            n.size = n.size.wrapping_add_signed(d_size);
            n.files = n.files.wrapping_add_signed(d_files);
            p = n.parent;
        }
    }
}

// The order the chart and the folder panel both use: the tree's own (folders first, by
// name) or largest first.
pub static BY_SIZE: AtomicBool = AtomicBool::new(false);

pub fn ordered(a: &Arena, n: u32) -> Vec<u32> {
    let mut kids = a.node(n).kids.clone();
    if BY_SIZE.load(Relaxed) {
        kids.sort_by(|&x, &y| a.node(y).size.cmp(&a.node(x).size));
    }
    kids
}

// Progress of a running scan; the window reads it while the scan thread writes it.
#[derive(Default)]
pub struct Stats {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
}

// Scans a folder or a drive into a new arena whose root is node 0.
pub fn scan(path: &str, name: &str, st: &Stats) -> Arena {
    let mut a = Arena::default();
    let root = a.add(&name.encode_utf16().collect::<Vec<_>>(), NONE, true);
    // the \\?\ prefix lifts the 260-character limit on paths
    let mut buf: Vec<u16> = format!("\\\\?\\{}", path.trim_end_matches('\\')).encode_utf16().collect();
    buf.push('\\' as u16);
    walk(&mut a, root, &mut buf, st);
    a
}

fn walk(a: &mut Arena, me: u32, buf: &mut Vec<u16>, st: &Stats) {
    *st.current.lock().unwrap() = String::from_utf16_lossy(&buf[4..]);
    let base = buf.len();
    buf.extend_from_slice(&['*' as u16, 0]);
    let mut fd: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let h = unsafe {
        FindFirstFileExW(buf.as_ptr(), FindExInfoBasic, (&mut fd as *mut WIN32_FIND_DATAW).cast::<c_void>(), FindExSearchNameMatch, std::ptr::null(), FIND_FIRST_EX_LARGE_FETCH)
    };
    buf.truncate(base);
    if h == INVALID_HANDLE_VALUE {
        if unsafe { GetLastError() } != ERROR_FILE_NOT_FOUND {
            st.errors.fetch_add(1, Relaxed);
        }
        return;
    }
    loop {
        if st.cancel.load(Relaxed) {
            break;
        }
        let len = fd.cFileName.iter().position(|&c| c == 0).unwrap_or(fd.cFileName.len());
        let name = &fd.cFileName[..len];
        let dots = name == ['.' as u16] || name == ['.' as u16, '.' as u16];
        // junctions and symbolic links would count things twice, or loop
        if !dots && fd.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            if fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                let k = a.add(name, me, true);
                buf.extend_from_slice(name);
                buf.push('\\' as u16);
                walk(a, k, buf, st);
                buf.truncate(base);
                let (size, files) = (a.node(k).size, a.node(k).files);
                let n = a.node_mut(me);
                n.size += size;
                n.files += files;
            } else {
                let size = (fd.nFileSizeHigh as u64) << 32 | fd.nFileSizeLow as u64;
                let k = a.add(name, me, false);
                (a.node_mut(k).size, a.node_mut(k).files) = (size, 1);
                let n = a.node_mut(me);
                n.size += size;
                n.files += 1;
                st.files.fetch_add(1, Relaxed);
                st.bytes.fetch_add(size, Relaxed);
            }
        }
        if unsafe { FindNextFileW(h, &mut fd) } == 0 {
            break;
        }
    }
    unsafe { FindClose(h) };
    a.sort_kids(me);
}

pub struct Drive {
    pub root: String, // "C:\"
    pub kind: u32,
}

// the drives that are ready to be read, as in the drive list on the left
pub fn drives() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i as u8) as char))
        .filter_map(|root| {
            let w = wide(&root);
            let kind = unsafe { GetDriveTypeW(w.as_ptr()) };
            let ready = unsafe { GetVolumeInformationW(w.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0) } != 0;
            (ready && kind > 1).then_some(Drive { root, kind })
        })
        .take(11)
        .collect()
}

// free and total bytes of a drive
pub fn space(root: &str) -> Option<(u64, u64)> {
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(wide(root).as_ptr(), &mut avail, &mut total, &mut free) } != 0;
    ok.then_some((free, total))
}

pub fn volume_label(root: &str) -> String {
    let mut buf = [0u16; 64];
    let ok = unsafe { GetVolumeInformationW(wide(root).as_ptr(), buf.as_mut_ptr(), buf.len() as u32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0) } != 0;
    if ok { String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]) } else { String::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a chain of folders as deep as a 32767-character path allows ("a\" is 2 characters)
    fn chain(depth: usize) -> (Arena, u32) {
        let mut a = Arena::default();
        let mut n = a.add(&[b'C' as u16, b':' as u16, 0x5C], NONE, true);
        let root = n;
        for _ in 0..depth {
            n = a.add(&['a' as u16], n, true);
        }
        a.propagate(n, 1, 1);
        (a, root)
    }

    // the window thread has the default 1 MB stack
    fn on_main_sized_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new().stack_size(1 << 20).spawn(f).unwrap().join().unwrap();
    }

    #[test]
    fn path_of_deepest_folder() {
        on_main_sized_stack(|| {
            let (a, _) = chain(16_000);
            assert_eq!(a.path(a.nodes.len() as u32 - 1).len(), 3 + 16_000 * 2 - 1);
        });
    }

    #[test]
    fn compact_deep_tree() {
        on_main_sized_stack(|| {
            let (mut a, root) = chain(16_000);
            let map = a.compact(&[root]);
            assert_eq!(a.nodes.len(), 16_001);
            assert_ne!(map[0], NONE);
        });
    }
}
