// SPDX-License-Identifier: MPL-2.0
//! POSIX filesystem services shared by the Linux and macOS adapters (PR-030).
//!
//! Saves keep the displaced version and swap the stage onto the name in one
//! rename, path trust is bound to the opened objects through retained directory
//! descriptors, capability reports classify the mount, owned caches use
//! `flock` leases that other processes respect, and `paths` resolves the
//! per-user data folders. `process` identifies processes (for cache owners and
//! recovery journals) and counts their resources, and `trash` moves entries to
//! the user's trash. Off Unix the crate compiles to an empty unit so the
//! workspace keeps one member list on every OS.
#![cfg(unix)]

mod cache;
mod capability;
mod entries;
mod files;
pub mod paths;
pub mod process;
mod resolve;
mod sys;
mod transaction;
pub mod trash;
mod trust;

pub use capability::PosixFilesystemCapability;
pub use files::PosixFileSystem;
pub use trust::{DirectoryGuard, PosixPathTrustProvider, PosixSessionPathTrustProvider};
