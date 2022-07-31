use std::{env, fs};
use std::collections::HashMap;
use std::error::Error;
use std::fs::read;
use std::path::PathBuf;
use anyhow::Context;

use cargo_toml::Manifest;

fn main() -> Result<(), Box<dyn Error>> {
    // Create a new manifest
    let mut uber = Manifest::from_str("[workspace]")?;

    // Populate manifest by adding any manifest in subfolders
    let path = env::current_dir()?;
    scan(&path, &path, &mut uber)?;

    // Write out a new parent worksapce toml
    let bytes = toml::ser::to_vec(&uber)?;
    let path = path.join("Cargo.toml");
    fs::write(path, bytes).unwrap();
    Ok(())
}

fn scan(base: &PathBuf, path: &PathBuf, uber: &mut Manifest) -> Result<(), Box<dyn Error>> {
    let paths = path.read_dir()?;
    for path in paths {
        let path = path?;
        if path.metadata()?.is_dir() {
            scan(base, &path.path(), uber)?;
            continue;
        }
        let name = path.file_name();
        if name != "Cargo.toml" {
            continue;
        }
        let bytes = read(path.path())?;
        let relative = path.path().strip_prefix(base)?
            .to_str().context("Missing path")?.to_string();
        if relative == "Cargo.toml" {
            continue;
        }
        let mani = Manifest::from_slice(&bytes)?;
        if let Some(pkg) = mani.package.as_ref() {
            uber.workspace.as_mut().ok_or("workspace needed!")?
                .members.push(relative.clone());
        }
        if let Some(_) = mani.workspace.as_ref() {
            uber.workspace.as_mut().ok_or("workspace needed!")?
                .exclude.push(relative.clone());
        }
    }
    Ok(())
}