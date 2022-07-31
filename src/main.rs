use std::{env, fs};
use std::collections::{HashMap};
use std::fs::read;
use std::path::PathBuf;
use anyhow::{anyhow, Context, Error};
use regex::{CaptureMatches, Captures, Regex};

use cargo_toml::{Dependency, DependencyDetail, DepsSet, Manifest};
use pathdiff::diff_paths;

fn main() -> Result<(), Error> {
    // Create a new manifest
    let mut uber = Manifest::from_str("[workspace]").context("Error creating manifest")?;
    let mut packages = HashMap::<String, PathBuf>::new();

    // Populate manifest by adding any manifest in subfolders
    let path = env::current_dir()?;
    build_manifest(&path, &path, &mut uber, &mut packages).context("Error building manifest")?;

    // Rewrite manifests to refer to each other by relative path
    update_manifests(&packages)?;

    // Write out a new parent worksapce toml
    let bytes = toml::ser::to_vec(&uber).context("Error serializing manifest")?;
    let path = path.join("Cargo.toml");
    fs::write(path, bytes).context("Error writing file")?;
    Ok(())
}

fn update_manifests(
    packages: &HashMap<String, PathBuf>,
) -> anyhow::Result<()> {
    let mani_paths: Vec<_> = packages.values().collect();
    for mani_path in mani_paths {
        let input_str = fs::read_to_string(mani_path).context("Error reading manifest")?;
        let mut output_str = "".to_string();
        let bytes = read(mani_path).context("Error reading manifest")?;
        let mani = Manifest::from_slice(&bytes).context("Error parsing manifest")?;

        let re = Regex::new(r"\n\[(.*)\]\n").context("Error creating regex")?;
        let splitter = SplitCaptures::new(&re, input_str.as_str());
        let mut cur_section = None;
        for state in splitter {
            match state {
                SplitState::Unmatched(txt) => {
                    if let Some(cur_section) = cur_section {
                        output_str += replace_deps(packages, cur_section, mani_path, txt)?.as_str();
                    } else {
                        output_str += txt;
                    }
                    cur_section = None;
                },
                SplitState::Captured(caps) => {
                    let section = &caps[1].to_string();
                    output_str += format!("\n[{}]\n", section).as_str();
                    cur_section = match section.as_str() {
                        "dependencies" => Some(&mani.dependencies),
                        "dev-dependencies" => Some(&mani.dev_dependencies),
                        "build-dependencies" => Some(&mani.build_dependencies),
                        _ => None
                    };
                },
            }
        }

        fs::write(mani_path, output_str)?;
    }
    Ok(())
}

fn replace_deps(
    packages: &HashMap<String, PathBuf>,
    deps: &DepsSet,
    mani_path: &PathBuf,
    input_str: &str,
) -> anyhow::Result<String> {
    let mut str = input_str.to_string();
    for (name, src_dep) in deps {
        let dep_path = match packages.get(&name.clone()) {
            None => continue,
            Some(it) => it,
        };
        let relative = diff_paths(dep_path, mani_path).ok_or(anyhow!("Can't diff paths!"))?;
        let relative = relative.to_str().ok_or(anyhow!("Can't diff paths!"))?.to_string();
        let new_dep = clone_dep(src_dep, relative);
        let new_dep = toml::ser::to_string(&new_dep).context("Error serializing manifest")?;
        let new_dep: Vec<_> = new_dep.trim().split("\n").collect();
        let new_dep = new_dep.join(", ");
        let new_dep = format!("{} = {{ {} }}", name, new_dep);
        let re = Regex::new(format!(r#"{}\s*=\s*.*"#, name).as_ref())
            .context("Error creating regex")?;
        str = re.replace_all(&str, new_dep).to_string();
    }
    return Ok(str);
}

fn clone_dep(src_dep: &Dependency, relative: String) -> Dependency {
    match src_dep {
        Dependency::Simple(_) => {
            Dependency::Detailed {
                0: DependencyDetail {
                    version: None,
                    registry: None,
                    registry_index: None,
                    path: Some(relative),
                    git: None,
                    branch: None,
                    tag: None,
                    rev: None,
                    features: vec![],
                    optional: false,
                    default_features: None,
                    package: None
                }
            }
        }
        Dependency::Detailed(it) => {
            Dependency::Detailed {
                0: DependencyDetail {
                    version: None,
                    registry: None,
                    registry_index: None,
                    path: Some(relative),
                    git: None,
                    branch: None,
                    tag: None,
                    rev: None,
                    features: it.features.clone(),
                    optional: it.optional,
                    default_features: it.default_features,
                    package: None
                }
            }
        }
    }
}

fn build_manifest(
    base: &PathBuf,
    path: &PathBuf,
    uber: &mut Manifest,
    packages: &mut HashMap<String, PathBuf>,
) -> Result<(), Error> {
    let paths = path.read_dir().context("Error scanning directory")?;
    for path in paths {
        let path = path.context("Error enumerating files")?;
        if path.metadata().context("Error getting file metadata")?.is_dir() {
            build_manifest(base, &path.path(), uber, packages).context("Error building manifest")?;
            continue;
        }
        let name = path.file_name();
        if name != "Cargo.toml" {
            continue;
        }
        let bytes = read(path.path()).context("Error reading bytes")?;
        let abs = path.path();
        let relative = abs.strip_prefix(base).context("Error relativizing path")?
            .to_str().context("Missing path")?.to_string();
        if relative == "Cargo.toml" {
            continue;
        }
        let mani = Manifest::from_slice(&bytes).context("Error reading manifest")?;
        if let Some(pkg) = mani.package.as_ref() {
            packages.insert(pkg.name.clone(), abs);
            uber.workspace.as_mut().ok_or(anyhow!("workspace needed!"))?
                .members.push(relative.clone());
        }
        if let Some(_) = mani.workspace.as_ref() {
            uber.workspace.as_mut().ok_or(anyhow!("workspace needed!"))?
                .exclude.push(relative.clone());
        }
    }
    Ok(())
}

struct SplitCaptures<'r, 't> {
    finder: CaptureMatches<'r, 't>,
    text: &'t str,
    last: usize,
    caps: Option<Captures<'t>>,
}

impl<'r, 't> SplitCaptures<'r, 't> {
    fn new(re: &'r Regex, text: &'t str) -> SplitCaptures<'r, 't> {
        SplitCaptures {
            finder: re.captures_iter(text),
            text: text,
            last: 0,
            caps: None,
        }
    }
}

#[derive(Debug)]
enum SplitState<'t> {
    Unmatched(&'t str),
    Captured(Captures<'t>),
}

impl<'r, 't> Iterator for SplitCaptures<'r, 't> {
    type Item = SplitState<'t>;

    fn next(&mut self) -> Option<SplitState<'t>> {
        if let Some(caps) = self.caps.take() {
            return Some(SplitState::Captured(caps));
        }
        match self.finder.next() {
            None => {
                if self.last >= self.text.len() {
                    None
                } else {
                    let s = &self.text[self.last..];
                    self.last = self.text.len();
                    Some(SplitState::Unmatched(s))
                }
            }
            Some(caps) => {
                let m = caps.get(0).unwrap();
                let unmatched = &self.text[self.last..m.start()];
                self.last = m.end();
                self.caps = Some(caps);
                Some(SplitState::Unmatched(unmatched))
            }
        }
    }
}
