//! NestBruno: synchronize NestJS controllers into a Bruno collection.
//!
//! The binary in `main.rs` is a thin wrapper; all behavior lives in this
//! library so it can be tested directly.

pub mod bruno;
pub mod cli;
pub mod commands;
pub mod config;
pub mod diag;
pub mod discovery;
pub mod error;
pub mod nest;
pub mod output;
pub mod project;
pub mod resolve;
pub mod route;
pub mod staged;
pub mod sync;
pub mod watch;
pub mod workspace;
