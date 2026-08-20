# Security Policy

> **Status: DRAFT — confirm the publisher/contact details and have this reviewed before publishing.**
> Publisher: **The Elideus Group**. Project: **flicker** / **Prism Alpha**, the client of the **ClayEngine** ecosystem.
> Applies to: the **v0.2.x** release line. Last updated: 2026-08-18.

flicker is an alpha, open-source game client distributed under a dual
[MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE) license. We take the integrity of
what we ship — and of the open-source graph we build on — seriously. This document
describes how to report a vulnerability and the security posture we maintain.

See also: [PRIVACY.md](PRIVACY.md) (what data the platform holds and how) and
[WARRANTY.md](WARRANTY.md) (the as-is / no-warranty terms).

## Supported versions

flicker is in alpha and ships as a single moving line of releases. Only the **latest
published release** on GitHub Releases is supported; there are no back-ported security
fixes for older tags.

| Version | Supported |
| ------- | --------- |
| Latest release (`0.2.x`) | ✅ |
| Any older tag | ❌ |

## Reporting a vulnerability

**Please report privately — do not open a public issue for a security problem.**

- **Preferred:** open a private advisory via **GitHub Security Advisories** on
  <https://github.com/elide-us/flicker/security/advisories/new>.
- **Email:** <aaron@elideus.net>.

Please include: the affected component and version, a description of the issue, the
impact you foresee, and enough detail (steps, proof-of-concept, or a minimal repro) to
reproduce it. If it concerns the online account/identity platform rather than the game
client, say so.

**What to expect.** This is a small, pre-alpha project maintained on a best-effort
basis. We aim to acknowledge a report within a reasonable window, keep you updated, and
credit you if you would like. We practice **coordinated disclosure**: please give us a
reasonable opportunity to remediate before disclosing publicly. There is no paid bug
bounty at this time.

## Supply-chain posture

Our stance is shaped by the wave of real supply-chain attacks against open-source
package ecosystems. We treat every third-party dependency as untrusted-by-default and
gate the whole graph:

- **The lockfile is the pin.** `Cargo.lock` is committed and every CI/release build runs
  with `--locked`. That fixes exact versions **and** SHA-256 checksums for the entire
  dependency graph, so a typosquat, a registry tamper, or a freshly-compromised publish
  cannot enter the build without showing up as a reviewable lockfile diff. We deliberately
  do **not** use `=` exact pins in manifests — they block patch-level security fixes while
  duplicating what the lockfile already guarantees.
- **Reviewed version floors.** Workspace manifests carry reviewed minimum versions, so a
  future dependency resolution can never silently drop below an audited baseline.
- **A hard `cargo-deny` gate** runs in CI on every push and can be run locally with
  `cargo deny check`. See [deny.toml](deny.toml). It enforces:
  - **advisories** — the [RustSec](https://rustsec.org) database; a direct dependency going
    unmaintained fails the gate;
  - **sources** — **crates.io only**. No git dependencies and no alternate registries can
    enter the graph;
  - **licenses** — a permissive-only allowlist; anything new fails and gets reviewed;
  - **bans** — bare `*` version requirements are denied.
- **CI actions are pinned to full commit SHAs** (not tags), defending against the
  compromised-tag class of GitHub Actions attacks. Movement of both the crate pins and the
  action pins happens only through reviewed, CI-gated **Dependabot** PRs — see
  [.github/dependabot.yml](.github/dependabot.yml).
- **Update cooldown.** Dependabot will not propose a release until it has been public long
  enough for a compromised publish to be caught and yanked (**14 days** for crates, **30**
  for semver-majors, **7** for actions). Nothing auto-merges; secrets are withheld from
  Dependabot-triggered CI runs.
- **Minimal, audited native surface.** No OpenSSL, `git2`, or `curl` anywhere; TLS is
  `rustls` + `ring`; compression is pure-Rust. The few vendored C/native surfaces
  (e.g. the FBX importer, the Luau runtime) are pinned by lockfile checksums like
  everything else. Unused dependencies are removed on sight — a dead dependency is free
  attack surface.

## Open-source and third-party components

flicker is built on open-source software and ships as open source. We do not vendor
opaque binaries, and we don't add dependencies casually: new dependencies are reviewed by
name (to catch typosquats), constrained to the permissive-license allowlist above, and
minimized. Third-party components remain under their own licenses; the `cargo-deny`
license gate is the enforced record of what is permitted. Bundled assets carry their own
licenses too — e.g. the Prism fonts are Google Fonts families under the SIL Open Font
License 1.1, with the texts included under [Prism/Licenses/](Prism/Licenses/); see
[WARRANTY.md](WARRANTY.md).

## Distribution integrity

- **Official builds come only from the official release pipeline.** Releases are cut by a
  tag-triggered GitHub Actions workflow ([.github/workflows/release.yml](.github/workflows/release.yml))
  that publishes to **GitHub Releases** for this repository. Per-OS installers (Windows
  MSI, macOS `.pkg`, Linux `.deb`) and portable archives are built there; game content
  ships as a single deterministic, store-only `package.flk` packed once in CI.
- **Verify your download.** Every release publishes a `SHA256SUMS` file. Check your
  download against it before running.
- **Signing status.** Alpha builds are currently **unsigned**; Windows SmartScreen and
  macOS Gatekeeper will warn on first launch. Code signing is wired into the pipeline and
  gated on signing secrets — it will activate without workflow changes once certificates
  are provisioned. Until then, `SHA256SUMS` verification is the integrity check.
- **Builds obtained anywhere else are unofficial.** The license permits redistribution,
  but we cannot vouch for the integrity of third-party rebuilds or repackages. Prefer the
  official Releases page.

## Runtime security boundary

- **Content scripting is data, not trusted code.** The Luau scripting layer sits behind a
  strict data-only boundary: scripts drive UI/behaviour through a constrained contract and
  are not a path for arbitrary host code execution. Untrusted script content is treated as
  untrusted.
- **The networking client is thin and explicit.** `flicker-net` holds two independent
  clients: an anonymous release-update check against the public GitHub Releases API (no user
  identifiers, silent offline), and a `clay-chat` client to ClayEngine's own chat server.
  Networked play (chat, world-state sync) and account features (auth/entitlements via
  TheOracleRPC) reach only our own servers, and only when you go online; server host
  discovery and authentication are moving to the web backend rather than being hard-coded.
  See [PRIVACY.md](PRIVACY.md).

## Platform & account security

For the online platform (accounts, entitlements), sign-in is delegated to third-party
identity providers over OAuth/OIDC — **we never receive or store account passwords** —
and user identity is stored in tokenized form. Application secrets are held in an
environment/secret store outside the source tree and outside the database. The data-model
details and the privacy guarantees are documented in [PRIVACY.md](PRIVACY.md).

---

*This policy is provided in good faith for a pre-alpha project and will evolve. It is not
a contract and does not limit the disclaimers in [WARRANTY.md](WARRANTY.md) or the license
files.*
