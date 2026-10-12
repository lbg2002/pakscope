# Security Policy

PakScope v0.1 does not modify installed applications. It scans local metadata only. It does not require root and it does not collect telemetry.

Never infer a delete target from an installation directory: APT/DEB packages may share `/usr`, `/opt`, library paths, user data and other files.

For reports involving command injection, ambiguous package ownership, privilege escalation, or untrusted desktop/AppImage files, please open a private security advisory rather than a public issue. The future uninstall feature must use the originating package manager with impact previews and confirmation.
