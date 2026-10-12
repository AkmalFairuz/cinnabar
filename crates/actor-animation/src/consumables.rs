//! Consumption presentation derived from the canonical item-use catalog.

use assets::item_use::{ItemConsumption, builtin_consumption};

/// How a consumable is used: eaten food scatters crumbs, drinks do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsumeAnimation {
    Eat,
    Drink,
}

/// The use animation of the vanilla consumable `identifier`, or `None` for other items.
#[must_use]
pub fn consume_animation(identifier: &str) -> Option<ConsumeAnimation> {
    match builtin_consumption(identifier) {
        ItemConsumption::Eat => Some(ConsumeAnimation::Eat),
        ItemConsumption::Drink | ItemConsumption::Honey => Some(ConsumeAnimation::Drink),
        ItemConsumption::Unresolved | ItemConsumption::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assets::item_use::{ItemUseProfile, compile};

    #[test]
    fn animations_follow_the_canonical_item_use_catalog() {
        for descriptor in compile(&[]).iter() {
            let expected = match &descriptor.profile {
                ItemUseProfile::Provisional { consumption, .. } => match consumption {
                    ItemConsumption::Eat => Some(ConsumeAnimation::Eat),
                    ItemConsumption::Drink | ItemConsumption::Honey => {
                        Some(ConsumeAnimation::Drink)
                    }
                    ItemConsumption::Unresolved | ItemConsumption::None => None,
                },
                ItemUseProfile::Unresolved => None,
            };
            assert_eq!(
                consume_animation(&descriptor.identifier),
                expected,
                "{}",
                descriptor.identifier
            );
        }
        assert_eq!(consume_animation("fixture:unadmitted"), None);
    }

    #[test]
    fn honey_is_drunk_without_food_crumbs() {
        assert_eq!(
            consume_animation("minecraft:honey_bottle"),
            Some(ConsumeAnimation::Drink)
        );
    }
}
