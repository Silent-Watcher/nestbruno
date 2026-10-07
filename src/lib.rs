//! NestBruno: synchronize NestJS controllers into a Bruno collection.
//!
//! The binary in `main.rs` is a thin wrapper; all behavior lives in this
//! library so it can be tested directly.

pub mod cli;
pub mod commands;
pub mod config;
pub mod diag;
pub mod discovery;
pub mod error;
pub mod nest;
pub mod project;
pub mod route;
