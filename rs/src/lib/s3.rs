use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use rusoto_s3::{PutObjectRequest, S3};
use tokio_compat_02::FutureExt;

use crate::internal::*;

#[derive(Debug)]
pub struct S3Uploader<B, T>
where
    B: Into<Bytes>,
    T: Stream<Item = Result<B, std::io::Error>>,
{
    /// Object data.
    pub data: Option<T>,

    /// Target filename to upload to.
    pub filename: Option<String>,

    /// Target bucket to upload to.
    pub bucket: Option<String>,

    /// Size of the body in bytes. Since we're streaming, we must specify the size ahead of time.
    pub content_length: Option<i64>,

    /// The canned ACL to apply to the object.
    pub acl: Option<String>,
    /*
    // copied from S3PutObjectRequest
    /// Specifies caching behavior along the request/reply chain.
    pub cache_control: Option<String>,
    /// Specifies what content encodings have been applied to the object and thus what decoding mechanisms must be applied to obtain the media-type referenced by the Content-Type header field.
    pub content_encoding: Option<String>,
    /// The language the content is in.
    pub content_language: Option<String>,
    /// The base64-encoded 128-bit MD5 digest of the part data. This parameter is auto-populated when using the command from the CLI. This parameted is required if object lock parameters are specified.
    pub content_md5: Option<String>,
    /// A standard MIME type describing the format of the object data.
    pub content_type: Option<String>,
    /// The date and time at which the object is no longer cacheable.
    pub expires: Option<String>,
    /// Gives the grantee READ, READ_ACP, and WRITE_ACP permissions on the object.
    pub grant_full_control: Option<String>,
    /// Allows grantee to read the object data and its metadata.
    pub grant_read: Option<String>,
    /// A map of metadata to store with the object in S3.
    pub metadata: Option<::std::collections::HashMap<String, String>>,
    /// The type of storage to use for the object. Defaults to 'STANDARD'.
    pub storage_class: Option<String>,
    /// The tag-set for the object. The tag-set must be encoded as URL Query parameters. (For example, "Key1=Value1")
    pub tagging: Option<String>,
    */
}

/// Clones the static settings (bucket, acl) of the builder, but not the file details.
impl<B, T> Clone for S3Uploader<B, T>
where
    B: Into<Bytes>,
    T: Stream<Item = Result<B, std::io::Error>>,
{
    fn clone(&self) -> Self {
        Self {
            bucket: self.bucket.clone(),
            acl: self.acl.clone(),
            filename: None,
            data: None,
            content_length: None,
        }
    }
}

impl<B, T> Default for S3Uploader<B, T>
where
    B: Into<Bytes>,
    T: Stream<Item = Result<B, std::io::Error>>,
{
    fn default() -> Self {
        Self {
            filename: None,
            bucket: None,
            data: None,
            content_length: None,
            acl: None,
        }
    }
}

#[allow(dead_code)]
impl<B, T> S3Uploader<B, T>
where
    B: Into<Bytes>,
    T: Stream<Item = Result<B, std::io::Error>> + StreamExt + Send + Sync + 'static,
{
    pub fn bucket<K: ToString>(&mut self, bucket: K) -> &mut Self {
        self.bucket = Some(bucket.to_string());
        self
    }

    pub fn filename<F: ToString>(&mut self, filename: F) -> &mut Self {
        self.filename = Some(filename.to_string());
        self
    }

    pub fn data(&mut self, stream: T) -> &mut Self {
        self.data = Some(stream);
        self
    }

    pub fn content_length(&mut self, size: i64) -> &mut Self {
        self.content_length = Some(size);
        self
    }

    pub fn acl<F: ToString>(&mut self, acl: F) -> &mut Self {
        self.acl = Some(acl.to_string());
        self
    }

    pub fn build(self) -> Result<PutObjectRequest> {
        let body = rusoto_core::ByteStream::new(
            // compat must be external to any map, since a .map on a stream will expect the
            // newer tokio runtime and panic
            tokio_compat_02::IoCompat::new(
                self.data
                    .ok_or_else(|| format_err!("data is required but missing"))?,
            )
            .map(|b| b.map(Into::into)),
        );
        Ok(PutObjectRequest {
            key: self
                .filename
                .ok_or_else(|| format_err!("key is required but missing"))?,
            bucket: self
                .bucket
                .ok_or_else(|| format_err!("bucket is required but missing"))?,
            acl: self.acl,
            content_length: Some(
                self.content_length
                    .ok_or_else(|| format_err!("content_length is required but missing"))?,
            ),
            body: Some(body),
            ..PutObjectRequest::default()
        })
    }

    // alternatively, to upload directly.
    pub async fn upload<C: S3Put>(self, c: &C) -> Result<()> {
        if let Some(_) = option_env!("FAKE_S3") {
            warn!("faking an s3 upload");
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            debug!("fake upload complete");
            return Ok(());
        }

        Ok(c.upload_object(self.build()?)
            .compat() // XXX: remove with rusoto on tokio 0.3+
            .await?)
    }
}

#[async_trait]
pub trait S3Put: Send + Sync {
    async fn upload_object(&self, obj: PutObjectRequest) -> Result<()>;
}

#[async_trait]
impl<T> S3Put for T
where
    T: S3 + Send + Sync,
{
    async fn upload_object(&self, obj: PutObjectRequest) -> Result<()> {
        self.put_object(obj).compat().await.ah().map(|_| ()) // XXX: remove with rusoto on tokio 0.3+
    }
}
