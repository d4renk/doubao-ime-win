# Security Policy

## Dependency and build policy

- Release builds must use the committed `Cargo.lock` through `cargo build --locked`.
- GitHub Actions are pinned to immutable commit SHAs and updated through Dependabot.
- The vendored Protoc archive is verified against its published SHA-256 before use.
- CI runs RustSec against every pull request and protected branch update.

RustSec may report maintenance warnings for platform-specific transitive dependencies. The
project currently accepts `RUSTSEC-2026-0150` for `audiopus_sys` because the maintained
high-level `opus` crate has no compatible replacement and the advisory does not describe a
known vulnerability. This exception must be reviewed whenever the audio stack is updated.

## Release verification

Each GitHub Release contains `doubao-voice-input.exe.sha256`. Verify it in PowerShell with:

```powershell
(Get-FileHash .\doubao-voice-input.exe -Algorithm SHA256).Hash.ToLowerInvariant()
Get-Content .\doubao-voice-input.exe.sha256
```

Release executables are not currently Authenticode-signed. The checksum detects corruption
or replacement relative to the GitHub Release, but it does not establish a publisher identity.
