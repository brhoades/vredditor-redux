use std::fs::canonicalize;
use std::path::PathBuf;

use anyhow::{format_err, Result};
use glob::glob;
use prost_build::compile_protos;

fn main() {
    let proto_path = PathBuf::from(option_env!("PROTO_PATH").unwrap_or("../proto"));
    let base_path = canonicalize(proto_path.clone()).map_err(|e| format_err!("tried path {}: {}", proto_path.to_str().unwrap(), e)).unwrap();
    let path = base_path.join(PathBuf::from("*.proto"));
    let pathstr = path
        .to_str()
        .ok_or_else(|| format_err!("failed to make base path into string")).unwrap();

    let protos = glob(pathstr)
        .map_err(|e| format_err!("failed to glob path {}: {}", pathstr, e)).unwrap()
        .into_iter()
        .map(|r| {
            r.map_err(|e| format_err!("{}", e).context("glob succeeded but path was erroneous"))
        })
        .collect::<Result<Vec<_>>>().unwrap();

    compile_protos(protos.as_slice(), &[base_path]).unwrap()
}
