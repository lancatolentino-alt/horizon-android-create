fn create_block_entity_bounds(
    pos: [i32; 3],
    block: &str,
    entity: &::mod_api::ModBlockEntity,
    bounds: &mut [f32; 6],
) {
    crate::game::create_kinetics::bounds(pos, block, entity, bounds);
}
