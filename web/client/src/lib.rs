//! Cinnabar's browser entry. The page prepares the carriers, signs in on the web relay and picks a
//! server, then calls [`start`]; the client runs on the page's event loop from then on.
#![cfg(target_arch = "wasm32")]

use std::path::PathBuf;

use assets::carriers::file::MemoryCarriers;
use cinnabar::args::{ClientArgs, ParseOutcome};
use launcher::install_layout::InstallLayout;
use wasm_bindgen::prelude::*;

/// The carriers the page hands over, keyed by their place in the browser's install layout.
#[wasm_bindgen]
#[derive(Default)]
pub struct Carriers {
    carriers: MemoryCarriers,
    layout: Layout,
}

struct Layout(InstallLayout);

impl Default for Layout {
    fn default() -> Self {
        Self(InstallLayout::browser())
    }
}

impl Carriers {
    /// Where `relative_path` sits below the compiled-assets directory; it may not leave it.
    fn compiled_path(&self, relative_path: &str) -> Result<PathBuf, JsError> {
        let relative = PathBuf::from(relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(JsError::new(
                "carrier paths are relative and stay below their root",
            ));
        }
        Ok(self.layout.0.compiled_assets.join(relative))
    }
}

/// The sound bank's file name below the compiled-assets directory.
#[wasm_bindgen]
#[must_use]
pub fn sound_bank_file() -> String {
    assets::carriers::AUDIO_BANK.output.to_owned()
}

/// The directory, below the compiled-assets directory, of the languages a player can switch to;
/// the page hands them over with [`Carriers::add_remote_compiled`].
#[wasm_bindgen]
#[must_use]
pub fn language_carrier_dir() -> String {
    assets::carriers::LANGUAGES.output.to_owned()
}

/// Imports a resource pack file the player dropped on the page into the running client's
/// Global Resources, as the in-game Import button does.
#[wasm_bindgen]
pub fn import_resource_pack(name: String, bytes: Vec<u8>) {
    cinnabar::import_resource_pack(name, bytes);
}

/// The file extensions the Global Resources import accepts.
#[wasm_bindgen]
#[must_use]
pub fn resource_pack_extensions() -> Vec<String> {
    cinnabar::resource_pack_extensions()
        .iter()
        .map(|extension| (*extension).to_owned())
        .collect()
}

/// Whether the page can leave out the compiled file at `relative_path`: the prepare stamp or a
/// carrier's compile report, which only name files in rebuild hints and are never read.
#[wasm_bindgen]
#[must_use]
pub fn carrier_unused(relative_path: &str) -> bool {
    relative_path == assets::carriers::STAMP_FILE
        || assets::carriers::CARRIERS
            .iter()
            .any(|carrier| carrier.report == Some(relative_path))
}

/// Reports the web relay's latest join progress: the JSON of its control socket's `connect`
/// event, whose pack downloads the join screen shows.
#[wasm_bindgen]
pub fn set_connect_progress(json: &str) {
    client_shell::join_progress::set_relayed(json);
}

/// How many leading bytes of a sound bank [`sound_bank_prefix_length`] reads.
#[wasm_bindgen]
#[must_use]
pub fn sound_bank_header_bytes() -> usize {
    assets::sound_bank::SOUND_BANK_HEADER_BYTES
}

/// The length of a sound bank's index prefix, read from its first [`sound_bank_header_bytes`].
#[wasm_bindgen]
pub fn sound_bank_prefix_length(header: &[u8]) -> Result<usize, JsError> {
    assets::sound_bank::sound_bank_prefix_len(header)
        .map_err(|error| JsError::new(&error.to_string()))
}

/// How many bytes of hash end a sound bank's prefix; they name its contents.
#[wasm_bindgen]
#[must_use]
pub fn sound_bank_hash_bytes() -> usize {
    assets::sound_bank::SOUND_BANK_HASH_BYTES
}

/// Whether `prefix` is a whole, intact sound bank prefix, as a kept copy must be to stand in
/// for a download.
#[wasm_bindgen]
#[must_use]
pub fn sound_bank_prefix_valid(prefix: &[u8]) -> bool {
    assets::sound_bank::SoundBankIndex::decode_prefix(prefix).is_ok()
}

#[wasm_bindgen]
impl Carriers {
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a compiled carrier by its path below the compiled-assets directory, such as
    /// `vanilla-v2193.mcbea` or `lang/en_US.json`.
    pub fn add_compiled(&mut self, relative_path: &str, bytes: Vec<u8>) -> Result<(), JsError> {
        let path = self.compiled_path(relative_path)?;
        self.carriers.insert(path, bytes);
        Ok(())
    }

    /// Adds a compiled carrier the client downloads by byte range from `url` as it reads it;
    /// `prefix` holds its first bytes, its index. Only the sound bank is read this way.
    pub fn add_ranged_compiled(
        &mut self,
        relative_path: &str,
        prefix: Vec<u8>,
        url: String,
    ) -> Result<(), JsError> {
        let path = self.compiled_path(relative_path)?;
        self.carriers.insert_ranged(path, prefix, url);
        Ok(())
    }

    /// Adds a compiled carrier the client downloads whole from `url` only when it needs it, as
    /// the languages below [`language_carrier_dir`].
    pub fn add_remote_compiled(&mut self, relative_path: &str, url: String) -> Result<(), JsError> {
        let path = self.compiled_path(relative_path)?;
        self.carriers.insert_remote(path, url);
        Ok(())
    }

    /// Adds the pinned physics registry, which startup checks against its pin.
    pub fn set_physics_registry(&mut self, bytes: Vec<u8>) {
        let path = self.layout.0.physics_registry.clone();
        self.carriers.insert(path, bytes);
    }
}

/// Starts the client: it joins `server` (`host:port`) through the relay at `relay_url`, as the
/// relay session `session_id`. `display_name` names a signed-out player. `on_progress` is called
/// with each startup stage's label, its fraction or `null`, and whether the game now draws.
#[wasm_bindgen]
pub fn start(
    carriers: Carriers,
    relay_url: String,
    session_id: String,
    server: String,
    display_name: String,
    on_progress: js_sys::Function,
) -> Result<(), JsError> {
    // Startup runs before Bevy installs its own hook; panics should reach the console too.
    std::panic::set_hook(Box::new(console_error_panic_hook::hook));
    cinnabar::startup_progress::set_reporter(move |progress| {
        let fraction = progress
            .fraction
            .map_or(JsValue::NULL, |fraction| JsValue::from_f64(fraction.into()));
        let _ = on_progress.call3(
            &JsValue::NULL,
            &JsValue::from_str(progress.label),
            &fraction,
            &JsValue::from_bool(progress.ready),
        );
    });
    assets::carriers::file::preload(carriers.carriers);
    let args = match ClientArgs::parse_from([
        "cinnabar",
        "--address",
        &server,
        "--display-name",
        &display_name,
    ]) {
        Ok(ParseOutcome::Run(args)) => *args,
        Ok(ParseOutcome::Help) => return Err(JsError::new("unexpected help request")),
        Err(error) => return Err(JsError::new(&error.to_string())),
    };
    let relay = bridge::RelaySession {
        url: relay_url,
        session_id,
    };
    cinnabar::run_with(args, cinnabar::CoreLink::Relay(relay))
        .map_err(|error| JsError::new(&format!("{error:#}")))
}
