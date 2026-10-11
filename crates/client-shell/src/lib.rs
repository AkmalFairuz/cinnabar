//! Menu input, launcher services, desktop settings and Discord presence.
#[cfg(not(target_arch = "wasm32"))]
pub mod discord_presence;
pub mod fullscreen;
pub mod join_progress;
pub mod local_worlds;
pub mod menu;
pub mod player_skin;
mod plugin;
pub mod presence_target;
pub mod screen_policy;
pub mod settings_runtime;
pub use plugin::ClientShellPlugin;
