fn create_menu_button(
    game: &mut crate::game::Game<'_>,
    context: &mod_api::item::menu::MenuButtonContext,
    entity: &mut ::mod_api::ModBlockEntity,
) {
    crate::game::create_kinetics::menu_button(game, context.pos, entity, context.id);
}
