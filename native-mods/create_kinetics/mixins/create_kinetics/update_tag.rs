fn create_block_entity_update_tag(
    entity: &::mod_api::ModBlockEntity,
    tag: &mut Option<horizon_world::nbt::Compound>,
) {
    crate::game::create_kinetics::update_tag(entity, tag);
}
