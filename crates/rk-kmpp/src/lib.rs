//! Pure-Rust userspace client for Rockchip KMPP (`kmpp.ko`), the in-kernel
//! video encoder on RV1126B-class SoCs.
//!
//! This crate speaks the raw kernel ABI directly — `/dev/kmpp_objs` for the
//! reflective shared-memory object system and `/dev/vcodec` for encoder
//! channel control — with no dependency on `librockchip_mpp`.
//!
//! # Architecture
//!
//! KMPP runs the whole encoder (bitstream syntax, rate control, register
//! generation) inside `kmpp.ko`. Userspace only moves *shared-memory objects*
//! across the boundary:
//!
//! * [`Kmpp`] — connection to `/dev/kmpp_objs`. At init the kernel's object
//!   definition trie is mmap'd read-only into the process; every field access
//!   is resolved by *name* against kernel-provided `{offset, size, flag}`
//!   tables, so this crate never hardcodes kernel struct layouts.
//! * [`Obj`] — one kernel-allocated shared-memory object (`KmppFrame`,
//!   `KmppPacket`, `KmppVencStCfg`, ...) mapped into both address spaces.
//! * [`venc::Venc`] — one encoder channel on `/dev/vcodec`.
//!
//! # Non-blocking model
//!
//! The API is low-level and non-blocking:
//!
//! * [`venc::Venc::put_frame`] submits a dma-buf backed frame and never
//!   blocks (the channel is created with a non-blocking input timeout).
//! * Output readiness is signalled by the channel fd becoming readable —
//!   [`venc::Venc`] implements [`std::os::fd::AsFd`]/[`AsRawFd`] so it can be
//!   registered with `poll(2)`, `epoll`, tokio's `AsyncFd`, etc.
//! * [`venc::Venc::get_packet`] fetches one encoded packet without blocking,
//!   returning `Ok(None)` when nothing is queued.
//!
//! [`AsRawFd`]: std::os::fd::AsRawFd

pub mod abi;
pub mod objs;
pub mod trie;
pub mod types;
pub mod venc;

pub use objs::{Kmpp, Obj};

use std::io;

/// Errors produced by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A syscall (open / ioctl / mmap) failed.
    #[error("{op}: {source}")]
    Sys {
        op: &'static str,
        #[source]
        source: io::Error,
    },

    /// An object type name was not found in the kernel's objdef trie.
    #[error("kernel object type {0:?} not found")]
    UnknownObjType(String),

    /// A field name was not found in an object's entry table.
    #[error("object {obj:?} has no field {field:?}")]
    UnknownField { obj: &'static str, field: String },

    /// A field exists but its kernel-side size does not match the access.
    #[error("object {obj:?} field {field:?} has size {actual}, access wants {wanted}")]
    SizeMismatch {
        obj: &'static str,
        field: String,
        actual: usize,
        wanted: usize,
    },

    /// The kernel returned an object of an unexpected type.
    #[error("expected object type {expected:?}, kernel returned {actual:?}")]
    TypeMismatch { expected: &'static str, actual: String },

    /// The kernel trie / shared object data failed a sanity check.
    #[error("kernel shared data invalid: {0}")]
    BadKernelData(&'static str),

    /// The channel's input frame queue is full (`EAGAIN` from the patched
    /// vcodec driver, which reports backpressure distinctly from faults).
    #[error("encoder input queue full")]
    QueueFull,
}

impl Error {
    pub(crate) fn sys(op: &'static str) -> Self {
        Error::Sys {
            op,
            source: io::Error::last_os_error(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
