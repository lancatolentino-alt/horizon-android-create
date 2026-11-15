fn create_block_entity(block: &str, entity: &mut Option<(String, horizon_world::nbt::Compound)>) {
    crate::game::create_kinetics::block_entity(block, entity);
}
