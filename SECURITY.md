# Security

## Reporting a vulnerability

Please report it privately, through GitHub's private vulnerability reporting: on this
repository's **Security** tab, choose **Report a vulnerability**. Don't open a public
issue for it. You'll get a reply there, and a fix and an advisory once it's understood.

## In scope

- The hub (`atlas-hub`, `atlas-web`): getting in without an allowed Tailscale login,
  making changes without a passkey, cross-site requests, unsafe rendering.
- Updates and atlasd: getting a machine to apply something you didn't sign, replaying an
  old update, atlasd writing outside the entries it manages or running what it receives.
- Pairing and tokens, the install script the hub serves, and `deploy/install.sh`.

Out of scope: Tailscale, GitHub and the agents themselves, and anyone who already controls
the hub's machine or account (see the [security guide](documents/guide/security.md)).

## Maturity

Atlas is early software by one developer and hasn't had an outside security review. The
trust model is written down in `documents/architecture.md` and the guide, and some of its
protections are still planned rather than built: the atlasd builds the hub hands out are
checked by SHA-256 but not signed, and nothing scans content for risky instructions yet.
Treat it as you would any young tool that can change what your agents run.
