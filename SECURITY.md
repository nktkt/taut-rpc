# Security Policy

The taut-rpc maintainers take security seriously. This document describes how
to report vulnerabilities, which versions receive fixes, and what we do (and do
not) treat as a security issue.

## Reporting a Vulnerability

Please report suspected vulnerabilities privately. Do not open a public GitHub
issue, discuss the bug in a public PR, or post details on social media until a
fix has been released and coordinated disclosure has happened.

You have two options:

1. **Email** <!-- TODO: replace this placeholder address with a real, monitored
   inbox before the project is published. -->
   `security@taut-rpc.dev`. If you want an encrypted channel, ask in your
   first message and we will exchange a key out of band.

2. **GitHub Security Advisories.** While taut-rpc is pre-1.0, private reports
   through GitHub Security Advisories are also welcome and arguably easier to
   coordinate on:
   <https://github.com/nktkt/taut-rpc/security/advisories>

When you report, please include:

- A description of the issue and the impact you believe it has.
- A minimal reproduction (code, IR JSON, generated TS, etc.).
- The crate, version, and commit SHA you tested against.
- Any suggested mitigation, if you have one.

We will acknowledge your report within 5 business days and will keep you
updated as we triage and fix the issue.

## Supported Versions

taut-rpc is pre-1.0 and moves quickly. Only the **latest 0.x release** receives
security fixes. Older 0.x lines are not patched; please upgrade to the current
release before reporting.

| Version       | Supported          |
| ------------- | ------------------ |
| latest 0.x    | yes                |
| older 0.x     | no                 |

Once 1.0 ships, this table will be updated to reflect the supported stable
lines.

## What We Consider a Security Issue

The following classes of bugs are in scope. If you find something that fits, or
that you believe is morally equivalent, please report it:

- **Authentication or authorization bypass in middleware.** A request that
  should have been rejected by an `auth` middleware reaching a procedure
  handler, or a handler observing the wrong principal.
- **Codegen producing unsafe TypeScript.** The generator emitting code that is
  unsound (incorrect types that hide a runtime mismatch), that injects
  attacker-controlled identifiers, or that produces TS that escapes intended
  sandboxing.
- **IR JSON deserialization flaws.** A crafted IR document that lets an
  attacker forge procedure metadata, smuggle procedures the server did not
  declare, or otherwise convince a client/codegen tool that the server's
  surface is something it is not.
- **Validation skipping bugs.** Any path through the runtime where input
  validation declared by a procedure is bypassed, partially applied, or
  silently downgraded.

## What We Do Not Consider a Security Issue

The following are explicitly out of scope. Reports about these will generally
be closed with a pointer to this section; we may still fix them as ordinary
bugs, but they will not be treated as embargoed security issues.

- **Denial of service via large or expensive payloads.** taut-rpc does not
  attempt to be a hardened public-internet edge. Configure your reverse proxy
  (request size limits, connection limits, timeouts, rate limiting) to handle
  this. Pathological allocator behavior triggered only by huge inputs is also
  out of scope.
- **Schema-design choices made by the user.** If your schema accepts a field
  that you then trust without further checks, that is a design issue in the
  application, not a vulnerability in taut-rpc. The same applies to choosing
  weak auth, exposing internal procedures, or wiring middleware in the wrong
  order.
- **Vulnerabilities in third-party dependencies** that do not actually affect
  taut-rpc's exposed surface. Please report those upstream; we will pick up
  fixes through normal dependency updates.

If you are not sure whether something is in scope, report it anyway and we
will figure it out together.

## Disclosure Timeline

Our target is **90 days from the initial report to public disclosure.** Within
that window we aim to:

1. Acknowledge the report (≤ 5 business days).
2. Confirm or refute the issue and agree on severity with the reporter.
3. Develop and review a fix.
4. Cut a patched release and publish a security advisory crediting the
   reporter (unless they prefer to remain anonymous).

If active mitigation work is in progress at day 90 (for example, a fix is
written but not yet released, or coordinated disclosure with a downstream
project is still pending), we may extend the embargo. Any extension will be
discussed with the reporter rather than imposed unilaterally.

Thank you for helping keep taut-rpc and its users safe.
