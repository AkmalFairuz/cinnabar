use std::sync::Arc;

use protocol::{
    NetworkItemStack, NormalInventoryChange, NormalInventorySource, Packet, StackRequestAction,
    StackResponse, StackResponseStatus, VerifiedNetworkItemStack,
    inventory_drop_transaction_packet, normal_inventory_transaction_packet,
};

use super::cells::Held;
use super::gesture::Submission;
use super::{
    Cell, InventoryGestureError, InventoryPendingState, PendingRequest, PlayerInventoryLedger,
};

impl PlayerInventoryLedger {
    /// Cancels unsent transactions whose exact source was authoritatively rewritten.
    pub(super) fn retire_legacy_write(&mut self, cell: Cell) {
        let client_authority = self.authority == Some(protocol::InventoryAuthority::Client);
        if self.confirmed.contains(cell)
            && self.queue.iter().any(|request| {
                request.state == InventoryPendingState::AwaitingTransport
                    && request.touches(cell)
                    && (client_authority || request.world_drop.is_some())
            })
        {
            self.abandon_requests(|request| {
                request.state == InventoryPendingState::AwaitingTransport
                    && request.touches(cell)
                    && (client_authority || request.world_drop.is_some())
            });
            let mut owned = Vec::new();
            for prediction in self
                .queue
                .iter_mut()
                .rev()
                .flat_map(|request| request.predicted.iter_mut().rev())
            {
                prediction.active = !owned.contains(&prediction.cell);
                if prediction.active {
                    owned.push(prediction.cell);
                }
            }
            self.refold();
        }
    }

    pub(super) fn set_authoritative_cell(&mut self, cell: Cell, held: Option<Held>) -> bool {
        self.retire_legacy_write(cell);
        self.committed_world_drops
            .retain(|request| !request.touches(cell));
        let written = self.confirmed.set(cell, held);
        if written {
            self.note_observed_gear(cell);
        }
        written
    }

    pub(super) fn validate_legacy_submission(
        &self,
        submission: &Submission,
    ) -> Result<(), InventoryGestureError> {
        if submission.actions.iter().any(|action| {
            !matches!(
                action,
                StackRequestAction::Take { .. }
                    | StackRequestAction::Place { .. }
                    | StackRequestAction::Swap { .. }
                    | StackRequestAction::Drop { .. }
            )
        }) {
            return Err(InventoryGestureError::LegacyActionUnavailable);
        }
        for cell in submission
            .groups
            .iter()
            .flat_map(super::overlay::DeltaGroup::touched)
        {
            self.legacy_cell_address(cell)?;
            verified(self.view().get(cell))?;
        }
        Ok(())
    }

    pub(super) fn legacy_pending_packet(&self) -> Result<Option<Packet>, InventoryGestureError> {
        let Some(request) = self.queue.iter().find(|request| {
            request.mining.is_none() && request.state == InventoryPendingState::AwaitingTransport
        }) else {
            return Ok(None);
        };
        normal_inventory_transaction_packet(self.legacy_changes(request, None)?)
            .map(Some)
            .map_err(|_| InventoryGestureError::InvalidRequest)
    }

    /// A drop key press as the native client sends it even to a server that owns the
    /// inventory: a normal transaction whose remainder carries the drop's legacy id.
    pub(super) fn world_drop_packet(
        &self,
        request: &PendingRequest,
    ) -> Result<Packet, InventoryGestureError> {
        let remainder = world_drop_remainder_id(request);
        let Some(Cell::Inventory(slot)) = request.predicted.first().map(|p| p.cell) else {
            return Err(InventoryGestureError::InvalidRequest);
        };
        let changes = self.legacy_changes(request, Some(remainder))?;
        inventory_drop_transaction_packet(slot, (remainder != 0).then_some(remainder), changes)
            .map_err(|_| InventoryGestureError::InvalidRequest)
    }

    /// The transaction legs of `request`: each predicted cell's old and new stack, its new
    /// stacks renamed `stamp` when given, and a world leg for each drop.
    fn legacy_changes(
        &self,
        request: &PendingRequest,
        stamp: Option<i32>,
    ) -> Result<Vec<NormalInventoryChange>, InventoryGestureError> {
        let mut changes = Vec::with_capacity(request.predicted.len() + request.actions.len());
        for prediction in &request.predicted {
            let (window_id, slot) = self.legacy_cell_address(prediction.cell)?;
            changes.push(NormalInventoryChange {
                source: NormalInventorySource::Container(window_id as i8),
                slot: u32::from(slot),
                from: descriptor(prediction.before.as_ref(), None)?,
                to: descriptor(prediction.held.as_ref(), stamp)?,
            });
        }
        for action in &request.actions {
            if let StackRequestAction::Drop {
                amount,
                source,
                randomly,
            } = action
            {
                let cell = self
                    .request_cell(*source)
                    .ok_or(InventoryGestureError::InvalidRequest)?;
                let mut dropped = request
                    .predicted
                    .iter()
                    .find(|prediction| prediction.cell == cell)
                    .and_then(|prediction| prediction.before.clone())
                    .ok_or(InventoryGestureError::InvalidRequest)?;
                dropped.stack.count = u16::from(*amount);
                changes.push(NormalInventoryChange {
                    source: NormalInventorySource::World {
                        randomly: *randomly,
                    },
                    slot: 0,
                    from: verified(None)?,
                    to: descriptor(Some(&dropped), None)?,
                });
            }
        }
        Ok(changes)
    }

    fn legacy_cell_address(&self, cell: Cell) -> Result<(i32, u8), InventoryGestureError> {
        Ok(match cell {
            Cell::Inventory(slot) => (protocol::PLAYER_INVENTORY_WINDOW_ID, slot),
            Cell::Cursor => (protocol::UI_INVENTORY_WINDOW_ID, 0),
            Cell::Armor(slot) => (protocol::ARMOR_WINDOW_ID, slot),
            Cell::Offhand => (protocol::OFFHAND_WINDOW_ID, 0),
            Cell::Craft(slot) => (protocol::UI_INVENTORY_WINDOW_ID, slot),
            Cell::Storage(slot) => (
                self.storage
                    .as_ref()
                    .ok_or(InventoryGestureError::InvalidRequest)?
                    .window_id,
                slot,
            ),
            Cell::CreatedOutput => return Err(InventoryGestureError::LegacyActionUnavailable),
        })
    }

    /// Commits a sent drop while keeping its optional legacy response separate from pending work.
    /// Its remainder and later requests use the transaction's legacy id.
    pub(super) fn commit_world_drop(&mut self, index: usize) -> bool {
        let mut request: PendingRequest = self.queue.remove(index).expect("index observed");
        let remainder = world_drop_remainder_id(&request);
        let current = self.request_is_current(&request);
        if current {
            for prediction in &mut request.predicted {
                if let Some(held) = &mut prediction.held {
                    held.stack.stack_network_id = remainder;
                }
                self.confirmed.set(prediction.cell, prediction.held.clone());
            }
        }
        self.rename_request_dependencies(request.request_id, remainder);
        if current && remainder != 0 {
            request.state = InventoryPendingState::AwaitingResponse;
            request.transport_deadline_millis = None;
            if self.committed_world_drops.len() >= super::MAX_PENDING_REQUESTS {
                self.committed_world_drops.pop_front();
            }
            self.committed_world_drops.push_back(request);
        }
        self.refold();
        self.finish_closing();
        true
    }

    /// Reconciles one optional drop response by its emitted even id, once, without delaying play.
    pub(super) fn apply_world_drop_response(&mut self, response: &StackResponse) -> bool {
        let Some(index) = self.committed_world_drops.iter().position(|request| {
            request
                .world_drop
                .is_some_and(|drop| drop.legacy_request_id == response.request_id)
        }) else {
            return false;
        };
        let mut request = self
            .committed_world_drops
            .remove(index)
            .expect("index observed");
        if !self.request_is_current(&request) {
            return true;
        }
        let backing: Vec<_> = request
            .predicted
            .iter()
            .map(|prediction| {
                (
                    prediction.cell,
                    self.confirmed.get(prediction.cell).cloned(),
                )
            })
            .collect();
        let owns_backing = request.predicted.iter().any(|prediction| {
            same_stack_name(
                self.confirmed.get(prediction.cell),
                prediction.held.as_ref(),
            )
        });
        let owns_history = request.predicted.iter().any(|source| {
            self.committed_world_drops
                .iter()
                .chain(self.queue.iter())
                .filter(|later| later.world_drop.is_some())
                .flat_map(|later| &later.predicted)
                .any(|prediction| {
                    prediction.cell == source.cell
                        && same_stack_name(prediction.before.as_ref(), source.held.as_ref())
                })
        });
        if !owns_backing && !owns_history {
            return true;
        }
        for prediction in &mut request.predicted {
            prediction.active = true;
        }
        match response.status {
            StackResponseStatus::Accepted => {
                // Reconcile against this drop's snapshot, even when a later sent drop
                // currently owns the backing cell. Restore that newer value below.
                for prediction in &request.predicted {
                    self.confirmed.set(prediction.cell, prediction.held.clone());
                }
                request.accepted = Some(Arc::clone(&response.containers));
                self.settle(&request);
            }
            StackResponseStatus::Rejected => {
                for prediction in &request.predicted {
                    self.confirmed
                        .set(prediction.cell, prediction.before.clone());
                }
            }
            StackResponseStatus::Unknown(_) => return true,
        }
        for (cell, held) in backing {
            let corrected = self.confirmed.get(cell).cloned();
            let source = request
                .predicted
                .iter()
                .find(|prediction| prediction.cell == cell)
                .expect("backing came from this prediction");
            self.rebase_world_drop_dependents(cell, source.held.as_ref(), corrected);
            if !same_stack_name(held.as_ref(), source.held.as_ref()) {
                self.confirmed.set(cell, held);
            }
        }
        true
    }

    /// Rebases drop rollback snapshots and rebuilds unsent descriptors along their source chain.
    fn rebase_world_drop_dependents(&mut self, cell: Cell, from: Option<&Held>, to: Option<Held>) {
        let server_authority = self.authority == Some(protocol::InventoryAuthority::Server);
        let mut replacements = vec![(from.cloned(), to)];
        let mut invalid = false;
        for request in self
            .committed_world_drops
            .iter_mut()
            .chain(self.queue.iter_mut())
            .filter(|request| request.world_drop.is_some())
        {
            let amount = request
                .actions
                .iter()
                .find_map(|action| match action {
                    StackRequestAction::Drop { amount, .. } => Some(u16::from(*amount)),
                    _ => None,
                })
                .expect("world drops have a drop action");
            for prediction in &mut request.predicted {
                if prediction.cell != cell {
                    continue;
                }
                let Some((_, replacement)) = replacements
                    .iter()
                    .rev()
                    .find(|(old, _)| same_stack_name(prediction.before.as_ref(), old.as_ref()))
                else {
                    continue;
                };
                prediction.before = replacement.clone();
                if request.state != InventoryPendingState::AwaitingTransport {
                    continue;
                }
                let Some(before) = prediction
                    .before
                    .as_ref()
                    .filter(|held| held.stack.count >= amount)
                else {
                    invalid = true;
                    continue;
                };
                let old = prediction.held.clone();
                prediction.held = consumed_stack(
                    Some(before),
                    amount,
                    server_authority.then_some(request.request_id),
                );
                let name = before.stack.stack_network_id;
                for action in &mut request.actions {
                    if let StackRequestAction::Drop { source, .. } = action {
                        source.stack_network_id = name;
                    }
                }
                for group in &mut request.groups {
                    if let super::overlay::DeltaGroup::Shrink { source_id, .. } = group {
                        *source_id = name;
                    }
                }
                replacements.push((old, prediction.held.clone()));
            }
        }
        if invalid {
            self.retire_legacy_write(cell);
        }
    }

    /// Publishes an admitted one-item use before exact drop descriptors are built or retried.
    /// The caller's slot revision and source must still match the backing inventory.
    pub fn settle_use_slot_change(
        &mut self,
        slot: u8,
        revision: u64,
        change: &protocol::PredictedSlotChange,
    ) -> bool {
        let cell = Cell::Inventory(slot);
        if self.authoritative_slot_revision(slot) != Some(revision)
            || !protocol::is_legacy_request_id(change.legacy_request_id)
            || self
                .queue
                .iter()
                .any(|request| request.world_drop.is_none() && request.touches(cell))
            || change.to != change.from.less_one(change.legacy_request_id)
        {
            return false;
        }
        let Some(before) = self.confirmed.get(cell).cloned() else {
            return false;
        };
        if verified(Some(&before)).ok().as_ref() != Some(&change.from) {
            return false;
        }
        let after = consumed_stack(Some(&before), 1, Some(change.to.stack_network_id()));
        for request in self
            .committed_world_drops
            .iter_mut()
            .filter(|request| request.touches(cell))
        {
            let drop = request.world_drop.as_mut().expect("committed world drop");
            drop.consumed_after = drop.consumed_after.saturating_add(1);
            for prediction in &mut request.predicted {
                if prediction.cell != cell {
                    continue;
                }
                prediction.before = consumed_stack(prediction.before.as_ref(), 1, None);
                let rename = same_stack_name(prediction.held.as_ref(), Some(&before))
                    .then_some(change.to.stack_network_id());
                prediction.held = consumed_stack(prediction.held.as_ref(), 1, rename);
            }
        }
        self.confirmed.set(cell, after.clone());
        self.rebase_world_drop_dependents(cell, Some(&before), after);
        self.refold();
        true
    }

    /// Sent normal transactions become local truth; later slot/content pushes correct it.
    pub(super) fn commit_legacy_transport(&mut self) -> bool {
        let Some(index) = self.queue.iter().position(|request| {
            request.mining.is_none() && request.state == InventoryPendingState::AwaitingTransport
        }) else {
            return false;
        };
        let request: PendingRequest = self.queue.remove(index).expect("index observed");
        if self.request_is_current(&request) {
            for prediction in request.predicted {
                self.confirmed.set(prediction.cell, prediction.held);
            }
        }
        self.refold();
        self.finish_closing();
        true
    }
}

/// A world drop's reserved legacy id, or `0` when the slot emptied.
fn world_drop_remainder_id(request: &PendingRequest) -> i32 {
    let remainder = request
        .predicted
        .first()
        .is_some_and(|prediction| prediction.held.is_some());
    if remainder {
        request
            .world_drop
            .expect("world drop identity")
            .legacy_request_id
    } else {
        0
    }
}

/// Whether two retained values carry the same stack name, including an empty result.
fn same_stack_name(left: Option<&Held>, right: Option<&Held>) -> bool {
    left.map(|held| held.stack.stack_network_id) == right.map(|held| held.stack.stack_network_id)
}

/// Removes admitted consumption from a snapshot, optionally assigning its surviving stack a name.
fn consumed_stack(held: Option<&Held>, amount: u16, name: Option<i32>) -> Option<Held> {
    let mut held = held?.clone();
    held.stack.count = held.stack.count.saturating_sub(amount);
    if held.stack.count == 0 {
        return None;
    }
    if let Some(name) = name {
        held.stack.stack_network_id = name;
    }
    Some(held)
}

/// `held` as a transaction leg named `name`, else by its own id, which may be a pending
/// request's or a sent drop's rather than a server stack id.
fn descriptor(
    held: Option<&Held>,
    name: Option<i32>,
) -> Result<VerifiedNetworkItemStack, InventoryGestureError> {
    let Some(held) = held else {
        return verified(None);
    };
    let name = name.unwrap_or(held.stack.stack_network_id);
    if name >= -1 && name == held.stack.stack_network_id {
        return verified(Some(held));
    }
    // Validate the retained item bytes before applying a pending request's client-side name.
    let mut unnamed = held.clone();
    unnamed.stack.stack_network_id = -1;
    Ok(verified(Some(&unnamed))?.renamed(name))
}

fn verified(held: Option<&Held>) -> Result<VerifiedNetworkItemStack, InventoryGestureError> {
    let stack = held.map_or_else(NetworkItemStack::empty, |held| held.stack.clone());
    let digest = stack.nbt_digest;
    VerifiedNetworkItemStack::try_new(stack, digest)
        .map_err(|_| InventoryGestureError::InvalidRequest)
}
