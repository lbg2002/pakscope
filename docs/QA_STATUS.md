# QA / Validation status — 2026-10-12

## Checks completed in the authoring environment

- [x] `Cargo.toml` parseable TOML.
- [x] SVG valid XML.
- [x] Desktop launcher structure present.
- [x] `scripts/build-deb.sh` Bash syntax validation passed.
- [x] HTML demo inline JavaScript syntactically parsed by Node.js.
- [x] Scanner has no delete commands, root requests, or network clients.
- [x] Basic unit tests added for desktop parsing and source labels (not executed here).

## Not yet verified (must not be reported as passed)

- [ ] Rust `cargo test` / `cargo check` / `cargo build`: Rust toolchain unavailable in authoring container.
- [ ] GTK4/libadwaita window launch or live screenshot: GTK developer/runtime dependencies unavailable here.
- [ ] Actual Ubuntu 26.04 scan correctness: must test on Ubuntu host.
- [ ] DEB build and install: requires Rust + GTK headers + dpkg-deb on an Ubuntu runner.
- [ ] GitHub CI status: repository creation/push is pending.

The GitHub Actions workflow is included so the build and test suite can run automatically as soon as the project is pushed. Release packages should only be published after its green build and local scan validation.
