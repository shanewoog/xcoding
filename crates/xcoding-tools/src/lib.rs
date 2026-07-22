//! Tool contracts and implementations are introduced in Phase 1 and Phase 2.

/// The initial tool registry is intentionally empty. It reserves a dedicated
/// ownership boundary so the core dispatcher never needs to own OS tools directly.
#[derive(Default)]
pub struct ToolRegistry;
