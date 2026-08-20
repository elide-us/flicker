# Warranty & Disclaimer

> **Status: DRAFT — confirm the publisher/contact details and have this reviewed before publishing.**
> Publisher: **The Elideus Group**. Project: **flicker** / **Prism Alpha**, the client of the **ClayEngine** ecosystem.
> Applies to: the **v0.2.x** release line. Last updated: 2026-08-18.

This document explains, in plain language, the warranty position for flicker. The
**binding terms** are the project's license files — [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE) — which you may choose between. Where anything here and
those files differ, the license files govern.

See also: [SECURITY.md](SECURITY.md) and [PRIVACY.md](PRIVACY.md).

## No warranty — provided "as is"

flicker is free, open-source software provided **"AS IS", without warranty of any kind**,
express or implied — including, without limitation, the implied warranties of
merchantability, fitness for a particular purpose, title, and non-infringement. You use it
at your own risk. This restates the warranty disclaimers already in the MIT license and in
sections 7 (Disclaimer of Warranty) and 8 (Limitation of Liability) of the Apache License,
Version 2.0.

## Alpha software

flicker is **alpha** software (the `0.2.x` line). Substantial parts work, but it is not a
finished product:

- it is **not yet a feature-complete game**;
- it may crash, misbehave, corrupt or lose data, or change behavior between releases
  **without notice**;
- interfaces, formats, save data, and content are **unstable** and may break at any time;
- it is **not fit for production, safety-critical, or otherwise important use**.

Keep backups of anything you value. Do not rely on flicker for any purpose where failure
could cause loss or harm.

## Distribution and authenticity

We want you to be able to trust what you run:

- **Official builds are distributed only through the official release pipeline** — the
  tag-triggered GitHub Actions workflow that publishes to **GitHub Releases** for this
  repository (Windows MSI, macOS `.pkg`, Linux `.deb`, and portable archives), with game
  content shipped as a single deterministic `package.flk`.
- Every release publishes a **`SHA256SUMS`** file; verify your download against it.
- Alpha builds are currently **unsigned** (see [SECURITY.md](SECURITY.md)); `SHA256SUMS`
  verification is the integrity check until signing is enabled.
- The open-source licenses permit **anyone to rebuild and redistribute** flicker. Builds
  obtained from anywhere other than the official Releases page are **unofficial**: they
  carry no warranty from us, and we cannot vouch for their integrity or safety. Prefer the
  official channel.

## Content is fiction

flicker is a video game. Its worlds, planets, characters, personas, lore (the "Prism"
canon), and all generated or authored content are **works of fiction**. Any resemblance to
real persons, living or dead, or to actual events, places, or entities is coincidental.
Nothing in the software or its content is professional advice of any kind (financial,
legal, medical, or otherwise).

## Third-party components

flicker incorporates open-source components that remain under their own licenses; this
document does not extend any warranty over them, and their authors provide none through us.

- **Rust dependencies** are permissively licensed, enforced by the license gate in
  [deny.toml](deny.toml) (see [SECURITY.md](SECURITY.md)).
- **Bundled fonts are NOT covered by flicker's MIT/Apache-2.0 code license.** The Prism
  typeface roles are generated from four Google Fonts open-source families, each licensed
  under the **SIL Open Font License, Version 1.1 (OFL)**:
  - **Cinzel** — © 2020 The Cinzel Project Authors
  - **Cormorant Garamond** — © 2015 The Cormorant Project Authors
  - **EB Garamond** — © 2017 The EB Garamond Project Authors
  - **Noto Sans Runic** — © 2022 The Noto Project Authors

  The full OFL 1.1 license texts are included in the repository under
  [Prism/Licenses/](Prism/Licenses/). The shipped faces are modified single-weight
  instances **renamed off their original family names**, as the OFL's Reserved Font Name
  rule requires. The OFL is a font license only and does not affect the licensing of
  flicker's code.

## Limitation of liability

To the maximum extent permitted by applicable law, neither The Elideus Group nor the
project's contributors will be liable for any direct, indirect, incidental, special,
exemplary, or consequential damages — including loss of data, profits, or goodwill —
arising out of or relating to the use of, or inability to use, this software, even if
advised of the possibility of such damages. This mirrors and does not enlarge the
limitations in the license files.

**Your statutory rights.** Some jurisdictions do not allow the exclusion of certain
warranties or the limitation of certain liabilities. Nothing in this document or the
license files removes or limits any consumer or other rights you have under applicable law
that cannot legally be waived.

## Support

There is no service-level agreement and no guarantee of support, updates, or continued
availability. Help is best-effort and community-based:

- **Issues / bugs:** <https://github.com/elide-us/flicker/issues>
- **Security reports:** see [SECURITY.md](SECURITY.md) (please report privately)
- **Community:** the project Discord (<https://discord.gg/xXUZFTuzSw>)
- **Contact:** <aaron@elideus.net>

---

*This document is a plain-language summary provided in good faith for a pre-alpha project.
It is not a contract or legal advice; the [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE) files are the operative legal terms.*
