# MCP (Model Context Protocol)

taut-rpc can emit a [Model Context Protocol] manifest from your IR, turning every
query and mutation into a tool that an LLM agent can call. Phase 5 ships the
manifest emitter as a `cargo taut mcp` subcommand; you wire the resulting JSON
into any MCP-compatible host (Claude Desktop, your own server, etc.).

[Model Context Protocol]: https://modelcontextprotocol.io

## What MCP is

MCP is a small JSON-RPC-shaped standard that lets an LLM discover and invoke
external **tools**. A host (the LLM client) asks a server for its `tools/list`,
gets back a list of named tools with JSON Schema for their inputs, and then
calls them via `tools/call`. The protocol is intentionally narrow: a tool is a
named, schema-described, request/response function.

## Why taut-rpc supports it

Every `#[rpc]` query and mutation already *is* a request/response function with
a typed input. The IR carries everything an MCP manifest needs:

- a unique procedure name → tool `name`,
- the rustdoc string → tool `description`,
- the input `TypeRef` (plus transitively-reachable `TypeDef`s) → tool
  `inputSchema` as JSON Schema.

So the manifest emitter is a pure IR-to-JSON transformation — no extra
annotations, no second source of truth. If you've defined a query, an LLM can
call it.

Subscriptions don't fit the MCP shape (it has no streaming primitive) and are
skipped by default.

## Generating the manifest

From the IR file the proc-macros leave at `target/taut/ir.json`:

```bash
cargo run -p taut-rpc-cli -- taut mcp --out tools.json
```

Or, if you'd rather the CLI run your binary for you and dump a fresh IR first:

```bash
cargo build -p yourapp
cargo run -p taut-rpc-cli -- taut mcp \
    --from-binary target/debug/yourapp \
    --out tools.json
```

`--from-binary` spawns the binary with `TAUT_DUMP_IR` set, lets
`taut_rpc::dump_if_requested` write the IR, then reads it back. The binary
exits before binding any port — see [Getting started](./getting-started.md)
for the `dump_if_requested` call site.

Use `--out -` to write the manifest to stdout (handy for piping into `jq` or
into a wrapper script).

## The output shape

The manifest is a single JSON object with one field, `tools`, mirroring the
MCP `tools/list` response:

```json
{
  "tools": [
    {
      "name": "create_user",
      "description": "Create a new user. Returns the freshly-minted UserId.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "input": { "$ref": "#/$defs/CreateUserInput" }
        },
        "required": ["input"],
        "$defs": {
          "CreateUserInput": {
            "type": "object",
            "properties": {
              "email": { "type": "string" },
              "display_name": { "type": "string" }
            },
            "required": ["email", "display_name"],
            "additionalProperties": false
          }
        }
      }
    }
  ]
}
```

A few things to notice:

- The outer `inputSchema` is **always** an object with a single `input` field,
  even when the procedure takes a primitive. This mirrors taut-rpc's wire
  envelope (`{"input": <value>}`) — it's the same JSON the LLM will be sending
  over HTTP — and satisfies MCP's requirement that `inputSchema.type` be
  `"object"`.
- All named types are pulled into `$defs` and referenced via `$ref`. Reachability
  is transitive and terminates on cycles, so recursive types (a `Tree` whose
  children are `Vec<Tree>`) emit one `$defs/Tree` entry that references itself.
- Schemas use **JSON Schema Draft 2020-12** keywords: `prefixItems` for tuples,
  `const` for enum tags, `oneOf` for tagged unions, `additionalProperties: false`
  on closed structs.
- `Option<T>` collapses into a nullable type (`"type": ["string", "null"]`) when
  the inner schema has a single primitive type, and into a `oneOf` wrapper
  otherwise.

There is no `description` field if the procedure has no rustdoc — MCP hosts
treat the absence as "no description" rather than blank.

## Filtering

By default the manifest contains queries and mutations only. To include
subscriptions:

```bash
cargo run -p taut-rpc-cli -- taut mcp --include-subscriptions --out tools.json
```

The streaming nature is invisible at the manifest layer — a subscription tool
will look identical to a query, and an MCP host calling it will get the *first*
frame as the response. Use this only if you have a specific MCP host that
understands the convention.

The `--bigint-strategy` flag controls how `u64` / `i64` / `u128` / `i128` are
rendered:

- `native` (default) — `{"type": "integer"}`. Simple, but most LLM tool callers
  produce numbers via JavaScript and lose precision past 2^53.
- `as-string` — `{"type": "string", "pattern": "^-?\\d+$"}`. Forces the LLM to
  emit a string like `"9007199254740993"`. Recommended if any of your inputs
  carry IDs or counts that exceed 53 bits.

## Wiring into an MCP server

The manifest is a *description* of your tools — you still need a tiny adapter
that, on `tools/call`, invokes the corresponding taut-rpc procedure. The
shortest path is to use the published `@modelcontextprotocol/sdk`:

```ts
import { Server } from "@modelcontextprotocol/sdk/server";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio";
import manifest from "./tools.json" with { type: "json" };
import { createClient } from "taut-rpc/client";
import type { Procedures } from "./api.gen";

const rpc = createClient<Procedures>({ url: "http://localhost:3000/rpc" });

const server = new Server({ name: "yourapp", version: "0.1.0" }, {
    capabilities: { tools: {} },
});

server.setRequestHandler("tools/list", async () => manifest);

server.setRequestHandler("tools/call", async (req) => {
    const { name, arguments: args } = req.params;
    // `args.input` matches the wire envelope; just forward it.
    const result = await (rpc as any)[name].query(args.input);
    return { content: [{ type: "text", text: JSON.stringify(result) }] };
});

await server.connect(new StdioServerTransport());
```

Two things to note:

1. `tools/list` is just the manifest, served verbatim. taut-rpc has already
   done all the work.
2. `tools/call`'s `arguments` field is the same `{"input": ...}` envelope the
   manifest's `inputSchema` declared. Forward it straight to the generated
   client.

For mutations, swap `.query(...)` for `.mutate(...)`. If you want to expose
both kinds without branching, lean on the IR — the `kind` field tells you
which method to call — or generate the dispatch from the manifest at startup.

## Limitations

A few rough edges in Phase 5:

- **Errors aren't surfaced as MCP errors.** When a procedure returns
  `Err(MyError)`, the adapter above currently ships the error as JSON content
  rather than as an MCP-level `isError: true` response. The result is that a
  failing tool call looks "successful" to the host but contains an error
  payload. A future phase will lift the IR's `errors` list into structured MCP
  errors.
- **Custom predicates are dropped.** Constraints declared via
  `taut_rpc::constraints::*` that boil down to JSON Schema keywords
  (`minLength`, `maximum`, `pattern`, etc.) are preserved, but arbitrary
  Rust-side `#[validate(custom = ...)]` predicates have no JSON Schema
  representation and are silently elided. The procedure will still validate
  them at call time; the LLM just won't know in advance.
- **No `outputSchema`.** MCP 2025-06-18 added an optional `outputSchema` field;
  the current emitter only writes `inputSchema`. Output shapes are still
  available in the IR — wiring them in is straightforward and tracked for a
  follow-up.
- **Subscriptions are second-class.** As noted above, including them is best-
  effort: only the first frame is observable through standard MCP.

## See also

- [`crates/taut-rpc-cli/src/mcp.rs`](https://github.com/taut-rpc/taut-rpc/blob/main/crates/taut-rpc-cli/src/mcp.rs)
  — the manifest emitter, with extensive unit tests covering the schema shape.
- [The IR](../concepts/ir.md) — the input to the emitter.
- [Type mapping](../concepts/type-mapping.md) — how Rust types become wire JSON,
  which is the same shape the JSON Schema describes.
- [MCP specification](https://spec.modelcontextprotocol.io/) — the protocol
  this manifest targets.
