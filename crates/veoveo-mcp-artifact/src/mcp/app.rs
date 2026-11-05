//! Artifact's App, `ui://artifact/library`: a gallery of the artifacts the person may read, each
//! opening through the Hub's `/view/` page. It reads Artifact's own listing, and calls one tool,
//! `thumbnail`, for each artifact's picture.
use rmcp::model::Icon;
use veoveo_hub_sdk::{
    apps::AppUri,
    hosting::{AppBuilder, RoutePolicy, RoutesBuilder},
};

use super::ArtifactMcp;
use crate::contract::ArtifactPermission;

/// The App's address.
pub const APP_URI: &str = "ui://artifact/library";
/// The App's programmatic resource name.
pub const APP_NAME: &str = "library";
pub const APP_TITLE: &str = "Artifacts";
pub const APP_DESCRIPTION: &str = "A gallery of the artifacts you may read, newest first, each with a picture and opening in its viewer on the content domain.";
/// A stack of files, as an SVG `data:` URI.
pub const APP_ICON: &str = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjNmI3MjgwIiBzdHJva2Utd2lkdGg9IjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIgc3Ryb2tlLWxpbmVqb2luPSJyb3VuZCI+PHBhdGggZD0iTTMgN2wxMC00IDggNC0xMCA0eiIvPjxwYXRoIGQ9Ik0zIDEybDggNCAxMC00Ii8+PHBhdGggZD0iTTMgMTdsOCA0IDEwLTQiLz48L3N2Zz4=";
pub const APP_ICON_MIME_TYPE: &str = "image/svg+xml";

/// What the App adds to the gallery.
const ADAPTER: &str = include_str!("../../assets/library.js");

/// The App's document.
pub fn document() -> std::io::Result<veoveo_hub_sdk::apps::AppHtml> {
    veoveo_hub_sdk::apps::AppHtml::load(&veoveo_hub_ui::gallery_file(APP_TITLE, ADAPTER)?)
}

pub fn uri() -> AppUri {
    AppUri::parse(APP_URI).expect("Artifact's App address")
}

/// Declare the App, admitted by `artifact.artifacts.read` as Artifact's listing is.
pub(super) fn register(routes: RoutesBuilder<ArtifactMcp>) -> RoutesBuilder<ArtifactMcp> {
    routes.app(
        AppBuilder::new(uri(), APP_NAME)
            .title(APP_TITLE)
            .description(APP_DESCRIPTION)
            .icon(Icon::new(APP_ICON).with_mime_type(APP_ICON_MIME_TYPE))
            .policy(
                RoutePolicy::permission(ArtifactPermission::ArtifactsRead)
                    .expect("Artifact permission"),
            )
            .reader(|domain: ArtifactMcp, _| async move { Ok(domain.app.library.clone()) }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_opens_files_only_through_the_hubs_page_and_calls_only_for_pictures() {
        assert!(ADAPTER.contains("`/view/artifact/${artifact.artifactId}/"));
        assert!(ADAPTER.contains("ui.read(address(search, cursor))"));
        assert_eq!(
            ADAPTER.matches("ui.tool(").count(),
            1,
            "the one tool it calls"
        );
        assert!(ADAPTER.contains(r#"ui.tool("thumbnail", { artifactId: item.key"#));
        assert!(
            document()
                .unwrap()
                .as_str()
                .contains("<title>Artifacts</title>")
        );
        assert!(uri().server().as_str() == "artifact");
        assert!(APP_ICON.starts_with("data:image/svg+xml;base64,"));
    }
}
