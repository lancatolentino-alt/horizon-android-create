fn create_mouse_released(screen: &crate::game::native_hooks::NativeScreenView<'_>, button: i32) {
    crate::game::create_kinetics::mouse_released(screen, button);
}
