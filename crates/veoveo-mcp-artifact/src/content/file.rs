//! The answer of an artifact's bytes: RFC 9110's validators and one range, decided against its
//! size. An artifact is immutable once available, so its ID is its entity tag.
use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::stream;

use crate::{
    contract::{ArtifactId, MediaType, ObjectSize},
    storage::{ObjectKey, ObjectStorage},
};

/// One artifact's object and what its answer says of it.
pub(super) struct StoredFile<'a> {
    pub key: ObjectKey,
    pub artifact_id: ArtifactId,
    pub media_type: &'a MediaType,
    pub size: ObjectSize,
}

/// What a `Range` asks of an object of some size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asked {
    Whole,
    Range { first: u64, last: u64 },
    Unsatisfiable,
}

/// RFC 9110's one `bytes=first-last`, `bytes=first-` or `bytes=-suffix` against `size`. Anything
/// else, several ranges among them, is the whole object, as a server may answer.
fn asked(range: Option<&str>, size: u64) -> Asked {
    let Some(spec) = range.and_then(|range| range.strip_prefix("bytes=")) else {
        return Asked::Whole;
    };
    let Some((first, last)) = spec.split_once('-') else {
        return Asked::Whole;
    };
    let number = |text: &str| {
        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<u64>().ok())
            .flatten()
    };
    match (first, last) {
        ("", suffix) => match number(suffix) {
            None => Asked::Whole,
            Some(0) => Asked::Unsatisfiable,
            Some(_) if size == 0 => Asked::Unsatisfiable,
            Some(suffix) => Asked::Range {
                first: size.saturating_sub(suffix),
                last: size - 1,
            },
        },
        (first, "") => match number(first) {
            None => Asked::Whole,
            Some(first) if first >= size => Asked::Unsatisfiable,
            Some(first) => Asked::Range {
                first,
                last: size - 1,
            },
        },
        (first, last) => match (number(first), number(last)) {
            (Some(first), Some(last)) if first <= last => {
                if first >= size {
                    Asked::Unsatisfiable
                } else {
                    Asked::Range {
                        first,
                        last: last.min(size - 1),
                    }
                }
            }
            _ => Asked::Whole,
        },
    }
}

/// Whether `If-None-Match` holds `tag`, compared weakly as RFC 9110 says of it.
fn matches_tag(headers: &HeaderMap, tag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|value| value == "*" || value.strip_prefix("W/").unwrap_or(value) == tag)
}

/// The answer of `file` to a `method` request of `headers`.
pub(super) async fn answer(
    storage: &ObjectStorage,
    file: StoredFile<'_>,
    method: &Method,
    headers: &HeaderMap,
) -> Response {
    let size = file.size.bytes();
    let tag = format!("\"{}\"", file.artifact_id);
    if matches_tag(headers, &tag) {
        return with_identity(StatusCode::NOT_MODIFIED.into_response(), &tag, None);
    }
    // A range under an `If-Range` that names something else is the whole object.
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .filter(|_| {
            headers
                .get(header::IF_RANGE)
                .is_none_or(|value| value.as_bytes() == tag.as_bytes())
        });
    let asked = asked(range, size);
    if asked == Asked::Unsatisfiable {
        let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        if let Ok(value) = HeaderValue::from_str(&format!("bytes */{size}")) {
            response.headers_mut().insert(header::CONTENT_RANGE, value);
        }
        return with_identity(response, &tag, None);
    }
    let (status, length, content_range, storage_range) = match asked {
        Asked::Range { first, last } => (
            StatusCode::PARTIAL_CONTENT,
            last - first + 1,
            Some(format!("bytes {first}-{last}/{size}")),
            Some(format!("bytes={first}-{last}")),
        ),
        _ => (StatusCode::OK, size, None, None),
    };
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        match storage.read(&file.key, storage_range).await {
            Ok(bytes) => Body::from_stream(stream::try_unfold(bytes, |mut bytes| async move {
                Ok::<_, aws_sdk_s3::primitives::ByteStreamError>(
                    bytes.try_next().await?.map(|chunk| (chunk, bytes)),
                )
            })),
            Err(error) => {
                tracing::warn!(%error, "an artifact's object could not be read");
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        }
    };
    let mut response = (status, body).into_response();
    let answered = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(file.media_type.as_str()) {
        answered.insert(header::CONTENT_TYPE, value);
    }
    answered.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    with_identity(response, &tag, content_range)
}

fn with_identity(mut response: Response, tag: &str, range: Option<String>) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(value) = HeaderValue::from_str(tag) {
        headers.insert(header::ETAG, value);
    }
    if let Some(value) = range.and_then(|range| HeaderValue::from_str(&range).ok()) {
        headers.insert(header::CONTENT_RANGE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_range_is_decided_against_the_size() {
        let range = |first, last| Asked::Range { first, last };
        for (spec, want) in [
            ("bytes=0-3", range(0, 3)),
            ("bytes=2-", range(2, 9)),
            ("bytes=-3", range(7, 9)),
            ("bytes=-30", range(0, 9)),
            ("bytes=5-100", range(5, 9)),
            ("bytes=9-9", range(9, 9)),
            ("bytes=10-", Asked::Unsatisfiable),
            ("bytes=10-20", Asked::Unsatisfiable),
            ("bytes=-0", Asked::Unsatisfiable),
            // None of these is one range, so the whole object answers.
            ("bytes=3-2", Asked::Whole),
            ("bytes=0-1,3-4", Asked::Whole),
            ("bytes=", Asked::Whole),
            ("bytes=-", Asked::Whole),
            ("bytes=a-b", Asked::Whole),
            ("bytes=+1-2", Asked::Whole),
            ("items=0-1", Asked::Whole),
        ] {
            assert_eq!(asked(Some(spec), 10), want, "{spec}");
        }
        assert_eq!(asked(None, 10), Asked::Whole);
        assert_eq!(asked(Some("bytes=-3"), 0), Asked::Unsatisfiable);
        assert_eq!(asked(Some("bytes=0-"), 0), Asked::Unsatisfiable);
    }

    #[test]
    fn a_validator_matches_the_tag_whole_weak_or_in_a_list() {
        let holding = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static(value));
            headers
        };
        for value in ["\"a\"", "W/\"a\"", "\"b\", \"a\"", "*"] {
            assert!(matches_tag(&holding(value), "\"a\""), "{value}");
        }
        assert!(!matches_tag(&holding("\"b\""), "\"a\""));
        assert!(!matches_tag(&HeaderMap::new(), "\"a\""));
    }
}
