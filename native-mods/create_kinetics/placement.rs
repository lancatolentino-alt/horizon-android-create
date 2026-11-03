fn create_state_for_placement(
    level: &crate::game::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    face: String,
    answer: &mut Option<Option<horizon_world::registry::BlockStateId>>,
) {
    crate::game::create_kinetics::placement(level, state, &face, answer);
}
