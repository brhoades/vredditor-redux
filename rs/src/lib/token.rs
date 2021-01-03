use std::collections::HashSet;
use std::env::var_os;
use std::sync::Arc;

use crate::internal::*;
use crate::proto::Token;

/// Authorizer is a singleton which is cloned for each request.
pub trait Authorizer: Clone + Send + Sync {
    fn is_authorized(&self, t: &Token) -> bool;
}

/// EnvAuthorizer just reads VREDDITOR_VALID_KEYS, splits on ',', and
/// compares against those on call.
#[derive(Debug, Clone)]
pub(crate) struct EnvAuthorizer {
    authorized_tokens: Arc<HashSet<Vec<u8>>>,
}

impl EnvAuthorizer {
    // new_from_env creates a new EnvAuthorizor from the env variable key. It errors if
    // the key is absent or any of the keys fail to parse.
    pub(crate) fn new_from_env<S: AsRef<std::ffi::OsStr>>(env_name: S) -> Result<Self> {
        let env_name = env_name.as_ref();
        let keys = var_os(env_name)
            .map(std::ffi::OsString::into_string)
            .transpose()
            .map_err(|orig_str| format_err!("unable to convert '{:?}' into utf8 string", orig_str))?
            .ok_or_else(|| {
                format_err!(
                    "expected {} to contain authorized keys. it contains nothing.",
                    env_name.to_str().unwrap_or("[could not read env key name]")
                )
            })?;

        let new = Self {
            authorized_tokens: Arc::new(
                keys.split(",")
                    .into_iter()
                    .map(base64::decode)
                    .collect::<Result<HashSet<_>, _>>()
                    .context("one or more tokens failed to decode")?,
            ),
        };

        trace!("authorized tokens: {:?}", new);

        if new.authorized_tokens.len() > 0 {
            Ok(new)
        } else {
            Err(format_err!(
                "expected at least one authorized key in {} but there are 0.",
                env_name.to_str().unwrap_or("[could not read env key name]")
            ))
        }
    }
}

impl Authorizer for EnvAuthorizer {
    fn is_authorized(&self, token: &Token) -> bool {
        (&self).is_authorized(token)
    }
}

impl Authorizer for &EnvAuthorizer {
    fn is_authorized(&self, token: &Token) -> bool {
        match token {
            Token::V1(ref bytes) => self.authorized_tokens.contains(bytes),
        }
    }
}
