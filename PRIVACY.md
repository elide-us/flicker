# Privacy

> **Status: DRAFT — confirm the controller/contact details and have this reviewed before publishing.**
> Data controller: **The Elideus Group**. Project: **flicker** / **Prism Alpha**, the client of the **ClayEngine** ecosystem.
> Applies to: the **v0.2.x** release line. Last updated: 2026-08-18.

**Privacy-first is a design goal of this platform, not an afterthought.** flicker is a
video game and a work of fiction. We collect as little about you as possible, we tokenize
the little identity we do keep, and we aim to be aligned with the EU **General Data
Protection Regulation (GDPR)** and comparable regimes. This document explains, in plain
language, what is and isn't collected.

See also: [SECURITY.md](SECURITY.md) and [WARRANTY.md](WARRANTY.md).

## The short version

- **Single-player is offline-first and collects nothing about you.** No account is required
  to play offline. On its own the client makes one network call: an **anonymous version
  check** against the public GitHub Releases API — no login, no identifiers, silent if
  you're offline.
- **Online features talk to our own servers, not data brokers.** Chat and multiplayer
  world-state connect to ClayEngine's own game servers, and optional account features
  authenticate through our web backend — only when you choose to go online.
- **Sign-in (optional, for online features) is delegated to third parties.** You
  authenticate through **Microsoft, Google, or Discord**. **We never see or store your
  password.**
- **Your identity is stored tokenized.** Internal and public identifiers are random
  tokens; your identity-provider account is stored only as a one-way token. A database
  leak alone cannot reveal who you are with the provider.
- **We keep only minimal, opt-in personal data** — essentially a display name, an email,
  and optionally an avatar — and nothing is sold, rented, or used for advertising.
- **All in-game and world data is fictional.** Simulated planets, characters, and content
  are works of fiction and do not describe real people or places.

## Scope

This statement covers two distinct things:

1. **The game client** (the downloadable Prism Alpha application). Offline; see above.
2. **The online platform** (optional accounts, entitlements, and web services operated by
   The Elideus Group). Everything below about accounts applies here.

## The game client

Playing offline requires no account and collects no analytics, telemetry, or advertising
identifiers. On its own the client makes a single network request: an anonymous HTTPS `GET`
to the public GitHub Releases API to see whether a newer release exists, so the menu can
show an "update available" hint. It sends no user data and fails silently. Standard
technical data that any web request exposes (such as your IP address, visible to GitHub as
the host) is governed by that third party's policy, not ours.

**Online and multiplayer features** connect to **ClayEngine's own game servers** — a chat
service and live world-state synchronization — and, for account features, to our web
backend (**TheOracleRPC**). When you use them, only the data needed to provide them is
sent: for chat, the nickname you choose and the messages you send; for shared worlds, your
in-world position and input. These are our own servers, not third-party trackers, and this
traffic happens only when you choose to go online. These features are early — local and
developer-facing today, with server host discovery and authentication arriving from the web
backend — and this statement will be kept accurate as they come online.

## Accounts and identity (online platform)

### Federated sign-in — no passwords

Authentication is delegated to third-party identity providers (**Microsoft, Google,
Discord**) over OAuth 2.0 / OpenID Connect. We **do not** operate password logins and we
**do not** store, see, or handle your provider password. Unlinking your last provider is
equivalent to closing your account.

### Purely tokenized identity

The platform is designed so that identity is stored as tokens, never as raw provider data:

- Your **internal account id** is a random GUID that is never exposed publicly.
- Your **public id** (used anywhere your profile is referenced) is a *separate* random
  token that is **not derived from and cannot be reversed to** your internal id.
- The identity-provider's subject identifier is **normalized to a one-way token before it
  is stored** (a UUID5 derived using a secret namespace that is kept **out of the
  database**, in the environment). This means that even a full database leak, by itself,
  cannot reveal your underlying provider account.

### Data minimization — only opt-in details

We keep only what is needed to give you an account, and only what the provider returns and
you choose to share:

- a **display name**,
- an **email address** (used to operate your account; **private by default** — showing it
  publicly is an explicit opt-in toggle),
- optionally a **profile image / avatar**,
- your **entitlements and credits** (what features your account has access to), and
- operational **session tokens** and minimal device/session records needed to keep you
  signed in.

We do **not** collect: passwords, payment card numbers, government identifiers, postal
addresses, contact lists, biometric data, precise location, or cross-site advertising
profiles.

### What we do with it

We use this data only to authenticate you, remember your settings and entitlements, and
operate the service. **We do not sell or rent your personal data, and we do not use it for
third-party advertising or behavioral tracking.**

## All retained content is fictional

Beyond the small identity record above, the data the platform and game work with — worlds,
simulations, characters, personas, items, and generated content — is **fictional**. It is
authored or procedurally generated for a game and is not a description of, or a dossier
about, any real individual.

## Your rights (GDPR and similar)

Where GDPR (or a comparable law) applies, you have the right to:

- **Access & portability** — see the data on your account. Much of this is available to you
  directly through self-service account management.
- **Rectification** — correct your display name, email, and public-display preference at
  any time.
- **Erasure ("right to be forgotten")** — close your account. Closing an account removes
  the user-controlled personal data associated with it. *One exception, kept deliberately
  narrow:* a **one-way provider-identity token** is retained purely to prevent fraud and
  abusive re-registration. It contains no readable personal information and is not used to
  contact or profile you.
- **Restriction & objection** — ask us to limit or stop certain processing.
- **Withdraw consent** — unlink an identity provider, or turn off the public-email opt-in,
  at any time.

To exercise a right that isn't already available through self-service account management,
contact us (below).

### Legal bases

We rely on: **consent** (e.g. linking a provider, opting to display your email);
**performance of a contract** (operating the account you asked us to create); and narrow
**legitimate interests** (keeping the service secure and preventing fraud and abuse).

## Retention

Identity and account data is retained while your account is active. On account closure the
user-controlled personal data is removed; only the narrow anti-fraud token described above
persists. Operational logs and session records are kept only as long as needed to run and
secure the service.

## Processors and third parties

Operating the platform involves a small number of third parties acting on our behalf or as
independent controllers for their own part of the flow:

- **Identity providers** (Microsoft, Google, Discord) handle authentication and are
  independent controllers for that step under their own privacy policies.
- **Hosting / cloud infrastructure** stores the account data described above.

We do not share your personal data with these parties beyond what each function requires,
and we do not sell it to anyone.

## Children

The platform is not directed to children under the age of digital consent in their
jurisdiction (16 in much of the EU, subject to local law), and we do not knowingly collect
personal data from them. If you believe a child has provided personal data, contact us and
we will remove it.

## International transfers

Data may be processed in regions where our hosting and identity providers operate. Where
required, we rely on appropriate safeguards for cross-border transfers.

## Changes and contact

We may update this statement as the platform evolves; material changes will be reflected
here with a new "last updated" date.

- **Data controller:** The Elideus Group
- **Contact:** <aaron@elideus.net>
- **Community:** the project Discord (<https://discord.gg/xXUZFTuzSw>)

---

*This is a plain-language privacy statement provided in good faith for a pre-alpha project.
It describes the platform's design and intent and is not legal advice; the authoritative,
served policy for OAuth consent screens should be confirmed and legally reviewed before
publication.*
