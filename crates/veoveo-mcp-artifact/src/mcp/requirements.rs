//! Registration and subscription admission use the same ready routes as dispatch.
use super::setup::ArtifactContract;
use rmcp::ErrorData;
use std::sync::LazyLock;
use veoveo_hub_sdk::{
    HubInternalIdentity,
    registration::{AuthorizationDeclaration, RequirementTarget, RouteAccess},
    server_contract::McpServerContract,
};
use veoveo_types::ResourceUri;
pub static AUTHORIZATION: LazyLock<AuthorizationDeclaration> = LazyLock::new(|| {
    let declaration = super::ROUTES.authorization();
    declaration
        .check(&crate::server(), &ArtifactContract::scheme())
        .expect("Artifact route authorization");
    declaration
});
pub(crate) fn authorize(identity: &HubInternalIdentity, uri: &str) -> Result<(), ErrorData> {
    let undeclared =
        || ErrorData::invalid_request("the resource is not a declared Artifact route", None);
    let uri = ResourceUri::new(uri).map_err(|_| undeclared())?;
    match AUTHORIZATION
        .access(RequirementTarget::Resource(&uri))
        .ok_or_else(undeclared)?
    {
        RouteAccess::Authenticated => Ok(()),
        RouteAccess::Permission(permission) if identity.grants(permission) => Ok(()),
        RouteAccess::Permission(permission) => Err(ErrorData::invalid_request(
            format!("required route permission `{permission}` was not granted"),
            None,
        )),
    }
}
