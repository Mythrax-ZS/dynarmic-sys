# Publishing this fork

## One-time GitHub setup

```bash
cd thirdparty/dynarmic-sys
git init
git add -A
git commit -m "Initial Mythrax fork: Windows MSVC build support"
git branch -M main
git remote add origin https://github.com/Mythrax-ZS/dynarmic-sys.git
git push -u origin main
```

## Publish to crates.io

1. Register on crates.io (one-time) and grab an API token from
   https://crates.io/me/tokens.
2. Authenticate locally (one-time per machine):

   ```bash
   cargo login <your-api-token>
   ```

3. Verify the package fits under the registry's 10 MiB compressed limit:

   ```bash
   cd thirdparty/dynarmic-sys
   cargo package --list           # show every file that will be uploaded
   cargo package                  # build the .crate file under target/package/
   ls -la target/package/dynarmic-sys-mythrax-0.2.0.crate
   ```

   Upstream `dynarmic-sys-0.1.2` packages to ~3.7 MiB compressed; ours should
   be similar. If it exceeds 10 MiB, request a quota bump via
   https://crates.io/data-access.

4. Dry-run, then publish:

   ```bash
   cargo publish --dry-run
   cargo publish
   ```

## After publish

In the workspace `Cargo.toml` (or `crates/nx-cpu-dynarmic/Cargo.toml`),
swap the local path dep for the registry version so others can build the
emulator without checking out this fork:

```toml
[dependencies]
dynarmic-sys = { package = "dynarmic-sys-mythrax", version = "0.2" }
```

(Keep the `[patch.crates-io]` entry around if you ever need to test a
local edit before publishing a new version.)

## Bumping versions

Patches (build/script fixes only): bump the third digit, e.g. 0.2.1.
New runtime API or behavior change: bump the second digit, 0.3.0.
Breaking C++/ABI: bump the first, 1.0.0.

Edit `Cargo.toml` `version`, `git commit -am "release vX.Y.Z"`,
`git tag vX.Y.Z`, `git push --tags`, then `cargo publish`.
