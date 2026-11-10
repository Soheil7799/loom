//! What loom does, without a UI: the GTK app is one face of it, and later ones (macOS,
//! Windows) draw the same thing. Nothing here knows about widgets or a main loop: work runs
//! on its own threads and reports over channels.

pub mod agent;
pub mod api;
pub mod asks;
pub mod audit;
pub mod bridges;
pub mod board;
pub mod changes;
pub mod compacted;
pub mod files;
pub mod control;
pub mod history;
pub mod hub;
pub mod jobs;
pub mod jobs_api;
pub mod library;
pub mod journal;
pub mod lanes;
pub mod log;
pub mod mcp;
pub mod memory;
pub mod notes;
pub mod notes_api;
pub mod notes_chat;
pub mod paths;
pub mod plugin;
pub mod project_files;
pub mod provider;
pub mod quickopen;
pub mod repos;
pub mod rt;
pub mod schedule;
pub mod sandbox;
pub mod scope;
pub mod scribe;
pub mod settings;
pub mod state;
pub mod stt;
pub mod topics;
pub mod vault;
pub mod workflow;
pub mod workspace;
