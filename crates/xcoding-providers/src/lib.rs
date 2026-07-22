//! Cloud model provider contracts are introduced with the read-only agent loop.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderDescriptor {
    pub id: String,
    pub display_name: String,
}
