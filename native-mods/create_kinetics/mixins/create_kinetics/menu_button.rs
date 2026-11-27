fn create_menu_button(
    game: &mut crate::game::Game<'_>,
    context: &mod_api::item::menu::MenuButtonContext,
    entity: &mut horizon_client::containers::ModBlockEntity,
) {
    crate::game::create_kinetics::menu_button(game, context.pos, entity, context.id);
}
