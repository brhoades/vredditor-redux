use std::fs::canonicalize;
use std::path::PathBuf;

use anyhow::{format_err, Result};
use glob::glob;
use prost_build::compile_protos;

fn main() {
    match build() {
        Ok(_) => (),
        Err(e) => {
            println!("{}", e);
            println!("{}", e.backtrace());
            panic!("{}", e)
        }
    }
}

fn build() -> Result<()> {
    let base_path = canonicalize(PathBuf::from("../proto"))?;
    let path = base_path.join(PathBuf::from("*.proto"));
    let pathstr = path
        .to_str()
        .ok_or_else(|| format_err!("failed to make base path into string"))?;

    let protos = glob(pathstr)
        .map_err(|e| format_err!("failed to glob path {}: {}", pathstr, e))?
        .into_iter()
        .map(|r| {
            r.map_err(|e| format_err!("{}", e).context("glob succeeded but path was erroneous"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(compile_protos(protos.as_slice(), &[base_path]).unwrap())
}
