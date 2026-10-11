use super::*;

// Java crouching raised the first-person camera off the eye, which read as third person.
#[test]
fn first_person_bites_follow_a_raised_camera() {
    let stream = super::totem_tests::stream();
    let eye = bevy::math::Vec3::new(0.0, protocol::PLAYER_NETWORK_OFFSET, 0.0);
    let camera = Transform::from_translation(eye + bevy::math::Vec3::Y * 0.27);
    let first = local_bite_position(&stream, &camera, true).unwrap();
    let expected = camera.translation + camera.forward() * FIRST_PERSON_BITE_AHEAD
        - camera.up() * FIRST_PERSON_BITE_BELOW;
    assert!(first.distance(expected) < 1e-5, "{first:?}");
    let mouth = stream.authority().actor(1).unwrap().eating_position();
    assert_eq!(
        local_bite_position(&stream, &camera, false),
        Some(bevy::math::Vec3::from_array(mouth))
    );
}
