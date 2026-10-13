//! Ordered rollback sources shared by screen drops and normal world transactions.

use protocol::{StackRequestAction, StackResponseStatus};

use super::cells::Held;
use super::{Cell, InventoryPendingState, PendingRequest, PlayerInventoryLedger};

impl PlayerInventoryLedger {
    /// Settles a drop against its own source, then rebases its remaining descendants.
    /// A newer admitted transaction keeps its backing value until its own reply.
    pub(super) fn reconcile_drop(
        &mut self,
        request: &mut PendingRequest,
        status: StackResponseStatus,
    ) -> bool {
        if request.drop_amount().is_none()
            || !self.request_is_current(request)
            || matches!(status, StackResponseStatus::Unknown(_))
        {
            return false;
        }
        let source = &request.predicted[0];
        let cell = source.cell;
        let backing = self.confirmed.get(cell).cloned();
        let owns_backing = same_stack_name(backing.as_ref(), source.held.as_ref());
        let newer_backing = self.committed_world_drops.iter().any(|later| {
            later.request_id < request.request_id
                && later.predicted.iter().any(|prediction| {
                    prediction.cell == cell
                        && same_stack_name(backing.as_ref(), prediction.held.as_ref())
                })
        });
        let owns_history = self
            .committed_world_drops
            .iter()
            .chain(self.queue.iter())
            .filter(|later| later.request_id < request.request_id && later.drop_amount().is_some())
            .any(|later| {
                let prediction = &later.predicted[0];
                prediction.cell == cell
                    && same_stack_name(prediction.before.as_ref(), source.held.as_ref())
            });
        if !owns_backing && !owns_history {
            return false;
        }
        let from = source.held.clone();
        match status {
            StackResponseStatus::Accepted => {
                // Screen requests have not committed their prediction to backing truth.
                // Unstated corrections retain that source rather than an orphaned odd id.
                let base = if request.world_drop.is_some() {
                    source.held.clone()
                } else {
                    source.before.clone()
                };
                self.confirmed.set(cell, base);
                request.predicted[0].active = true;
                self.settle(request);
            }
            StackResponseStatus::Rejected => {
                self.confirmed.set(cell, source.before.clone());
            }
            StackResponseStatus::Unknown(_) => unreachable!("unknown replies were excluded"),
        }
        let corrected = self.confirmed.get(cell).cloned();
        self.rebase_drop_dependents(cell, from.as_ref(), corrected);
        if !owns_backing && (request.world_drop.is_some() || newer_backing) {
            self.confirmed.set(cell, backing);
        }
        true
    }

    /// Rebases each later drop's rollback source and rebuilds descriptors not yet sent.
    pub(super) fn rebase_drop_dependents(
        &mut self,
        cell: Cell,
        from: Option<&Held>,
        to: Option<Held>,
    ) {
        let server_authority = self.authority == Some(protocol::InventoryAuthority::Server);
        let mut replacements = vec![(from.cloned(), to)];
        let mut invalid = Vec::new();
        // Committed world drops and outstanding screen requests can be interleaved.
        // Ledger request ids decrease, so this restores their original gesture order.
        let mut requests: Vec<_> = self
            .committed_world_drops
            .iter_mut()
            .chain(self.queue.iter_mut())
            .filter(|request| request.drop_amount().is_some())
            .collect();
        requests.sort_unstable_by(|left, right| right.request_id.cmp(&left.request_id));
        for request in requests {
            let amount = request.drop_amount().expect("single-cell drop");
            let prediction = &mut request.predicted[0];
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
                invalid.push(request.request_id);
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
        if !invalid.is_empty() {
            self.abandon_requests(|request| invalid.contains(&request.request_id));
        }
    }
}

/// Whether two retained values carry the same stack name, including an empty result.
pub(super) fn same_stack_name(left: Option<&Held>, right: Option<&Held>) -> bool {
    left.map(|held| held.stack.stack_network_id) == right.map(|held| held.stack.stack_network_id)
}

/// Removes admitted consumption from a snapshot, optionally renaming its surviving stack.
pub(super) fn consumed_stack(held: Option<&Held>, amount: u16, name: Option<i32>) -> Option<Held> {
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
