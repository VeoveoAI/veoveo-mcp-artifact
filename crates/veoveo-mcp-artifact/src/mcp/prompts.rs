//! Artifact's prompts: guidance for reviewing how an artifact is shared and for deciding
//! the access requests waiting on the caller. Both read through resources and act through
//! tools, so a prompt grants nothing itself.
use rmcp::{
    ErrorData as McpError,
    model::{GetPromptResult, JsonObject, Prompt, PromptArgument, PromptMessage, Role},
};
use serde::Deserialize;
use serde_json::Value;

use crate::contract::{AccessRequestRole, AccessRequestState, ArtifactId, ArtifactResource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactPrompt {
    ReviewArtifactSharing,
    ReviewAccessRequests,
}

impl ArtifactPrompt {
    pub const ALL: [Self; 2] = [Self::ReviewArtifactSharing, Self::ReviewAccessRequests];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ReviewArtifactSharing => "review-artifact-sharing",
            Self::ReviewAccessRequests => "review-access-requests",
        }
    }

    pub fn definition(self) -> Prompt {
        let (title, description, arguments) = match self {
            Self::ReviewArtifactSharing => (
                "Review artifact sharing",
                "Review who can reach an artifact you administer: its grants, release state and share links.",
                vec![
                    PromptArgument::new("artifact_id")
                        .with_description("The artifact to review.")
                        .with_required(true),
                ],
            ),
            Self::ReviewAccessRequests => (
                "Review access requests",
                "Decide the pending access requests on artifacts you administer.",
                Vec::new(),
            ),
        };
        Prompt::new(self.name(), Some(description), Some(arguments)).with_title(title)
    }

    pub fn render(self, arguments: Option<JsonObject>) -> Result<GetPromptResult, McpError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Arguments {
            artifact_id: Option<String>,
        }
        let arguments: Arguments =
            serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
                .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        let text = match self {
            Self::ReviewArtifactSharing => {
                let artifact_id = arguments
                    .artifact_id
                    .as_deref()
                    .ok_or_else(|| missing("artifact_id"))?
                    .parse::<ArtifactId>()
                    .map_err(|_| {
                        McpError::invalid_params("`artifact_id` is no artifact identifier", None)
                    })?;
                let artifact = ArtifactResource::Artifact(artifact_id);
                let grants = ArtifactResource::Grants(artifact_id);
                let links = ArtifactResource::ShareLinks { artifact_id };
                format!(
                    "Read {artifact} for the artifact's owner, Work Context, labels and release \
                     state, {grants} for its grants and {links} for its share links. Report who \
                     can reach it and through which mechanism. Flag grants without an expiry \
                     to groups, share links that live longer than the artifact needs, links \
                     close to their redemption limit, and a released artifact nobody shares by \
                     link. Propose `revoke_artifact_grant`, `revoke_share_link` or \
                     `set_release_state` to `private` where access exceeds the artifact's \
                     purpose, and call none of them until the user agrees."
                )
            }
            Self::ReviewAccessRequests => {
                let pending = ArtifactResource::AccessRequests {
                    role: Some(AccessRequestRole::Decider),
                    state: Some(AccessRequestState::Pending),
                };
                format!(
                    "Read {pending} for the pending requests you can decide, following \
                     `nextCursor` to the end. For each one, read the artifact it names and its \
                     grants, then weigh the requested level against the requester's \
                     justification and the artifact's labels. Present each request with a \
                     recommendation, and call `decide_access_request` with `approve` or `deny` \
                     and a note only for the requests the user decides."
                )
            }
        };
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            text,
        )]))
    }
}

fn missing(name: &str) -> McpError {
    McpError::invalid_params(format!("missing prompt argument `{name}`"), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sharing_review_names_the_artifact_resources() {
        let artifact = ArtifactId::new();
        let mut arguments = JsonObject::new();
        arguments.insert("artifact_id".into(), Value::String(artifact.to_string()));
        let rendered = ArtifactPrompt::ReviewArtifactSharing
            .render(Some(arguments))
            .unwrap();
        let text = serde_json::to_string(&rendered).unwrap();
        assert!(text.contains(&format!("artifact://artifacts/{artifact}/share-links")));
        assert!(ArtifactPrompt::ReviewArtifactSharing.render(None).is_err());
        let requests = ArtifactPrompt::ReviewAccessRequests.render(None).unwrap();
        let text = serde_json::to_string(&requests).unwrap();
        assert!(
            text.contains("artifact://access-requests?role=decider&state=pending"),
            "{text}"
        );
    }
}
