// SPDX-License-Identifier: MPL-2.0
//! POSIX services shared by the Linux and macOS adapters: the filesystem
//! (PR-030), the single-instance handoff and the extension host transport (PR-031).
//!
//! Saves keep the displaced version and swap the stage onto the name in one
//! rename, path trust is bound to the opened objects through retained directory
//! descriptors, capability reports classify the mount, owned caches use
//! `flock` leases that other processes respect, and `paths` resolves the
//! per-user data folders. Off Unix the crate compiles to an empty unit so the
//! workspace keeps one member list on every OS.
#![cfg(unix)]

mod cache;
mod capability;
mod entries;
pub mod extension_transport;
mod files;
pub mod instance;
mod ipc;
pub mod paths;
mod process;
mod resolve;
mod sys;
mod transaction;
mod trust;

pub use capability::PosixFilesystemCapability;
pub use files::PosixFileSystem;
pub use trust::{DirectoryGuard, PosixPathTrustProvider, PosixSessionPathTrustProvider};
