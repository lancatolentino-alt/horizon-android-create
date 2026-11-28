fn create_block_entity_types(
    value: Vec<horizon_mod_package::content::BlockEntityType>,
) -> Vec<horizon_mod_package::content::BlockEntityType> {
    crate::game::create_kinetics::block_entity_types(value)
}
