fn create_block_entity_container(
    block: &str,
    container: &mut Option<horizon_client::containers::ModContainer>,
) -> ::core::ops::ControlFlow<()> {
    crate::game::create_kinetics::container(block, container);
    if container.is_some() {
        ::core::ops::ControlFlow::Break(())
    } else {
        ::core::ops::ControlFlow::Continue(())
    }
}
