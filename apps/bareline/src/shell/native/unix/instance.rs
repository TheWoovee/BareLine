// SPDX-License-Identifier: MPL-2.0
//! Single-instance handoff through a Unix socket in the user's private runtime
//! folder (ADR-C), shared by Linux and macOS: the same framed protocol, limits
//! and outcomes as the Windows named pipe. The first window of a profile owns
//! the socket under an exclusive lock; a later launch hands its files over and
//! exits, and a socket left behind by a crashed owner is replaced.
pub use bareline_platform_posix::instance::{InstanceServer, OpenRequest, Outcome, coordinate};
