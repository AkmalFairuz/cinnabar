//! The join's stage and pack download progress that a web relay reports for a browser's join,
//! which has no launcher core to report it.

use std::cell::Cell;

use bridge::ConnectProgress;

thread_local! {
    static RELAYED: Cell<Option<ConnectProgress>> = const { Cell::new(None) };
}

/// Records the relay's latest report, the JSON of its control socket's `connect` event. A
/// missing, empty or unreadable stage means no join is being prepared.
pub fn set_relayed(json: &str) {
    let progress = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .filter(|value| {
            value
                .get("stage")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|stage| !stage.is_empty())
        })
        .and_then(|value| serde_json::from_value(value).ok());
    RELAYED.set(progress);
}

/// The relay's latest report of the join being prepared, if any.
pub(crate) fn relayed() -> Option<ConnectProgress> {
    RELAYED.get()
}
