//! Jafnify P2P tunnel -- Linux frontend library.
//!
//! Composes jafnify-core primitives (pairing, discovery, handshake,
//! config generation, state) with the kernel WireGuard backend and
//! pkexec root escalation. Consumed by the `jafnify` CLI and the
//! `jafnify-gui` egui frontend.

pub mod app;
pub mod applog;
pub mod autostart;
pub mod backend;
pub mod children;
pub mod cli;
pub mod clipboard;
pub mod code_entry;
pub mod connection_log;
pub mod control;
pub mod daemon;
pub mod details;
pub mod diag;
pub mod drop_store;
pub mod exec;
pub mod export;
pub mod firewall;
pub mod flags;
pub mod flavor;
pub mod good_hours_store;
pub mod helper;
pub mod home_layout;
pub mod host_dns;
pub mod host_hints;
pub mod host_probe;
pub mod icon;
pub mod link_test;
pub mod local_name;
pub mod nav;
pub mod pages;
pub mod notification;
pub mod owner;
pub mod palette;
pub mod phone_stats;
pub mod polkit;
pub mod probe;
pub mod root;
pub mod route;
pub mod scale;
pub mod session;
pub mod settings;
pub mod station;
pub mod stop;
pub mod terms;
pub mod theme;
pub mod tile_scopes;
pub mod tray;
pub mod ui_text;
pub mod uplink;
pub mod usage_store;
pub mod view;
pub mod wifi_scan;

/// Directory name under `dirs::config_dir()` holding Jafnify state.
pub const APP_DIR: &str = flavor::NAME;
