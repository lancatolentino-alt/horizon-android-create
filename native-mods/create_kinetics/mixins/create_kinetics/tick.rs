fn create_block_entity_tick(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    entity: &mut ::mod_api::ModBlockEntity,
) {
    crate::game::create_kinetics::tick(game, pos, entity);
}
