use protocol::{ContainerCloseEvent, NO_CONTAINER_WINDOW_TYPE};

use super::*;

/// Admits a screen drop and closes its window while retaining the pending reply.
fn screen_drop_and_close(ledger: &mut PlayerInventoryLedger) -> i32 {
    open_personal(ledger);
    let screen = ledger
        .begin_drop(DropSource::Target(InventoryTarget::Player(2)), Some(1))
        .unwrap();
    assert!(ledger.mark_transport_enqueued(20));
    ledger.request_personal_close();
    assert!(ledger.mark_transport_enqueued(21));
    ledger.apply(&InventoryEvent::Close(ContainerCloseEvent {
        container: ContainerIdentity::window(2),
        window_type: NO_CONTAINER_WINDOW_TYPE,
        server_initiated: false,
    }));
    assert!(!ledger.personal_inventory_desired_open());
    screen
}

#[test]
fn screen_then_world_rejections_restore_the_source_in_each_reply_order() {
    for screen_first in [false, true] {
        let original = digested(stack(6, 100, 5));
        let mut ledger = known_ledger(&[(2, original.clone())]);
        let screen = screen_drop_and_close(&mut ledger);
        let world = ledger.begin_world_drop(2, Some(1)).unwrap();
        let legacy = drop_legacy_id(&ledger, world);
        assert!(ledger.mark_transport_enqueued(22));
        assert_eq!(ledger.displayed_stack(2).unwrap().count, 3);
        let order = if screen_first {
            [screen, legacy]
        } else {
            [legacy, screen]
        };
        for id in order {
            ledger.apply(&response(id, StackResponseStatus::Rejected, None));
        }
        assert_eq!(ledger.displayed_stack(2), Some(&original));
        assert_eq!(ledger.pending_request_count(), 0);
        assert!(!ledger.resync_required());
        assert!(ledger.begin_world_drop(2, Some(1)).is_ok());
    }
}

#[test]
fn accepted_screen_corrections_survive_a_world_rejection_in_each_reply_order() {
    for screen_first in [false, true] {
        let mut ledger = known_ledger(&[(2, digested(stack(6, 100, 5)))]);
        let screen = screen_drop_and_close(&mut ledger);
        let world = ledger.begin_world_drop(2, Some(1)).unwrap();
        let legacy = drop_legacy_id(&ledger, world);
        assert!(ledger.mark_transport_enqueued(22));
        let accepted = response(screen, StackResponseStatus::Accepted, Some((3, 91)));
        let rejected = response(legacy, StackResponseStatus::Rejected, None);
        let order = if screen_first {
            [accepted, rejected]
        } else {
            [rejected, accepted]
        };
        for event in order {
            ledger.apply(&event);
        }
        let held = ledger.displayed_stack(2).unwrap();
        assert_eq!((held.count, held.stack_network_id), (3, 91));
        assert_eq!(ledger.pending_request_count(), 0);
        assert!(!ledger.resync_required());
    }
}

#[test]
fn rejecting_a_screen_predecessor_rebuilds_an_unsent_world_drop() {
    let mut ledger = known_ledger(&[(2, digested(stack(6, 100, 5)))]);
    let screen = screen_drop_and_close(&mut ledger);
    let world = ledger.begin_world_drop(2, Some(1)).unwrap();
    let legacy = drop_legacy_id(&ledger, world);
    ledger.apply(&response(screen, StackResponseStatus::Rejected, None));
    let (packet, count) = ledger.pending_batch().unwrap().unwrap();
    assert_eq!(count, 1);
    let transaction = observe_transaction(&packet).unwrap();
    assert_eq!(transaction.actions[0].from_item.count, 5);
    assert_eq!(transaction.actions[0].from_item.stack_network_id, Some(100));
    assert_eq!(transaction.actions[0].to_item.count, 4);
    assert!(ledger.mark_transport_enqueued(22));
    ledger.apply(&response(legacy, StackResponseStatus::Rejected, None));
    let held = ledger.displayed_stack(2).unwrap();
    assert_eq!((held.count, held.stack_network_id), (5, 100));
}

#[test]
fn world_then_screen_rejections_restore_the_source_in_each_reply_order() {
    for screen_first in [false, true] {
        let original = digested(stack(6, 100, 5));
        let mut ledger = known_ledger(&[(2, original.clone())]);
        let world = ledger.begin_world_drop(2, Some(1)).unwrap();
        let legacy = drop_legacy_id(&ledger, world);
        assert!(ledger.mark_transport_enqueued(0));
        open_personal(&mut ledger);
        let screen = ledger
            .begin_drop(DropSource::Target(InventoryTarget::Player(2)), Some(1))
            .unwrap();
        assert!(ledger.mark_transport_enqueued(20));
        let order = if screen_first {
            [screen, legacy]
        } else {
            [legacy, screen]
        };
        for id in order {
            ledger.apply(&response(id, StackResponseStatus::Rejected, None));
        }
        assert_eq!(ledger.displayed_stack(2), Some(&original));
        assert!(!ledger.resync_required());
        assert!(
            ledger
                .begin_drop(DropSource::Target(InventoryTarget::Player(2)), Some(1))
                .is_ok()
        );
    }
}

#[test]
fn a_world_reply_rebuilds_an_unsent_screen_successor() {
    for status in [StackResponseStatus::Accepted, StackResponseStatus::Rejected] {
        let mut ledger = known_ledger(&[(2, digested(stack(6, 100, 5)))]);
        let world = ledger.begin_world_drop(2, Some(1)).unwrap();
        let legacy = drop_legacy_id(&ledger, world);
        assert!(ledger.mark_transport_enqueued(0));
        open_personal(&mut ledger);
        let screen = ledger
            .begin_drop(DropSource::Target(InventoryTarget::Player(2)), Some(1))
            .unwrap();
        ledger.apply(&response(
            legacy,
            status,
            (status == StackResponseStatus::Accepted).then_some((3, 91)),
        ));
        let (count, name) = if status == StackResponseStatus::Accepted {
            (3, 91)
        } else {
            (5, 100)
        };
        assert_eq!(ledger.displayed_stack(2).unwrap().count, count - 1);
        assert!(ledger.pending_batch().unwrap().is_some());
        assert!(ledger.mark_transport_enqueued(20));
        ledger.apply(&response(screen, StackResponseStatus::Rejected, None));
        let held = ledger.displayed_stack(2).unwrap();
        assert_eq!((held.count, held.stack_network_id), (count, name));
        assert!(!ledger.resync_required());
    }
}

#[test]
fn an_empty_world_correction_cancels_its_unsent_successor_without_reviving_older_predictions() {
    let mut ledger = known_ledger(&[(2, digested(stack(6, 100, 5)))]);
    let screen = screen_drop_and_close(&mut ledger);
    let world = ledger.begin_world_drop(2, Some(1)).unwrap();
    let legacy = drop_legacy_id(&ledger, world);
    assert!(ledger.mark_transport_enqueued(22));
    ledger.begin_world_drop(2, Some(1)).unwrap();
    ledger.apply(&response(
        legacy,
        StackResponseStatus::Accepted,
        Some((0, 0)),
    ));
    assert!(ledger.pending_batch().unwrap().is_none());
    assert_eq!(ledger.slot_state(2), Some(PlayerInventorySlot::Empty));
    ledger.apply(&response(screen, StackResponseStatus::Rejected, None));
    assert_eq!(ledger.slot_state(2), Some(PlayerInventorySlot::Empty));
    assert!(!ledger.resync_required());
}

#[test]
fn interleaved_screen_and_world_rejections_restore_the_entire_chain() {
    for order in [[0, 1, 2, 3], [3, 2, 1, 0], [1, 2, 0, 3]] {
        let original = digested(stack(6, 100, 7));
        let mut ledger = known_ledger(&[(2, original.clone())]);
        open_personal(&mut ledger);
        let mut ids = Vec::new();
        for world in [false, true, false, true] {
            if world {
                let request = ledger.begin_world_drop(2, Some(1)).unwrap();
                ids.push(drop_legacy_id(&ledger, request));
            } else {
                ids.push(
                    ledger
                        .begin_drop(DropSource::Target(InventoryTarget::Player(2)), Some(1))
                        .unwrap(),
                );
            }
            assert!(ledger.mark_transport_enqueued(20));
        }
        for index in order {
            ledger.apply(&response(ids[index], StackResponseStatus::Rejected, None));
        }
        assert_eq!(ledger.displayed_stack(2), Some(&original));
        assert_eq!(ledger.pending_request_count(), 0);
        assert!(!ledger.resync_required());
    }
}
