//! Skin selection work stays outside the frame loop; results publish atomically.

use launcher::dressing_room::{Action, DressingRoomView, SkinModel};
use std::sync::Arc;
use {super::*, launcher::install_layout::InstallLayout, launcher::menu::MenuAction};

mod editor;
mod picker;
#[cfg(test)]
mod tests;

pub(super) struct Worker {
    commands: crossbeam_channel::Sender<Job>,
    results: crossbeam_channel::Receiver<Outcome>,
    pending: usize,
    #[cfg(target_arch = "wasm32")]
    inline: std::sync::Arc<std::sync::Mutex<Inline>>,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Worker")
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum Job {
    Select(usize),
    Model(launcher::dressing_room::SkinModel),
    Import(Option<PathBuf>),
    Rename(usize, String),
    Delete(usize),
    SelectCape(Option<usize>),
    ImportCape(Option<PathBuf>),
    RenameCape(usize, String),
    DeleteCape(usize),
    /// Files a browser player picked to import, by name; none when they cancelled.
    #[cfg(target_arch = "wasm32")]
    Picked {
        cape: bool,
        files: Vec<(String, Vec<u8>)>,
    },
}

#[derive(Debug)]
struct Outcome {
    view: DressingRoomView,
    changed: bool,
    packet: Option<protocol::Packet>,
    completed_command: bool,
}

impl Outcome {
    fn new(
        view: &DressingRoomView,
        before: Option<render_api::StandardSkin>,
        previous_model: Option<(SkinModel, Arc<str>)>,
        uuid: [u8; 16],
        completed_command: bool,
    ) -> Self {
        let active = view.active_skin();
        let selected = view.selected_skin();
        let changed = before != active
            || previous_model != selected.map(|entry| (entry.model, entry.engine_version.clone()));
        let mut packet = changed
            .then(|| {
                active.as_ref().zip(selected).map(|(skin, entry)| {
                    protocol::player_skin_packet(
                        uuid,
                        skin,
                        entry.model.arm_size(),
                        &entry.id,
                        &entry.name,
                    )
                })
            })
            .flatten();
        if let Some(packet) = &mut packet
            && let Some(selected) = selected
        {
            protocol::set_skin_packet_engine_version(packet, &selected.engine_version);
        }
        Self {
            view: view.clone(),
            changed,
            packet,
            completed_command,
        }
    }
}

/// The skin library and its step, run on the worker thread or, in a browser, inline.
struct Library {
    layout: InstallLayout,
    view: DressingRoomView,
    uuid: [u8; 16],
}

impl Library {
    /// Loads the library and the outcome that publishes it against the client's current skin.
    fn load(layout: InstallLayout, fallback: &crate::player_skin::LocalPlayerSkin) -> (Self, Outcome) {
        let view = crate::player_skin::catalog::load(&layout, fallback);
        let outcome = Outcome::new(
            &view,
            Some(fallback.standard_skin()),
            Some((fallback.model(), fallback.engine_version.clone())),
            fallback.local_uuid,
            true,
        );
        let library = Self {
            layout,
            view,
            uuid: fallback.local_uuid,
        };
        (library, outcome)
    }

    /// Changes the library: runs `job`, or with `None` merges the downloaded default capes.
    fn step(&mut self, job: Option<Job>) -> Outcome {
        let before = self.view.active_skin();
        let previous_model = self
            .view
            .selected_skin()
            .map(|entry| (entry.model, entry.engine_version.clone()));
        let completed_command = match job {
            Some(job) => {
                self.view.message = apply_job(&self.layout, &mut self.view, job).err();
                true
            }
            None => {
                crate::player_skin::catalog::merge_default_capes(&self.layout, &mut self.view);
                false
            }
        };
        Outcome::new(
            &self.view,
            before,
            previous_model,
            self.uuid,
            completed_command,
        )
    }
}

impl Worker {
    #[cfg(not(target_arch = "wasm32"))]
    fn start(
        layout: InstallLayout,
        fallback: crate::player_skin::LocalPlayerSkin,
    ) -> Result<Self, String> {
        Self::start_with_refresh(layout, fallback, |layout| {
            #[cfg(not(any(test, feature = "test-support")))]
            crate::player_skin::catalog::refresh_default_capes(layout);
            #[cfg(any(test, feature = "test-support"))]
            let _ = layout;
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start_with_refresh(
        layout: InstallLayout,
        fallback: crate::player_skin::LocalPlayerSkin,
        refresh: impl FnOnce(&InstallLayout) + Send + 'static,
    ) -> Result<Self, String> {
        let (commands, incoming) = crossbeam_channel::bounded(4);
        let (outgoing, results) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("skin-selection".to_owned())
            .spawn(move || {
                let (mut library, loaded) = Library::load(layout.clone(), &fallback);
                if outgoing.send(loaded).is_err() {
                    return;
                }
                let (finished, mut cape_done) = crossbeam_channel::bounded(1);
                if std::thread::Builder::new()
                    .name("cape-textures".to_owned())
                    .spawn(move || {
                        refresh(&layout);
                        let _ = finished.send(());
                    })
                    .is_err()
                {
                    cape_done = crossbeam_channel::never();
                }
                loop {
                    let job = crossbeam_channel::select! {
                        recv(incoming)->job=>{
                            let Ok(job)=job else {break;};
                            Some(job)
                        },
                        recv(cape_done)->done=>{
                            cape_done=crossbeam_channel::never();
                            if done.is_err() {continue;}
                            None
                        }
                    };
                    if outgoing.send(library.step(job)).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            commands,
            results,
            pending: 1,
        })
    }

    /// A browser has no threads, so the library runs inline in [`Worker::run_inline`]; default
    /// capes download only on desktops.
    #[cfg(target_arch = "wasm32")]
    fn start(
        layout: InstallLayout,
        fallback: crate::player_skin::LocalPlayerSkin,
    ) -> Result<Self, String> {
        let (commands, incoming) = crossbeam_channel::unbounded();
        let (outgoing, results) = crossbeam_channel::unbounded();
        let (library, loaded) = Library::load(layout, &fallback);
        let _ = outgoing.send(loaded);
        Ok(Self {
            commands,
            results,
            pending: 1,
            inline: std::sync::Arc::new(std::sync::Mutex::new(Inline {
                library,
                incoming,
                outgoing,
            })),
        })
    }

    /// Runs the queued jobs; an import first asks the page for files.
    #[cfg(target_arch = "wasm32")]
    fn run_inline(&self) {
        let Ok(mut inline) = self.inline.lock() else {
            return;
        };
        while let Ok(job) = inline.incoming.try_recv() {
            match job {
                Job::Import(None) => picker::pick_in_page(false, self.commands.clone()),
                Job::ImportCape(None) => picker::pick_in_page(true, self.commands.clone()),
                Job::Picked { cape, files } => {
                    let outcome = match inline.library.stage_picked(cape, files) {
                        Some(job) => inline.library.step(Some(job)),
                        // A cancelled pick finishes its import without a change.
                        None => inline.library.unchanged(),
                    };
                    crate::player_skin::files::clear_staged();
                    let _ = inline.outgoing.send(outcome);
                }
                job => {
                    let outcome = inline.library.step(Some(job));
                    crate::player_skin::files::clear_staged();
                    let _ = inline.outgoing.send(outcome);
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Library {
    /// Finishes a command that changed nothing.
    fn unchanged(&self) -> Outcome {
        let selected = self.view.selected_skin();
        Outcome::new(
            &self.view,
            self.view.active_skin(),
            selected.map(|entry| (entry.model, entry.engine_version.clone())),
            self.uuid,
            true,
        )
    }

    /// Stages picked files where the importer reads them and returns the import of the main one:
    /// a skin pack, else a PNG, whose geometry JSON beside it comes along; `None` when the player
    /// picked nothing.
    fn stage_picked(&mut self, cape: bool, files: Vec<(String, Vec<u8>)>) -> Option<Job> {
        let dir = self.layout.dressing_room_dir().join("picked");
        let mut main = None;
        for (name, bytes) in files {
            // Only the name's last component, so a picked name cannot point elsewhere.
            let Some(name) = std::path::Path::new(&name).file_name() else {
                continue;
            };
            let path = dir.join(name);
            let rank = match path.extension().and_then(|extension| extension.to_str()) {
                Some(extension) if extension.eq_ignore_ascii_case("mcpack") => 0,
                Some(extension) if extension.eq_ignore_ascii_case("png") => 1,
                _ => 2,
            };
            if main.as_ref().is_none_or(|(best, _)| rank < *best) {
                main = Some((rank, path.clone()));
            }
            crate::player_skin::files::stage(path, bytes);
        }
        let (_, path) = main?;
        Some(if cape {
            Job::ImportCape(Some(path))
        } else {
            Job::Import(Some(path))
        })
    }
}

/// The browser worker's library and channel ends.
#[cfg(target_arch = "wasm32")]
struct Inline {
    library: Library,
    incoming: crossbeam_channel::Receiver<Job>,
    outgoing: crossbeam_channel::Sender<Outcome>,
}

fn apply_job(layout: &InstallLayout, view: &mut DressingRoomView, job: Job) -> Result<(), String> {
    use crate::player_skin::catalog;
    match job {
        Job::Select(index) => catalog::select(layout, view, index),
        Job::Model(model) => catalog::set_model(layout, view, model),
        Job::Import(None) => picker::pick(false)
            .and_then(|path| path.map_or(Ok(()), |path| catalog::import(layout, view, &path))),
        Job::Import(Some(path)) => catalog::import(layout, view, &path),
        Job::Rename(index, name) => catalog::rename(layout, view, index, &name),
        Job::Delete(index) => catalog::delete(layout, view, index),
        Job::SelectCape(index) => catalog::select_cape(layout, view, index),
        Job::ImportCape(None) => picker::pick(true)
            .and_then(|path| path.map_or(Ok(()), |path| catalog::import_cape(layout, view, &path))),
        Job::ImportCape(Some(path)) => catalog::import_cape(layout, view, &path),
        Job::RenameCape(index, name) => catalog::rename_cape(layout, view, index, &name),
        Job::DeleteCape(index) => catalog::delete_cape(layout, view, index),
        #[cfg(target_arch = "wasm32")]
        Job::Picked { .. } => Ok(()),
    }
}

/// A session-scoped sender that returns packets when transport is full or closed.
type SkinPacketSender<'a> =
    dyn Fn(u64, protocol::Packet) -> Result<(), client_session::PacketSendError> + 'a;

impl MenuRuntime {
    pub(super) fn ensure_dressing_room(&mut self) {
        if self.dressing_room_worker.is_some() {
            return;
        }
        match Worker::start(self.layout.clone(), self.player_skin.clone()) {
            Ok(worker) => {
                self.dressing_room_worker = Some(worker);
                Arc::make_mut(&mut self.dressing_room).busy = true;
            }
            Err(error) => Arc::make_mut(&mut self.dressing_room).message = Some(error),
        }
    }

    fn queue_skin_job(&mut self, job: Job) {
        self.ensure_dressing_room();
        let Some(worker) = self.dressing_room_worker.as_mut() else {
            return;
        };
        match worker.commands.try_send(job) {
            Ok(()) => {
                worker.pending += 1;
                let view = Arc::make_mut(&mut self.dressing_room);
                view.busy = true;
                view.message = None;
            }
            Err(_) => {
                Arc::make_mut(&mut self.dressing_room).message =
                    Some("Please wait for the current skin change to finish.".to_owned())
            }
        }
    }

    pub(super) fn activate_dressing_room(&mut self, action: Action) {
        if self.dressing_room.busy && action != Action::Cancel {
            return;
        }
        match action {
            Action::SetSection(section) => {
                Arc::make_mut(&mut self.dressing_room).section = section;
                self.field = None;
                self.hovered = None;
                self.pressed = None;
                self.focused = 0;
            }
            Action::Select(index) => self.queue_skin_job(Job::Select(index)),
            Action::Import => self.queue_skin_job(Job::Import(None)),
            Action::ImportCape => self.queue_skin_job(Job::ImportCape(None)),
            Action::SelectCape(index) => self.queue_skin_job(Job::SelectCape(index)),
            Action::SetModel(model) => self.queue_skin_job(Job::Model(model)),
            Action::BeginRename(index) => self.begin_skin_editor(
                index,
                launcher::dressing_room::SkinEditorMode::Rename,
                launcher::dressing_room::SkinEditorTarget::Skin,
            ),
            Action::BeginDelete(index) => self.begin_skin_editor(
                index,
                launcher::dressing_room::SkinEditorMode::Delete,
                launcher::dressing_room::SkinEditorTarget::Skin,
            ),
            Action::BeginRenameCape(index) => self.begin_skin_editor(
                index,
                launcher::dressing_room::SkinEditorMode::Rename,
                launcher::dressing_room::SkinEditorTarget::Cape,
            ),
            Action::BeginDeleteCape(index) => self.begin_skin_editor(
                index,
                launcher::dressing_room::SkinEditorMode::Delete,
                launcher::dressing_room::SkinEditorTarget::Cape,
            ),
            Action::SaveRename => self.save_skin_name(),
            Action::ConfirmDelete => self.confirm_skin_delete(),
            Action::Cancel => self.cancel_skin_editor(),
        }
    }

    /// Uses the same skin importer as the Dressing Room file picker.
    #[cfg(any(test, feature = "developer-control"))]
    pub fn import_skin_path(&mut self, path: PathBuf) {
        if self.dressing_room.editor.is_some() {
            return;
        }
        self.queue_skin_job(Job::Import(Some(path)));
    }

    #[cfg(any(test, feature = "developer-control"))]
    pub fn import_cape_path(&mut self, path: PathBuf) {
        if self.dressing_room.editor.is_none() {
            self.queue_skin_job(Job::ImportCape(Some(path)));
        }
    }

    /// Publishes completed appearance work to local state and retries the current skin packet.
    pub fn poll_dressing_room(
        &mut self,
        local: Option<&mut crate::player_skin::LocalPlayerSkin>,
        stream: Option<&mut chunk_pipeline::WorldStream>,
        send: Option<&SkinPacketSender<'_>>,
        session: u64,
    ) {
        if let Some(worker) = self.dressing_room_worker.as_mut() {
            #[cfg(target_arch = "wasm32")]
            worker.run_inline();
            while let Ok(mut result) = worker.results.try_recv() {
                if result.completed_command {
                    worker.pending = worker.pending.saturating_sub(1);
                }
                result.view.busy = worker.pending != 0;
                result.view.section = self.dressing_room.section;
                if !result.completed_command {
                    result.view.editor = self.dressing_room.editor.clone();
                    result.view.message = self.dressing_room.message.clone();
                } else if result.view.message.is_some() {
                    result.view.editor = self.dressing_room.editor.clone();
                } else if self.dressing_room.editor.is_some() {
                    self.field = None;
                    self.focused = 0;
                }
                if result.changed
                    && let Some(selected) = result.view.selected_skin()
                {
                    if let Some(active) = result.view.active_skin() {
                        self.player_skin.set_selection(&active, selected.model);
                        self.player_skin.engine_version = selected.engine_version.clone();
                    }
                    self.skin_update_pending = true;
                    self.skin_outbound = None;
                    self.skin_packet_pending = result.packet;
                }
                self.dressing_room = Arc::new(result.view);
            }
        }
        if !self.skin_update_pending {
            self.flush_skin_packet(send);
            return;
        }
        if let Some(local) = local {
            *local = self.player_skin.clone();
        }
        let Some(stream) = stream else {
            self.skin_update_pending = self.is_connecting();
            if !self.skin_update_pending {
                self.skin_packet_pending = None;
            }
            return;
        };
        let authority = stream.authority();
        let uuid = authority
            .actor(authority.local_player_runtime_id())
            .and_then(|actor| match &actor.kind {
                protocol::ActorKind::Player { uuid, .. } => Some(*uuid),
                _ => None,
            })
            .unwrap_or(self.player_skin.local_uuid);
        if let Some(mut profile) = stream
            .authority()
            .actor_player_profile(stream.local_player_runtime_id())
            .cloned()
        {
            profile.skin = self.player_skin.player_skin();
            stream.update_local_player_skin(&profile);
        }
        if session == 0 {
            return;
        }
        let Some(mut packet) = self.skin_packet_pending.take() else {
            return;
        };
        protocol::set_skin_packet_uuid(&mut packet, uuid);
        self.skin_update_pending = false;
        self.skin_outbound = Some((session, packet));
        self.flush_skin_packet(send);
    }

    /// Retains a skin packet only while the current sender reports backpressure.
    fn flush_skin_packet(&mut self, send: Option<&SkinPacketSender<'_>>) {
        let Some(send) = send else { return };
        let Some((session, packet)) = self.skin_outbound.take() else {
            return;
        };
        match send(session, packet) {
            Ok(()) | Err(client_session::PacketSendError::Closed(_)) => {}
            Err(client_session::PacketSendError::Full(packet)) => {
                self.skin_outbound = Some((session, packet))
            }
        }
    }

    pub(super) fn dressing_room_focus(&self) -> Vec<MenuAction> {
        if let Some(editor) = &self.dressing_room.editor {
            if self.dressing_room.busy {
                return vec![MenuAction::DressingRoom(Action::Cancel)];
            }
            return match editor.mode {
                launcher::dressing_room::SkinEditorMode::Rename => {
                    let mut actions = vec![
                        MenuAction::DressingRoom(Action::Cancel),
                        MenuAction::EditSkinName,
                    ];
                    if !editor.draft.trim().is_empty() {
                        actions.push(MenuAction::DressingRoom(Action::SaveRename));
                    }
                    actions
                }
                launcher::dressing_room::SkinEditorMode::Delete => vec![
                    MenuAction::DressingRoom(Action::Cancel),
                    MenuAction::DressingRoom(Action::ConfirmDelete),
                ],
            };
        }
        let mut actions = vec![MenuAction::AddBack];
        if self.dressing_room.busy {
            return actions;
        }
        actions.extend(
            [
                launcher::dressing_room::DressingRoomSection::Skins,
                launcher::dressing_room::DressingRoomSection::Capes,
            ]
            .into_iter()
            .filter(|section| *section != self.dressing_room.section)
            .map(|section| MenuAction::DressingRoom(Action::SetSection(section))),
        );
        if self.dressing_room.section == launcher::dressing_room::DressingRoomSection::Capes {
            actions.extend([
                MenuAction::DressingRoom(Action::ImportCape),
                MenuAction::DressingRoom(Action::SelectCape(None)),
            ]);
            actions.extend(
                (0..self.dressing_room.capes.len())
                    .map(|index| MenuAction::DressingRoom(Action::SelectCape(Some(index)))),
            );
            if let Some(index) = self.dressing_room.selected_cape.filter(|index| {
                self.dressing_room
                    .capes
                    .get(*index)
                    .is_some_and(|entry| entry.imported)
            }) {
                actions.extend([
                    MenuAction::DressingRoom(Action::BeginRenameCape(index)),
                    MenuAction::DressingRoom(Action::BeginDeleteCape(index)),
                ]);
            }
            return actions;
        }
        actions.push(MenuAction::DressingRoom(Action::Import));
        actions.extend(
            (0..self.dressing_room.skins.len())
                .map(|index| MenuAction::DressingRoom(Action::Select(index))),
        );
        if self
            .dressing_room
            .selected_skin()
            .is_some_and(|skin| skin.imported)
        {
            actions.extend(
                [
                    launcher::dressing_room::SkinModel::Classic,
                    launcher::dressing_room::SkinModel::Slim,
                ]
                .into_iter()
                .filter(|model| {
                    self.dressing_room.selected_skin().is_some_and(|selected| {
                        selected.model != SkinModel::Custom && selected.model != *model
                    })
                })
                .map(|model| MenuAction::DressingRoom(Action::SetModel(model))),
            );
            if let Some(index) = self.dressing_room.selected {
                actions.extend([
                    MenuAction::DressingRoom(Action::BeginRename(index)),
                    MenuAction::DressingRoom(Action::BeginDelete(index)),
                ]);
            }
        }
        actions
    }
}
