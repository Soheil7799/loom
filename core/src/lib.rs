//! What loom does, without a UI: the GTK app is one face of it, and later ones (macOS,
//! Windows) draw the same thing. Nothing here knows about widgets or a main loop: work runs
//! on its own threads and reports over channels.

pub mod jobs;
