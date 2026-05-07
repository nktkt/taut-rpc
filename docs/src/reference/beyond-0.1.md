# Beyond v0.1

The `0.1.0` release is intentionally narrow. Once it ships, the IR shape is
frozen for compatibility, but there is plenty of surface area left to explore
on top of it. This page collects the items currently on the speculative
roadmap, the rough timing we have in mind, and a sentence or two on the
*why* behind each.

Nothing here is a commitment. Everything here is open to design discussion,
and several items are explicit invitations for community work.

## Transports and I/O

### File uploads — likely v0.2

Multipart and resumable uploads on top of the existing procedure model, so
file inputs are first-class instead of a side channel. This is the most
common shape that doesn't fit cleanly into JSON-only request bodies, and
landing it early keeps users from inventing one-off REST endpoints next to
their typed RPC surface.

### CBOR / MessagePack transports — likely v0.3

A binary wire format option for callers that already pay for the codec on
both ends, particularly in mobile and embedded contexts. The IR is
transport-agnostic by design, so this should be additive rather than
disruptive — but we want a real workload to validate the framing before
committing.

### BigInt coercion in npm runtime — v0.2 fix, near-term

The current runtime round-trips `i64` / `u64` through `Number` in a few
edge cases. The fix is small (string-encode large integers, decode to
`bigint` on the client) and we expect to backport it into the v0.1 line if
it lands cleanly.

## Adapters and emitters

### Second router adapter — explorations welcome

axum is the only first-class server adapter today. An `actix-web` or
`salvo` adapter would prove that the `taut-rpc` core is genuinely
router-agnostic, and would unblock teams that have already standardised on
those stacks. This is a strong candidate for a community-led PR; talk to
us before starting so we can align on the trait surface.

### Thin OpenAPI emitter — for parallel REST surfaces

Some teams need a REST surface in addition to the typed RPC client, often
for partner integrations or non-TypeScript consumers. A "thin" emitter
would project a subset of the IR into an OpenAPI document, with the
explicit caveat that it does not round-trip back — taut-rpc remains the
source of truth.

### Devtools panel — community contribution candidate

A browser devtools panel that taps the runtime and shows the live call
stream, with request/response payloads and validation outcomes. The
runtime already exposes the hooks needed; what's missing is the UI and
the wiring. We'd love to see a community implementation here.

## Type system and validation

### Generic procedure support — design under research

Procedures with type parameters (`fn list<T: Item>(...) -> Vec<T>`) are
a common ask, but they don't lower cleanly into the IR without either
monomorphising at codegen time or introducing a generics layer to the
wire format. We're collecting concrete use cases before committing to a
direction.

### State extractor (`State<S>`) — likely v0.2

By a clear margin, the most-requested feature from early users: a
first-class extractor for shared application state, mirroring axum's
`State<S>`. Today users wire this through middleware; landing the
extractor properly removes a class of boilerplate and makes the macro
emit straight-line code.

### Conditional / cross-field validation — v0.2

`#[validate]` today only knows about single-field constraints. Real
forms routinely need rules like "if `country == "US"`, `zip` is
required". The plan is to add a struct-level validation hook that
runs after field-level checks and can produce typed errors keyed to
specific fields.

### Async validation — design TBD

Uniqueness checks against a database or a remote service can't be
expressed in the current synchronous validator trait. We want to add
async validation without making the common synchronous case more
expensive, which is the part that needs design work. Likely candidates:
a separate `AsyncValidate` trait, or an opt-in async pass.

## Already shipped

For reference, one item from the original "Beyond 0.1" list has already
landed:

- **MCP tools manifest emitter** (`cargo taut mcp`) — exposes a
  taut-rpc service to LLM agent harnesses without hand-writing
  schemas. See the [Roadmap](./roadmap.md) for status.

## Contributing

Most items above are open for community contributions. The ones marked
"explorations welcome" or "community contribution candidate" are the
clearest fits — pick one up, open a draft PR or a design issue, and we
will engage early.

Before starting on anything non-trivial, please read
[CONTRIBUTING.md](https://github.com/your-org/taut-rpc/blob/main/CONTRIBUTING.md)
for the workflow, the spec-first principle, and the review expectations.
A short design sketch in an issue is almost always faster than a large
PR that needs re-shaping; we would much rather argue about a paragraph
than about a thousand lines of code.

If you are unsure where to start, the highest-leverage near-term items
are the **State extractor**, **conditional validation**, and the
**BigInt coercion fix** — all three are well-scoped, have clear user
demand, and fit comfortably inside v0.2.
