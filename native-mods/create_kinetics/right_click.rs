fn create_right_click_block(game: &mut crate::game::Game<'_>, pos: [i32; 3], canceled: &mut bool) {
    crate::game::create_kinetics::right_click(game, pos, canceled);
}
