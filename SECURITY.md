# Security policy

## Reporting a vulnerability

Please report it privately, not in a public issue. On the repository's **Security** tab choose **Report a
vulnerability** (GitHub private vulnerability reporting). Include the `vtee` version or commit, your operating
system, and the steps to reproduce.

## Supported versions

`vtee` is a 0.x project. Only the latest release receives fixes.

## What counts

The trust model is described in [docs/guide/security.md](docs/guide/security.md): programs running as the same user
are trusted, and other users are kept out.

In scope:
- A user other than the session's owner (or root, or on Windows SYSTEM) connecting to a session, or reading or
  changing its files.
- Socket files or session directories created with permissions looser than documented.
- A session name or path that escapes the session directory.
- A connected client crashing or hanging the holder in a way the documented limits should prevent.
- Any way to make `vtee kill` signal or terminate an unrelated process.

Out of scope:
- Actions by another process running as the same user, which can already do the same by other means.
- Attacks that need root, an administrator, or control of the user's account.
- Behaviour documented as a limitation, such as orphaned programs after `SIGKILL`.
