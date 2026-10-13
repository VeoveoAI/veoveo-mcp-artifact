//! A share link's secret and the URL that carries it. The server returns the URL once
//! and keeps only the SHA-256 of the secret, so a stolen database names no live link.
use std::fmt;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest as _, Sha256};
use url::Url;
use veoveo_types::Sha256Digest;

use crate::contract::ShareLinkUrl;

/// The length of a secret's token: 32 bytes in unpadded base64url.
pub const SHARE_TOKEN_CHARS: usize = 43;

/// 256 random bits that name one share link.
pub struct ShareLinkSecret([u8; 32]);

impl ShareLinkSecret {
    /// A fresh secret from the operating system's random source.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// The secret a token spells: 43 characters of canonical unpadded base64url.
    /// Anything else names no link.
    pub fn parse(token: &str) -> Option<Self> {
        if token.len() != SHARE_TOKEN_CHARS {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD.decode(token).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }

    /// The token a share link's URL carries.
    pub fn token(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    /// The digest the server stores and looks the link up by.
    pub fn digest(&self) -> Sha256Digest {
        Sha256Digest::from_bytes(Sha256::digest(self.0).into())
    }
}

impl fmt::Debug for ShareLinkSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ShareLinkSecret(..)")
    }
}

/// The origin under which `/artifact/shares/{token}` reaches the server: an `http` or
/// `https` origin without a path, query or fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareBase(Url);

impl ShareBase {
    pub fn parse(origin: &str) -> Result<Self, &'static str> {
        let url = Url::parse(origin.trim_end_matches('/'))
            .map_err(|_| "the share link origin is no URL")?;
        let origin_only = matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none();
        if !origin_only {
            return Err(
                "the share link origin is an http or https origin without credentials, path, query or fragment",
            );
        }
        Ok(Self(url))
    }

    /// The host and port a browser names when it follows a link, which the server must
    /// accept as a Host header.
    pub fn authority(&self) -> String {
        match self.0.port() {
            Some(port) => format!("{}:{port}", self.0.host_str().unwrap_or_default()),
            None => self.0.host_str().unwrap_or_default().to_owned(),
        }
    }

    /// The URL of the link `secret` names.
    pub fn url(&self, secret: &ShareLinkSecret) -> ShareLinkUrl {
        let mut url = self.0.clone();
        url.path_segments_mut()
            .expect("an http origin has a path")
            .clear()
            .extend([crate::server().as_str(), "shares", &secret.token()]);
        ShareLinkUrl::parse(url.as_str()).expect("an origin and a token form a share link URL")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_round_trips_through_its_token_and_only_canonical_tokens_parse() {
        let secret = ShareLinkSecret::generate().unwrap();
        let token = secret.token();
        assert_eq!(token.len(), SHARE_TOKEN_CHARS);
        let parsed = ShareLinkSecret::parse(&token).unwrap();
        assert_eq!(parsed.digest(), secret.digest());
        assert!(ShareLinkSecret::parse(&token[1..]).is_none());
        assert!(ShareLinkSecret::parse(&format!("{token}A")).is_none());
        assert!(ShareLinkSecret::parse(&"!".repeat(SHARE_TOKEN_CHARS)).is_none());
        // The last character carries two unused bits; a token that sets them is not
        // canonical and names no secret.
        let mut bytes = token.into_bytes();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'B' { b'C' } else { b'B' };
        let noncanonical = String::from_utf8(bytes).unwrap();
        if let Some(other) = ShareLinkSecret::parse(&noncanonical) {
            assert_ne!(other.digest(), secret.digest());
        }
        assert_eq!(format!("{secret:?}"), "ShareLinkSecret(..)");
    }

    #[test]
    fn a_share_url_names_the_route_under_the_origin() {
        let base = ShareBase::parse("http://host.openshell.internal:8801/").unwrap();
        let secret = ShareLinkSecret::generate().unwrap();
        let url = base.url(&secret);
        assert_eq!(
            url.expose(),
            format!(
                "http://host.openshell.internal:8801/artifact/shares/{}",
                secret.token()
            )
        );
        assert_eq!(base.authority(), "host.openshell.internal:8801");
        assert!(!format!("{url:?}").contains(&secret.token()));
        for refused in [
            "ftp://example.test",
            "https://example.test/prefix",
            "https://user@example.test",
            "https://example.test/?q=1",
            "relative",
        ] {
            assert!(ShareBase::parse(refused).is_err(), "{refused}");
        }
    }
}
