//! Razor's modules. Each is a directory under `src/` with one public surface (RULES.md 1, 4).

pub mod api;
pub(crate) mod children;
pub(crate) mod claude;
pub(crate) mod code_mode;
pub mod config;
pub mod controller;
pub(crate) mod devices;
pub mod extensions;
pub(crate) mod memory;
pub(crate) mod profiles;
pub mod protocol;
pub mod record;
pub mod residents;
pub mod responses;
pub(crate) mod shell;
pub mod terminal;
pub mod tools;
