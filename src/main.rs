use std::{env, fs};
use std::collections::{HashMap};
use std::fs::read;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Error};
use cargo_toml::{Dependency, DependencyDetail, DepsSet, Manifest};
use clap::{Parser};
use clap::ArgEnum;
use git2::{Commit, Oid, Repository};
use pathdiff::diff_paths;
use regex::{CaptureMatches, Captures, Regex};
use text_io::read;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// What mode to run the program in
    #[clap(arg_enum, value_parser)]
    mode: Mode,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ArgEnum)]
enum Mode {
    LocalPath,
    GitRef,
    Version,
}

fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    // Create a new manifest
    let mut uber = Manifest::from_str("[workspace]").context("Error creating manifest")?;
    let mut packages = HashMap::new();
    let mut tomls = HashMap::new();

    // Populate manifest by adding any manifest in subfolders
    let path = env::current_dir()?;
    build_manifest(&cli.mode, &path, &path, &mut uber, &mut tomls, &mut packages, None)
        .context("Error building manifest")?;

    println!("{} files are about to be overwritten, would you like to continue? (Y/n)",
             packages.len() + 1);
    let line: String = read!("{}\n");
    if !line.is_empty() && line.to_lowercase() != "y" {
        println!("No files were changed.");
        return Ok(());
    }

    // Rewrite manifests to refer to each other by relative path
    update_manifests(&cli.mode, &tomls, &packages)?;

    // Write out a new parent worksapce toml
    let bytes = toml::ser::to_vec(&uber).context("Error serializing manifest")?;
    let path = path.join("Cargo.toml");
    fs::write(path, bytes).context("Error writing file")?;

    println!("Manifests have been updated!");
    Ok(())
}

fn update_manifests(
    mode: &Mode,
    tomls: &HashMap<String, PathBuf>,
    packages: &HashMap<String, PackageRef>,
) -> anyhow::Result<()> {
    let toml_paths: Vec<_> = tomls.values().collect();
    for toml_path in toml_paths {
        let input_str = fs::read_to_string(&toml_path).context("Error reading manifest")?;
        let mut output_str = "".to_string();
        let bytes = read(&toml_path).context("Error reading manifest")?;
        let mani = Manifest::from_slice(&bytes).context("Error parsing manifest")?;
        let pkg_path = toml_path.parent().context("Error getting parent path")?.to_path_buf();
        let pkg_name = mani.package.unwrap().name;

        let re = Regex::new(r"\n\[(.*)\]\n").context("Error creating regex")?;
        let splitter = SplitCaptures::new(&re, input_str.as_str());
        let mut cur_section = None;
        for state in splitter {
            match state {
                SplitState::Unmatched(txt) => {
                    if let Some(cur_section) = cur_section {
                        let str = replace_deps(mode, packages, cur_section, &pkg_path, txt, &pkg_name)
                            .context("Unable to replace dependencies!")?;
                        output_str += str.as_str();
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

        fs::write(&toml_path, output_str)?;
    }
    Ok(())
}

fn replace_deps(
    mode: &Mode,
    packages: &HashMap<String, PackageRef>,
    deps: &DepsSet,
    pkg_path: &PathBuf,
    input_str: &str,
    pkg_name: &String,
) -> anyhow::Result<String> {
    let mut str = input_str.to_string();
    for (name, src_dep) in deps {
        let other_pkg = match packages.get(&name.clone()) {
            None => continue,
            Some(it) => it,
        };
        let this_pkg = &packages[pkg_name];
        let relative = diff_paths(&other_pkg.path, pkg_path).ok_or(anyhow!("Can't diff paths!"))?;
        let relative = relative.to_str().ok_or(anyhow!("Can't diff paths!"))?.to_string();
        let new_dep = match mode {
            Mode::LocalPath => clone_path_dep(src_dep, relative),
            Mode::GitRef => {
                if this_pkg.git.url == other_pkg.git.url {
                    clone_path_dep(src_dep, relative)
                } else {
                    clone_git_dep(src_dep, &other_pkg.git)
                }
            }
            Mode::Version => clone_ver_dep(src_dep, &other_pkg.version),
        };
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

fn clone_path_dep(src_dep: &Dependency, relative: String) -> Dependency {
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

fn clone_ver_dep(src_dep: &Dependency, version: &String) -> Dependency {
    match src_dep {
        Dependency::Simple(_) => {
            Dependency::Detailed {
                0: DependencyDetail {
                    version: Some(version.clone()),
                    registry: None,
                    registry_index: None,
                    path: None,
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
                    version: Some(version.clone()),
                    registry: None,
                    registry_index: None,
                    path: None,
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

fn clone_git_dep(src_dep: &Dependency, git_ref: &GitRef) -> Dependency {
    match src_dep {
        Dependency::Simple(_) => {
            Dependency::Detailed {
                0: DependencyDetail {
                    version: None,
                    registry: None,
                    registry_index: None,
                    path: None,
                    git: Some(git_ref.url.clone()),
                    branch: None,
                    tag: None,
                    rev: Some(git_ref.oid.to_string()),
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
                    path: None,
                    git: Some(git_ref.url.clone()),
                    branch: None,
                    tag: None,
                    rev: Some(git_ref.oid.to_string()),
                    features: it.features.clone(),
                    optional: it.optional,
                    default_features: it.default_features,
                    package: None
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct GitRef {
    pub url: String,
    pub oid: Oid,
}

struct PackageRef {
    pub path: PathBuf,
    pub git: GitRef,
    pub version: String,
}

fn contains_commit(
    search: &Commit,
    target: &Commit,
) -> bool {
    if search.id() == target.id() {
        return true;
    }
    for parent in search.parents() {
        if contains_commit(&parent, target) {
            return true;
        }
    }
    false
}

fn build_manifest(
    mode: &Mode,
    base: &PathBuf,
    path: &PathBuf,
    uber: &mut Manifest,
    tomls: &mut HashMap<String, PathBuf>,
    packages: &mut HashMap<String, PackageRef>,
    mut git_ref: Option<GitRef>,
) -> anyhow::Result<()> {
    if let Ok(repo) = Repository::open(&path) {
        let head = repo.head().context("Error getting HEAD!")?
            .peel_to_commit().context("Error getting commit!")?;
        let remote = best_remote_with_commit(&repo, &head)?;
        git_ref = Some(GitRef { url: remote, oid: head.id() });
    }

    // scan subfolders
    let paths = path.read_dir().context("Error scanning directory")?;
    for path in paths {
        let path = path.context("Error enumerating files")?;
        if path.metadata().context("Error getting file metadata")?.is_dir() {
            build_manifest(mode, base, &path.path(), uber, tomls, packages, git_ref.clone())
                .context("Error building manifest")?;
            continue;
        }
        let name = path.file_name();
        if name != "Cargo.toml" {
            continue;
        }
        let bytes = read(path.path()).context("Error reading bytes")?;
        let abs = path.path().parent().ok_or(anyhow!("Error getting parent path"))?.to_path_buf();
        let relative = diff_paths(&abs, &base).ok_or(anyhow!("Error relativizing path"))?;
        if relative.parent().is_none() {
            continue; // top level relative path
        }
        let relative = relative.to_str().ok_or(anyhow!("Error getting path"))?.to_string();
        let mani = Manifest::from_slice(&bytes).context("Error reading manifest")?;
        if let Some(pkg) = mani.package.as_ref() {
            println!("{} is at {:?}", pkg.name, git_ref);
            let git_ref = match &git_ref {
                None => Err(anyhow!("No git repo found!"))?,
                Some(it) => it,
            };
            let pkg = mani.package.ok_or(anyhow!("No package found!"))?;
            let pkg_ref = PackageRef {
                path: abs,
                git: git_ref.clone(),
                version: pkg.version,
            };

            packages.insert(pkg.name.clone(), pkg_ref);
            tomls.insert(pkg.name.clone(), path.path().clone());
            uber.workspace.as_mut().ok_or(anyhow!("workspace needed!"))?
                .members.push(relative.clone());
        }
        if let Some(mani) = mani.workspace.as_ref() {
            uber.workspace.as_mut().ok_or(anyhow!("workspace needed!"))?
                .exclude.push(relative.clone());
            for exclude in &mani.exclude {
                uber.workspace.as_mut().ok_or(anyhow!("workspace needed!"))?
                    .exclude.push(format!("{}/{}", relative.to_string(), exclude));
            }
            println!("Deleting {:?}", path.path());
            fs::remove_file(path.path())?;
        }
    }
    Ok(())
}

fn best_remote_with_commit(
    repo: &Repository,
    head: &Commit
) -> anyhow::Result<String> {
    let order = vec!["upstream", "origin"];
    let all_remotes = get_remotes(&repo)?;
    let mut best_remote = None;
    let mut best_score = usize::max_value();
    for reference in repo.references().context("Error getting references!")? {
        let reference = reference.context("Error getting reference!")?;
        if !reference.is_remote() {
            continue;
        }
        let name = reference.name().ok_or(anyhow!("Error getting reference name!"))?;
        let parts: Vec<_> = name.split("/").collect();
        if parts.len() < 3 || parts[0] != "refs" || parts[1] != "remotes" {
            Err(anyhow!("Invalid reference name!"))?;
        }
        let remote = parts[2];
        let score = order.iter().position(|it| it == &remote).unwrap_or(usize::max_value() - 1);
        if score >= best_score {
            continue;
        }
        let commit = reference.peel_to_commit().context("Error getting commit!")?;
        if !contains_commit(&commit, &head) {
            continue;
        }
        best_remote = Some(all_remotes[remote].clone());
        best_score = score;
    }
    Ok(best_remote.ok_or(anyhow!("No remote found!"))?)
}

fn get_remotes(repo: &Repository) -> anyhow::Result<HashMap<String, String>> {
    let mut remotes = HashMap::<String, String>::new();
    for remote in &repo.remotes().context("Error getting remotes!")? {
        let remote = remote.ok_or(anyhow!("Unable to get remote!"))?;
        let remote = repo.find_remote(&remote).context("Unable to find remote!")?;
        let url = remote.url().ok_or(anyhow!("Unable to get URL!"))?;
        let name = remote.name().ok_or(anyhow!("Unable to get name!"))?;
        remotes.insert(name.to_string(), url.to_string());
    }
    Ok(remotes)
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
            text,
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
