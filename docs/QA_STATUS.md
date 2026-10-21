# PakScope v0.2 · QA gates (2026-10-21)

- Unit tests cover identifier rejection, command plans, APT purge/remove, Flatpak user/system scope and AppImage guards.
- CI executes scanner tests, Rust compiler checks, GTK startup smoke test under Xvfb and DEB packaging.
- Safety: no shell, source validation, APT simulation + second plan check, explicit user approval.
- CI deliberately does not exercise real destructive package removal. GNOME/Polkit workflows require manual verification.
- QA status depends on the GitHub Actions results associated with this branch; this file does not imply a passing build.
