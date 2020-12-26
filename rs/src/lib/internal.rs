pub(crate) use anyhow::*;
#[allow(unused_imports)]
pub(crate) use log::{debug, error, info, trace, warn};


pub(crate) trait Anyhow<T, E: std::error::Error> {
    fn ah(self) -> Result<T>;
}

impl<T, E: std::error::Error> Anyhow<T, E> for std::result::Result<T, E> {
    fn ah(self) -> Result<T> {
        self.map_err(|e| format_err!("{}", e))
    }
}
