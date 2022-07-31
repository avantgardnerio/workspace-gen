# workspace-gen

Given several subdirectories containing cargo projects, creates a parent workspace manifest file.

## Usage

```sh
git clone x # clone a project into a subfolder
git clone y # clone a 2nd project 
cargo install workspace-gen # add this executable to the path
workspace-gen # run it with no arguments
cargo build # A Cargo.toml now exists, and should wrap both subprojects in a workspace!
```
