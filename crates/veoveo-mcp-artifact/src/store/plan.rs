//! How an upload receives its bytes, chosen from what the caller declared and the
//! tenant's limits.
use std::time::Duration;

use crate::contract::{
    CreateUploadRequest, ObjectSize, PartNumber, PartSize, UploadForm, UploadLimits,
};

use super::ArtifactStoreError;

/// The largest object one signed PutObject receives: 100 MiB.
pub const SINGLE_PUT_MAX_BYTES: u64 = 100 * 1024 * 1024;
/// The part size of a multipart upload whose bound fits 10,000 parts of it: 16 MiB.
pub const DEFAULT_PART_BYTES: u64 = PartSize::DEFAULT;
/// How long an upload stays open: 24 hours.
pub const UPLOAD_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

const MIB: u64 = 1024 * 1024;

/// An upload's form, the bytes it reserves against the quota, and its part size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadPlan {
    pub form: UploadForm,
    pub reserved: ObjectSize,
    pub part_size: Option<PartSize>,
}

impl UploadPlan {
    /// A known size up to [`SINGLE_PUT_MAX_BYTES`] takes one PutObject. A larger or
    /// unknown size takes a multipart upload that reserves the declared size, the
    /// declared maximum, or the tenant's largest object, in that order.
    pub fn new(
        request: &CreateUploadRequest,
        limits: &UploadLimits,
    ) -> Result<Self, ArtifactStoreError> {
        let largest = limits.largest_object;
        let bound = request.size.or(request.max_size).unwrap_or(largest);
        if bound > largest {
            return Err(ArtifactStoreError::TooLarge {
                largest: largest.bytes(),
            });
        }
        if request
            .size
            .is_some_and(|size| size.bytes() <= SINGLE_PUT_MAX_BYTES)
        {
            return Ok(Self {
                form: UploadForm::Single,
                reserved: bound,
                part_size: None,
            });
        }
        let parts = u64::from(PartNumber::MAX);
        let fitted = bound.bytes().div_ceil(parts).div_ceil(MIB) * MIB;
        let part_size = PartSize::new(fitted.max(DEFAULT_PART_BYTES))
            .expect("5 TiB over 10,000 parts fits the part bounds");
        Ok(Self {
            form: UploadForm::Multipart,
            reserved: bound,
            part_size: Some(part_size),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::contract::{ArtifactName, MAX_OBJECT_BYTES, MediaType};

    use super::*;

    fn request(size: Option<u64>, max_size: Option<u64>) -> CreateUploadRequest {
        CreateUploadRequest {
            name: ArtifactName::parse("a.bin").unwrap(),
            media_type: MediaType::parse("application/octet-stream").unwrap(),
            size: size.map(|bytes| ObjectSize::new(bytes).unwrap()),
            sha256: None,
            max_size: max_size.map(|bytes| ObjectSize::new(bytes).unwrap()),
            labels: Default::default(),
            retention_days: None,
            metadata: Default::default(),
            derived_from: Default::default(),
            idempotency_key: None,
        }
    }

    #[test]
    fn small_known_sizes_take_one_put() {
        let plan = UploadPlan::new(&request(Some(10), None), &UploadLimits::default()).unwrap();
        assert_eq!(plan.form, UploadForm::Single);
        assert_eq!(plan.reserved.bytes(), 10);
        assert!(plan.part_size.is_none());
    }

    #[test]
    fn large_and_unknown_sizes_take_parts_that_fit_ten_thousand() {
        let limits = UploadLimits::default();
        let known =
            UploadPlan::new(&request(Some(SINGLE_PUT_MAX_BYTES + 1), None), &limits).unwrap();
        assert_eq!(known.form, UploadForm::Multipart);
        assert_eq!(known.part_size.unwrap().bytes(), DEFAULT_PART_BYTES);
        let unknown = UploadPlan::new(&request(None, None), &limits).unwrap();
        assert_eq!(unknown.reserved.bytes(), MAX_OBJECT_BYTES);
        let part = unknown.part_size.unwrap().bytes();
        assert!(part * u64::from(PartNumber::MAX) >= MAX_OBJECT_BYTES);
        assert_eq!(part % MIB, 0);
    }

    #[test]
    fn a_bound_above_the_largest_object_is_refused() {
        let limits = UploadLimits {
            largest_object: ObjectSize::new(1000).unwrap(),
            ..UploadLimits::default()
        };
        assert!(matches!(
            UploadPlan::new(&request(None, Some(1001)), &limits),
            Err(ArtifactStoreError::TooLarge { largest: 1000 })
        ));
        assert_eq!(
            UploadPlan::new(&request(None, None), &limits)
                .unwrap()
                .reserved
                .bytes(),
            1000
        );
    }
}
