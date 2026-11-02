//! Embeds the skills under `skills/`, which the server serves at `skill://artifact/…`.
fn main() {
    veoveo_skills::build::embed_skills("skills").expect("Artifact's skills");
}
