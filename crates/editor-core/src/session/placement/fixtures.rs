use super::*;

pub fn fixture() -> (Session, ObjectId) {
    let mut session = Session::new(Err("No assets needed for brush placement".into()));
    for (low, high) in [
        ([-256.0, -256.0, -16.0], [256.0, 256.0, 0.0]),
        ([-256.0, -256.0, 128.0], [256.0, 256.0, 144.0]),
        ([-256.0, 64.0, 0.0], [256.0, 80.0, 128.0]),
    ] {
        session
            .create_block(
                Actor::User,
                DVec3::from_array(low),
                DVec3::from_array(high),
                "brick/test",
                false,
            )
            .unwrap();
    }
    let selected = session
        .create_block(
            Actor::User,
            DVec3::new(-8.0, -8.0, 32.0),
            DVec3::new(8.0, 8.0, 48.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .select(&[selected], crate::session::SelectMode::Replace)
        .unwrap();
    (session, selected)
}
