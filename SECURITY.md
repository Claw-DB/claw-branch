# Security Policy

## Reporting a Vulnerability

Please report security issues privately to the maintainers before public disclosure.

- Open a private security advisory on GitHub for this repository when possible.
- Include reproduction steps, impact, and affected versions.
- We will acknowledge receipt within 72 hours and provide an initial triage timeline.

## Scope Notes

`claw-branch` persists branch state as local SQLite files.

- The caller is responsible for host and filesystem permissions.
- Use per-workspace filesystem isolation and least-privilege directory access.
- Do not place branch files on world-writable paths.

## Supported Versions

- `0.1.x`: supported
