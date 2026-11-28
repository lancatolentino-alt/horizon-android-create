fn create_block_entity(block: &str, entity: &mut Option<horizon_world::nbt::Compound>) {
    crate::game::create_kinetics::block_entity(block, entity);
}
