use std::fmt::{Formatter, Debug, Result as FResult};

#[allow(unused_imports)]
pub(crate) use anyhow::*;
pub(crate) use log::{debug, error, info, trace, warn};

pub use rusoto_s3::S3Client;
pub use reqwest::{Client as HTTPClient};


pub(crate) trait Anyhow<T, E: std::error::Error> {
    fn ah(self) -> Result<T>;
}

impl<T, E: std::error::Error> Anyhow<T, E> for std::result::Result<T, E> {
    fn ah(self) -> Result<T> {
        self.map_err(|e| format_err!("{}", e))
    }
}

// vreddit clients injected into each job executor
#[derive(Clone)]
pub(crate) struct Clients {
    pub(crate) s3_client: S3Client,
    pub(crate) http_client: HTTPClient,
}

impl Default for Clients {
    fn default() -> Self {
        Self {
            s3_client: rusoto_s3::S3Client::new(Default::default()),
            http_client: reqwest::Client::default(),
        }
    }
}

impl Debug for Clients {
    fn fmt(&self, f: &mut Formatter<'_>) -> FResult {
        f.debug_struct("VRClients")
            // .field("s3_client", "<unformattable>")
            .field("http_client", &self.http_client)
            .finish()
    }
}

impl AsRef<Clients> for Clients {
    fn as_ref(&self) -> &Self {
        &self
    }
}
