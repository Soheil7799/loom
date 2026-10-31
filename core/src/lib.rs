//! What loom does, without a UI: the GTK app is one face of it, and later ones (macOS,
//! Windows) draw the same thing. Nothing here knows about widgets or a main loop: work runs
//! on its own threads and reports over channels.

pub mod agent;
pub mod bridges;
pub mod compacted;
pub mod control;
pub mod jobs;
pub mod journal;
pub mod log;
pub mod mcp;
pub mod memory;
pub mod notes;
pub mod paths;
pub mod plugin;
pub mod provider;
pub mod quickopen;
pub mod repos;
pub mod rt;
pub mod schedule;
pub mod scribe;
pub mod settings;
pub mod state;
pub mod stt;
pub mod topics;
pub mod vault;
pub mod workflow;
