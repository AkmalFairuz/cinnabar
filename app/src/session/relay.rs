//! Joins through a web relay, which dials the server itself, as a browser has no core of its own.

use super::{BlobCache, Commands, MenuRuntime, SessionController, SessionResources};

/// The relay a browser's sessions join through, and a join waiting to open on it.
#[derive(Debug, Default)]
pub(super) struct RelayLink {
    session: Option<bridge::RelaySession>,
    pub(super) pending_join: Option<String>,
}

impl SessionController {
    /// Joins every later session, such as a reconnect, through `relay` rather than a core.
    pub(crate) fn with_relay(mut self, relay: Option<bridge::RelaySession>) -> Self {
        self.relay.session = relay;
        self
    }
}

/// Holds a remote join for the relay, which dials the server itself, so the session opens on
/// the next poll; returns the address when the join goes through a core instead.
pub(super) fn defer_join(
    controller: &mut SessionController,
    menu: &mut MenuRuntime,
    address: String,
    local_world: bool,
) -> Option<String> {
    if controller.relay.session.is_none() || local_world {
        return Some(address);
    }
    controller.relay.pending_join = Some(address);
    controller.connecting = true;
    menu.show_connecting();
    None
}

/// Opens the session of a join waiting on the relay once the login ID is ready; returns whether
/// a relay join owned this poll, so the core join path is skipped.
pub(super) fn poll_join(
    commands: &mut Commands,
    menu: &mut MenuRuntime,
    session: &mut SessionResources<'_>,
    cache: &BlobCache,
) -> bool {
    let controller = &mut *session.controller;
    let (Some(address), Some(relay)) = (
        controller.relay.pending_join.take(),
        controller.relay.session.clone(),
    ) else {
        return false;
    };
    let client_random_id = match controller.poll_client_random_id() {
        Ok(Some(id)) => id,
        Ok(None) => {
            controller.relay.pending_join = Some(address);
            return true;
        }
        Err(message) => {
            controller.fail_join(menu, message);
            return true;
        }
    };
    let login_settings = super::login_settings(menu, session.input.as_deref());
    let endpoint = (
        bridge::CoreEndpoint::Relay(relay),
        Some(bridge::ConnectTarget::RakNet(address)),
    );
    if let Err(error) = super::start_network(
        commands,
        menu,
        controller.generation,
        cache,
        endpoint,
        login_settings,
        client_random_id,
        session.actor_artwork.as_deref().map(|artwork| &artwork.0),
        session.ui_catalog.as_deref(),
    ) {
        controller.fail_join(menu, format!("Could not connect: {error}"));
    }
    true
}
