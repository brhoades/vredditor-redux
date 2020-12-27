use std::{env::temp_dir, fs::remove_file, iter, path::PathBuf};

use rand::{distributions, thread_rng, Rng};
use tokio::fs::File;
use tokio_util::codec::{BytesCodec, FramedRead};

use crate::internal::*;

// returns a named tempfile which deletes itself when it falls out of scope.
// this still doesn't handle deletion on crashes, but it does handle
// happy path cases
#[derive(Debug)]
pub struct GuardedTempFile {
    path: PathBuf,
    pathstr: String,
    f: File,
}

pub type TempFileStream = FramedRead<File, BytesCodec>;

#[allow(dead_code)]
impl GuardedTempFile {
    pub async fn new() -> Result<Self> {
        let path = temp_dir().join(random_id());

        let pathstr = path.clone();
        let pathstr = pathstr
            .to_str()
            .ok_or_else(|| {
                format_err!(
                    "failed to convert random file pathbuf to string: {:?}",
                    pathstr
                )
            })?
            .to_string();

        Ok(Self {
            f: File::create(&path)
                .await
                .context("creating temporary named file")?,
            pathstr,
            path,
        })
    }

    pub fn pathstr(&self) -> &str {
        self.pathstr.as_str()
    }

    pub fn path_string(&self) -> String {
        self.pathstr.clone()
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn filename(&self) -> Result<String> {
        self.path
            .file_name()
            .and_then(|filename| filename.to_str())
            .map(str::to_owned)
            .ok_or_else(|| format_err!("failed to get file name from path: {}", self.pathstr()))
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

    // reopens this file in place without deleting it
    pub async fn reopen(&mut self) -> Result<()> {
        self.f = File::open(&self.path).await?;
        Ok(())
    }

    pub async fn stream(&mut self) -> Result<FramedRead<File, BytesCodec>> {
        // XXX: this clone doesn't seem ideal. Maybe we should return a guarded framedread?
        self.f
            .try_clone()
            .await
            .map(|f| FramedRead::new(f, BytesCodec::new()))
            .ah()
    }
}

impl Drop for GuardedTempFile {
    fn drop(&mut self) {
        let err = remove_file(self.path.clone());

        trace!(
            "deleted on drop: {} ({})",
            self.pathstr(),
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
