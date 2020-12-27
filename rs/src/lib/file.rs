use std::{env::temp_dir, fs::remove_file, iter, path::PathBuf};

use rand::{distributions, thread_rng, Rng};
use tokio::fs::File;

use crate::internal::*;

// returns a named tempfile which deletes itself when it falls out of scope.
// this still doesn't handle deletion on crashes, but it does handle
// happy path cases
#[derive(Debug)]
pub struct GuardedTempFile {
    name: PathBuf,
    namestr: String,
    f: File,
}

impl GuardedTempFile {
    pub fn new() -> Result<Self> {
        let name = temp_dir().join(random_id());

        let namestr = name.clone();
        let namestr = namestr
            .to_str()
            .ok_or_else(|| {
                format_err!(
                    "failed to convert random file pathbuf to string: {:?}",
                    name
                )
            })?
            .to_string();

        Ok(Self {
            f: File::from_std(
                std::fs::File::create(&name).context("creating temporary named file")?,
            ),
            namestr,
            name,
        })
    }

    pub fn filename(&self) -> &str {
        self.namestr.as_str()
    }

    pub fn path(&self) -> &PathBuf {
        &self.name
    }

    pub fn file(&mut self) -> &File {
        &self.f
    }

    pub fn file_mut(&mut self) -> &mut File {
        &mut self.f
    }

    pub async fn metadata(&mut self) -> Result<std::fs::Metadata> {
        self.f.metadata().await.ah()
    }

    pub async fn len(&mut self) -> Result<u64> {
        self.f.metadata().await.ah().map(|f| f.len())
    }
}

impl Drop for GuardedTempFile {
    fn drop(&mut self) {
        let err = remove_file(self.name.clone());

        trace!(
            "deleted on drop: {} ({})",
            self.namestr,
            err.err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "OK".to_owned())
        );
    }
}

pub fn random_id() -> String {
    random_string(7)
}
pub fn random_string(len: usize) -> String {
    let mut rng = thread_rng();
    iter::repeat(())
        .map(|()| rng.sample(distributions::Alphanumeric))
        .take(len)
        .collect()
}
