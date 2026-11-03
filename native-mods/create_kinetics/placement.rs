fn create_state_for_placement(
    level: &crate::game::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    context: &horizon_world::level::PlacementContext,
    answer: &mut Option<Option<horizon_world::registry::BlockStateId>>,
) {
    let face = format!("{:?}", context.face).to_lowercase();
    crate::game::create_kinetics::placement(level, state, &face, answer);
}
