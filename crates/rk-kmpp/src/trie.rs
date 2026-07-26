//! Read-only walker for the kernel's name-lookup trie.
//!
//! `kmpp.ko` builds a trie mapping field / object names to payloads and maps
//! it read-only into this process (`KMPP_SHM_IOC_QUERY_INFO`). The node
//! format is shared verbatim between kernel (`sys/kmpp_trie.c`) and the
//! reference userspace (`mpp/base/mpp_trie.c`); this module ports the lookup
//! walk and info iteration.
//!
//! Layout, from the root pointer:
//!
//! ```text
//! [ node 0 ][ node 1 ] ... [ node N-1 ][ info records ... ]
//! ```
//!
//! where `N` is recorded in the trie itself under the reserved key
//! `"__node__"`. Each info record is `{u32 header}{name bytes}{ctx bytes}`
//! with header bitfields `index:12 | ctx_len:12 | str_len:8`.
//!
//! Names are looked up 4 bits at a time (high nibble then low nibble of each
//! byte), with up to 16 nibbles of prefix compressed into a node's 64-bit
//! `tag_val`.

use crate::{Error, Result};

/// `sizeof(MppTrieNode)`: s16 next[16], s32 id, s16 idx, s16 prev,
/// u64 tag_val, u16 key, s16 tag_len, u16 next_cnt (+2 pad).
const NODE_SIZE: usize = 56;
const OFF_NEXT: usize = 0;
const OFF_ID: usize = 32;
const OFF_TAG_VAL: usize = 40;
const OFF_TAG_LEN: usize = 50;

/// One `{header, name, ctx}` info record in the trie.
#[derive(Debug, Clone, Copy)]
pub struct TrieInfo {
    ptr: *const u8,
}

impl TrieInfo {
    pub fn index(&self) -> u32 {
        self.header() & 0xfff
    }
    pub fn ctx_len(&self) -> usize {
        ((self.header() >> 12) & 0xfff) as usize
    }
    pub fn str_len(&self) -> usize {
        ((self.header() >> 24) & 0xff) as usize
    }

    fn header(&self) -> u32 {
        unsafe { (self.ptr as *const u32).read_unaligned() }
    }

    /// The record's name (NUL-padded in storage).
    pub fn name(&self) -> &str {
        unsafe {
            let s = core::slice::from_raw_parts(self.ptr.add(4), self.str_len());
            let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
            core::str::from_utf8(&s[..end]).unwrap_or("")
        }
    }

    pub fn ctx_ptr(&self) -> *const u8 {
        unsafe { self.ptr.add(4 + self.str_len()) }
    }

    pub fn ctx_u32(&self) -> Option<u32> {
        (self.ctx_len() >= 4)
            .then(|| unsafe { (self.ctx_ptr() as *const u32).read_unaligned() })
    }

    pub fn ctx_u64(&self) -> Option<u64> {
        (self.ctx_len() >= 8)
            .then(|| unsafe { (self.ctx_ptr() as *const u64).read_unaligned() })
    }

    fn next(&self) -> TrieInfo {
        TrieInfo {
            ptr: unsafe { self.ptr.add(4 + self.str_len() + self.ctx_len()) },
        }
    }
}

/// A borrowed view of a kernel trie. The mapping stays valid for as long as
/// the owning `/dev/kmpp_objs` fd is open; [`crate::Kmpp`] guarantees that.
#[derive(Debug, Clone, Copy)]
pub struct Trie {
    root: *const u8,
    node_count: u32,
    info_count: u32,
}

unsafe impl Send for Trie {}
unsafe impl Sync for Trie {}

impl Trie {
    /// Import a trie from a kernel-provided root pointer.
    ///
    /// # Safety
    /// `root` must be the userspace address of a kernel trie mapping that
    /// outlives the returned value.
    pub unsafe fn from_root(root: *const u8) -> Result<Trie> {
        Self::try_from_root(root).ok_or(Error::BadKernelData("trie has no __node__ record"))
    }

    /// Same as [`Trie::from_root`] but `None` for a trie whose self-describing
    /// records cannot be reached.
    ///
    /// Some kernel objdef sub-tries (`KmppVenc`, `KmppVdec` — the skeleton
    /// object types with no fields) come out of the kernel's trie compressor
    /// with a *tag* node at index 0, which the shared walk algorithm cannot
    /// enter: the walk starts with `match == 0 == node.idx`, so the first
    /// nibble takes the node's `next[]` branch instead of the tag branch and
    /// every `__`-prefixed name misses. Rockchip's own userspace hits this
    /// too — `mpp_trie_init_by_root` leaves such a trie at
    /// `node_used = 0, info_count = 0` and the client carries on — so this is
    /// tolerated rather than treated as a fault.
    ///
    /// # Safety
    /// As [`Trie::from_root`].
    pub unsafe fn try_from_root(root: *const u8) -> Option<Trie> {
        let mut t = Trie {
            root,
            // Provisional counts: raw lookup only needs the nodes, which we
            // reach through the root pointer without knowing the counts.
            node_count: 0,
            info_count: 0,
        };
        t.node_count = t.lookup_raw("__node__").and_then(|i| i.ctx_u32())?;
        t.info_count = t.lookup_raw("__info__").and_then(|i| i.ctx_u32())?;
        Some(t)
    }

    pub fn root(&self) -> *const u8 {
        self.root
    }

    pub fn info_count(&self) -> u32 {
        self.info_count
    }

    fn node(&self, idx: i32) -> *const u8 {
        unsafe { self.root.add(idx as usize * NODE_SIZE) }
    }

    fn node_next(&self, node: *const u8, nibble: u32) -> i32 {
        unsafe {
            (node.add(OFF_NEXT + 2 * nibble as usize) as *const i16).read_unaligned() as i32
        }
    }

    fn node_id(&self, node: *const u8) -> i32 {
        unsafe { (node.add(OFF_ID) as *const i32).read_unaligned() }
    }

    fn node_idx(&self, node: *const u8) -> i32 {
        unsafe { (node.add(OFF_ID + 4) as *const i16).read_unaligned() as i32 }
    }

    fn node_tag_val(&self, node: *const u8) -> u64 {
        unsafe { (node.add(OFF_TAG_VAL) as *const u64).read_unaligned() }
    }

    fn node_tag_len(&self, node: *const u8) -> i32 {
        unsafe { (node.add(OFF_TAG_LEN) as *const i16).read_unaligned() as i32 }
    }

    /// Look up a name; `None` if absent.
    pub fn lookup(&self, name: &str) -> Option<TrieInfo> {
        self.lookup_raw(name)
    }

    fn lookup_raw(&self, name: &str) -> Option<TrieInfo> {
        let bytes = name.as_bytes();
        if bytes.is_empty() {
            return None;
        }

        // Walk state: current prefix-tag accumulator and the index of the
        // node whose tag was fully matched (so the next step may leave it).
        let mut w = Walk {
            tag: 0,
            len: 0,
            matched: 0,
        };
        let mut idx: i32 = 0;

        for (i, &c) in bytes.iter().enumerate() {
            let end = i + 1 == bytes.len();
            idx = self.step(&mut w, idx, (c >> 4) as u32 & 0xf, false)?;
            idx = self.step(&mut w, idx, c as u32 & 0xf, end)?;
        }

        let node = self.node(idx);
        let id = self.node_id(node);
        if id < 0 {
            return None;
        }
        Some(TrieInfo {
            ptr: unsafe { self.root.add(id as usize) },
        })
    }

    fn step(&self, w: &mut Walk, idx: i32, nibble: u32, end: bool) -> Option<i32> {
        let node = self.node(idx);
        let tag_len = self.node_tag_len(node);
        let node_idx = self.node_idx(node);

        let next = if tag_len == 0 || w.matched == node_idx {
            // Normal 16-way branch node, or the node's tag was already
            // consumed on a previous step.
            w.tag = 0;
            w.len = 0;
            w.matched = -1;
            self.node_next(node, nibble)
        } else {
            let val_new = (w.tag << 4) | nibble as u64;
            let len_new = w.len + 1;
            w.matched = -1;
            if tag_len > len_new && !end {
                // Keep accumulating the compressed prefix in this node.
                w.tag = val_new;
                w.len = len_new;
                node_idx
            } else {
                w.tag = 0;
                w.len = 0;
                if self.node_tag_val(node) != val_new {
                    -1
                } else {
                    w.matched = node_idx;
                    node_idx
                }
            }
        };

        (next >= 0).then_some(next)
    }

    /// First info record (records live directly after the node array).
    pub fn info_first(&self) -> Option<TrieInfo> {
        (self.info_count > 0).then(|| TrieInfo {
            ptr: unsafe { self.root.add(self.node_count as usize * NODE_SIZE) },
        })
    }

    /// Successor of `info` in insertion order.
    pub fn info_next(&self, info: TrieInfo) -> Option<TrieInfo> {
        (info.index() + 1 < self.info_count).then(|| info.next())
    }
}

struct Walk {
    tag: u64,
    len: i32,
    matched: i32,
}
