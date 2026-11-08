fn create_mouse_pressed(screen: &crate::game::native_hooks::NativeScreenView<'_>, button: i32) {
    crate::game::create_kinetics::mouse_pressed(screen, button);
}
