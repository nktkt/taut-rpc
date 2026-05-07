//! `cargo taut inspect` — render the IR for humans.
//!
//! See `ROADMAP.md` Phase 5.
//!
//! Three output flavours are supported:
//!
//! - `table` (default): two ASCII tables — one for procedures, one for types.
//!   No external table crate is pulled in; the columns are aligned by the
//!   maximum width of each column. This is the format you eyeball during
//!   development.
//! - `json`: `serde_json::to_string_pretty(&ir)`. Suitable for piping to `jq`.
//! - `mermaid`: a `flowchart LR` block whose nodes are procedures and whose
//!   edges go from a procedure to its input/output type references. Useful
//!   for embedding the API surface in a markdown doc.
//!
//! The IR can come from either an existing JSON file (`--ir <PATH>`, default
//! `target/taut/ir.json`) or by spawning a compiled user binary with
//! `TAUT_DUMP_IR` set (`--from-binary <PATH>`); the two flags are mutually
//! exclusive. The from-binary flow mirrors `cargo taut gen` so that an
//! `inspect` invocation Just Works without a separate dump step.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, ValueEnum};

use taut_rpc::ir::{
    EnumDef, Field, Ir, Primitive, ProcKind, Procedure, TypeDef, TypeRef, TypeShape, Variant,
    VariantPayload,
};

/// Output format for `cargo taut inspect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InspectFormat {
    /// Human-readable ASCII tables (default).
    Table,
    /// Pretty-printed JSON, suitable for piping into `jq`.
    Json,
    /// Mermaid `flowchart LR` block, suitable for embedding in markdown.
    Mermaid,
}

/// Arguments for `cargo taut inspect`.
#[derive(Debug, Args)]
#[command(
    about = "Render the IR as a human-readable table, JSON, or Mermaid diagram.",
    long_about = "Loads target/taut/ir.json (or the path passed to --ir, or the IR \
                  dumped from --from-binary) and prints procedures and types in \
                  a format suitable for eyeballing during development."
)]
pub struct InspectArgs {
    /// Path to the IR JSON file to render.
    #[arg(
        long,
        value_name = "PATH",
        default_value = "target/taut/ir.json",
        conflicts_with = "from_binary"
    )]
    pub ir: PathBuf,

    /// Path to a compiled user binary. The binary is spawned with
    /// `TAUT_DUMP_IR=<--ir path>` so it writes its IR via
    /// `taut_rpc::dump_if_requested` and exits before binding any port.
    #[arg(long, value_name = "PATH", conflicts_with = "ir")]
    pub from_binary: Option<PathBuf>,

    /// Output format: `table` (default), `json`, or `mermaid`.
    #[arg(long, value_name = "KIND", value_enum, default_value_t = InspectFormat::Table)]
    pub format: InspectFormat,
}

/// Entry point for `cargo taut inspect`.
#[allow(clippy::needless_pass_by_value)] // owned `args` matches the clap-generated dispatch convention
pub fn run(args: InspectArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let ir_abs = if args.ir.is_absolute() {
        args.ir.clone()
    } else {
        cwd.join(&args.ir)
    };

    if let Some(bin) = args.from_binary.as_ref() {
        let bin_abs = if bin.is_absolute() {
            bin.clone()
        } else {
            cwd.join(bin)
        };
        dump_ir_from_binary(&bin_abs, &ir_abs)?;
    }

    if !ir_abs.exists() {
        return Err(anyhow!(
            "IR file not found at {}.\n\
             Build and run your crate first so the proc-macros can dump the IR \
             (e.g. `cargo run` with `taut_rpc::dump_if_requested(&router)` in \
             your `main`), or pass `--from-binary <PATH>` to spawn it for you.",
            ir_abs.display()
        ));
    }

    let raw = std::fs::read_to_string(&ir_abs)
        .with_context(|| format!("reading IR from {}", ir_abs.display()))?;
    let ir: Ir = serde_json::from_str(&raw)
        .with_context(|| format!("parsing IR JSON at {}", ir_abs.display()))?;

    let rendered = match args.format {
        InspectFormat::Table => render_table(&ir),
        InspectFormat::Json => render_json(&ir)?,
        InspectFormat::Mermaid => render_mermaid(&ir),
    };

    print!("{rendered}");
    if !rendered.ends_with('\n') {
        println!();
    }
    Ok(())
}

/// Spawn `bin` with `TAUT_DUMP_IR=<ir_target>` and wait for it to exit.
///
/// Mirrors the helper in `commands::gen` so that `cargo taut inspect
/// --from-binary <PATH>` works without a separate dump step.
fn dump_ir_from_binary(bin: &Path, ir_target: &Path) -> Result<()> {
    if !bin.exists() {
        bail!("--from-binary path does not exist: {}", bin.display());
    }

    if let Some(parent) = ir_target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create IR output directory: {}", parent.display())
            })?;
        }
    }

    let output = Command::new(bin)
        .env("TAUT_DUMP_IR", ir_target)
        .output()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = output
            .status
            .code()
            .map_or_else(|| "<terminated by signal>".to_string(), |c| c.to_string());
        return Err(anyhow!(
            "binary {} exited with status {} while dumping IR\n--- stderr ---\n{}",
            bin.display(),
            code,
            stderr.trim_end()
        ));
    }

    if !ir_target.exists() {
        bail!(
            "binary {} exited successfully but did not write IR to {}; \
             does its main() call `taut_rpc::dump_if_requested(&router)` \
             before any port binding?",
            bin.display(),
            ir_target.display()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

fn render_json(ir: &Ir) -> Result<String> {
    serde_json::to_string_pretty(ir).context("serializing IR to JSON")
}

// ---------------------------------------------------------------------------
// Mermaid
// ---------------------------------------------------------------------------

/// Render the IR as a `flowchart LR` block.
///
/// Each procedure becomes a node labelled `<name> / <kind>`. Each procedure
/// gets two outgoing edges, one for its input typeref and one for its output,
/// labelled accordingly. The right-hand side of the edge is a *terminal* node
/// — the rendered `TypeRef` — not a procedure-style node, so primitive sinks
/// (e.g. `string`, `User`) collapse together when reused across procedures.
fn render_mermaid(ir: &Ir) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str("flowchart LR\n");

    if ir.procedures.is_empty() {
        out.push_str("    %% no procedures registered\n");
        return out;
    }

    for p in &ir.procedures {
        let node = format!(
            "{name}[{name} / {kind}]",
            name = p.name,
            kind = kind_label(&p.kind)
        );
        let input = render_typeref_short(&p.input);
        let output = render_typeref_short(&p.output);
        let _ = writeln!(out, "    {node} -->|input| {input}");
        let _ = writeln!(out, "    {name} -->|output| {output}", name = p.name);
        for err in &p.errors {
            let _ = writeln!(
                out,
                "    {name} -->|error| {err}",
                name = p.name,
                err = render_typeref_short(err),
            );
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------------

/// Render the IR as two ASCII tables: procedures, then types.
fn render_table(ir: &Ir) -> String {
    let mut out = String::new();

    out.push_str("Procedures\n");
    out.push_str("----------\n");
    if ir.procedures.is_empty() {
        out.push_str("no procedures registered\n");
    } else {
        out.push_str(&render_proc_table(&ir.procedures));
    }
    out.push('\n');

    out.push_str("Types\n");
    out.push_str("-----\n");
    if ir.types.is_empty() {
        out.push_str("no types defined\n");
    } else {
        out.push_str(&render_type_table(&ir.types));
    }

    out
}

fn render_proc_table(procs: &[Procedure]) -> String {
    let header = ["Name", "Kind", "Method", "Input", "Output", "Errors"];
    let mut rows: Vec<[String; 6]> = Vec::with_capacity(procs.len());
    for p in procs {
        let errors = if p.errors.is_empty() {
            "-".to_string()
        } else {
            p.errors
                .iter()
                .map(render_typeref_short)
                .collect::<Vec<_>>()
                .join(", ")
        };
        rows.push([
            p.name.clone(),
            kind_label(&p.kind).to_string(),
            method_label(&p.http_method).to_string(),
            render_typeref_short(&p.input),
            render_typeref_short(&p.output),
            errors,
        ]);
    }
    render_grid(&header, &rows)
}

fn render_type_table(types: &[TypeDef]) -> String {
    let header = ["Name", "Shape", "Fields"];
    let mut rows: Vec<[String; 3]> = Vec::with_capacity(types.len());
    for t in types {
        let (shape, fields) = describe_shape(&t.shape);
        rows.push([t.name.clone(), shape, fields]);
    }
    render_grid(&header, &rows)
}

/// Render a single ASCII grid given column headers and rows.
///
/// Columns are left-aligned and padded to the max width seen in that column
/// (header included). A separator line of `-` follows the header. Cells with
/// embedded newlines are left as-is (Mermaid/JSON paths don't go through
/// here, and we deliberately keep cells single-line for procs/types tables).
fn render_grid<const N: usize>(header: &[&str; N], rows: &[[String; N]]) -> String {
    let mut widths: [usize; N] = [0; N];
    for (i, h) in header.iter().enumerate() {
        widths[i] = h.len();
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if cell.len() > widths[i] {
                widths[i] = cell.len();
            }
        }
    }

    let mut out = String::new();
    push_row(&mut out, header.iter().copied(), &widths);
    push_separator(&mut out, &widths);
    for row in rows {
        push_row(&mut out, row.iter().map(String::as_str), &widths);
    }
    out
}

fn push_row<'a, I, const N: usize>(out: &mut String, cells: I, widths: &[usize; N])
where
    I: IntoIterator<Item = &'a str>,
{
    let mut first = true;
    for (i, cell) in cells.into_iter().enumerate() {
        if !first {
            out.push_str(" | ");
        }
        first = false;
        out.push_str(cell);
        // Pad to the column width with spaces. We don't pad the last column
        // since trailing whitespace would just bloat the output.
        if i + 1 < N {
            for _ in cell.len()..widths[i] {
                out.push(' ');
            }
        }
    }
    out.push('\n');
}

fn push_separator<const N: usize>(out: &mut String, widths: &[usize; N]) {
    let mut first = true;
    for (i, w) in widths.iter().enumerate() {
        if !first {
            out.push_str("-+-");
        }
        first = false;
        for _ in 0..*w {
            out.push('-');
        }
        // Match `push_row`: don't extend trailing whitespace past the last col.
        let _ = i;
    }
    out.push('\n');
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn kind_label(k: &ProcKind) -> &'static str {
    match k {
        ProcKind::Query => "query",
        ProcKind::Mutation => "mutation",
        ProcKind::Subscription => "subscription",
    }
}

fn method_label(m: &taut_rpc::ir::HttpMethod) -> &'static str {
    match m {
        taut_rpc::ir::HttpMethod::Post => "POST",
        taut_rpc::ir::HttpMethod::Get => "GET",
    }
}

fn primitive_label(p: Primitive) -> &'static str {
    match p {
        Primitive::Bool => "bool",
        Primitive::U8 => "u8",
        Primitive::U16 => "u16",
        Primitive::U32 => "u32",
        Primitive::U64 => "u64",
        Primitive::I8 => "i8",
        Primitive::I16 => "i16",
        Primitive::I32 => "i32",
        Primitive::I64 => "i64",
        Primitive::U128 => "u128",
        Primitive::I128 => "i128",
        Primitive::F32 => "f32",
        Primitive::F64 => "f64",
        Primitive::String => "string",
        Primitive::Bytes => "bytes",
        Primitive::Unit => "()",
        Primitive::DateTime => "DateTime",
        Primitive::Uuid => "Uuid",
    }
}

/// Render a [`TypeRef`] as a compact human-readable string, roughly mirroring
/// Rust syntax. This is the form used in both the table cells and the right-
/// hand side of mermaid edges. It is *not* TypeScript — that's
/// `taut_rpc::type_map::render_type`'s job.
fn render_typeref_short(t: &TypeRef) -> String {
    match t {
        TypeRef::Primitive(p) => primitive_label(*p).to_string(),
        TypeRef::Named(name) => name.clone(),
        TypeRef::Option(inner) => format!("Option<{}>", render_typeref_short(inner)),
        TypeRef::Vec(inner) => format!("Vec<{}>", render_typeref_short(inner)),
        TypeRef::Map { key, value } => format!(
            "Map<{}, {}>",
            render_typeref_short(key),
            render_typeref_short(value)
        ),
        TypeRef::Tuple(elems) => {
            if elems.is_empty() {
                "()".to_string()
            } else {
                let inner: Vec<String> = elems.iter().map(render_typeref_short).collect();
                format!("({})", inner.join(", "))
            }
        }
        TypeRef::FixedArray { elem, len } => {
            format!("[{}; {}]", render_typeref_short(elem), len)
        }
    }
}

/// Describe a [`TypeShape`] as a `(shape_label, fields_summary)` pair for the
/// types table.
fn describe_shape(s: &TypeShape) -> (String, String) {
    match s {
        TypeShape::Struct(fields) => ("struct".to_string(), describe_fields(fields)),
        TypeShape::Enum(EnumDef { tag, variants }) => {
            (format!("enum (tag={tag})"), describe_variants(variants))
        }
        TypeShape::Tuple(elems) => {
            let inner: Vec<String> = elems.iter().map(render_typeref_short).collect();
            ("tuple".to_string(), format!("({})", inner.join(", ")))
        }
        TypeShape::Newtype(inner) => ("newtype".to_string(), render_typeref_short(inner)),
        TypeShape::Alias(inner) => ("alias".to_string(), render_typeref_short(inner)),
    }
}

fn describe_fields(fields: &[Field]) -> String {
    if fields.is_empty() {
        return "(empty)".to_string();
    }
    fields
        .iter()
        .map(|f| {
            let opt = if f.optional { "?" } else { "" };
            format!("{}{}: {}", f.name, opt, render_typeref_short(&f.ty))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe_variants(variants: &[Variant]) -> String {
    if variants.is_empty() {
        return "(empty)".to_string();
    }
    variants
        .iter()
        .map(|v| match &v.payload {
            VariantPayload::Unit => v.name.clone(),
            VariantPayload::Tuple(elems) => {
                let inner: Vec<String> = elems.iter().map(render_typeref_short).collect();
                format!("{}({})", v.name, inner.join(", "))
            }
            VariantPayload::Struct(fields) => {
                format!("{} {{ {} }}", v.name, describe_fields(fields))
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use taut_rpc::ir::HttpMethod;

    fn sample_ir() -> Ir {
        Ir {
            ir_version: Ir::CURRENT_VERSION,
            procedures: vec![
                Procedure {
                    name: "create_user".to_string(),
                    kind: ProcKind::Mutation,
                    input: TypeRef::Named("CreateUser".to_string()),
                    output: TypeRef::Named("User".to_string()),
                    errors: vec![TypeRef::Named("UserError".to_string())],
                    http_method: HttpMethod::Post,
                    doc: None,
                },
                Procedure {
                    name: "ping".to_string(),
                    kind: ProcKind::Query,
                    input: TypeRef::Primitive(Primitive::Unit),
                    output: TypeRef::Primitive(Primitive::String),
                    errors: vec![],
                    http_method: HttpMethod::Get,
                    doc: None,
                },
            ],
            types: vec![
                TypeDef {
                    name: "User".to_string(),
                    doc: None,
                    shape: TypeShape::Struct(vec![
                        Field {
                            name: "id".to_string(),
                            ty: TypeRef::Primitive(Primitive::U64),
                            optional: false,
                            undefined: false,
                            doc: None,
                            constraints: vec![],
                        },
                        Field {
                            name: "name".to_string(),
                            ty: TypeRef::Primitive(Primitive::String),
                            optional: false,
                            undefined: false,
                            doc: None,
                            constraints: vec![],
                        },
                    ]),
                },
                TypeDef {
                    name: "CreateUser".to_string(),
                    doc: None,
                    shape: TypeShape::Struct(vec![Field {
                        name: "name".to_string(),
                        ty: TypeRef::Primitive(Primitive::String),
                        optional: false,
                        undefined: false,
                        doc: None,
                        constraints: vec![],
                    }]),
                },
                TypeDef {
                    name: "UserError".to_string(),
                    doc: None,
                    shape: TypeShape::Enum(EnumDef {
                        tag: "type".to_string(),
                        variants: vec![
                            Variant {
                                name: "NotFound".to_string(),
                                payload: VariantPayload::Unit,
                            },
                            Variant {
                                name: "Conflict".to_string(),
                                payload: VariantPayload::Tuple(vec![TypeRef::Primitive(
                                    Primitive::String,
                                )]),
                            },
                        ],
                    }),
                },
            ],
        }
    }

    #[test]
    fn table_format_renders_procedures_and_types() {
        let ir = sample_ir();
        let out = render_table(&ir);

        // Section headers
        assert!(
            out.contains("Procedures"),
            "missing Procedures header:\n{out}"
        );
        assert!(out.contains("Types"), "missing Types header:\n{out}");

        // Column headers for proc table
        for col in ["Name", "Kind", "Method", "Input", "Output", "Errors"] {
            assert!(out.contains(col), "missing proc column {col}:\n{out}");
        }
        // Column headers for type table
        for col in ["Shape", "Fields"] {
            assert!(out.contains(col), "missing type column {col}:\n{out}");
        }

        // Procedure rows
        assert!(out.contains("create_user"), "missing proc name:\n{out}");
        assert!(out.contains("mutation"), "missing kind:\n{out}");
        assert!(out.contains("POST"), "missing method:\n{out}");
        assert!(out.contains("CreateUser"), "missing input typeref:\n{out}");
        assert!(out.contains("UserError"), "missing error typeref:\n{out}");
        assert!(out.contains("ping"), "missing second proc:\n{out}");
        assert!(out.contains("query"), "missing query kind:\n{out}");
        assert!(out.contains("GET"), "missing GET method:\n{out}");

        // Type rows
        assert!(out.contains("struct"), "missing struct shape:\n{out}");
        assert!(out.contains("id: u64"), "missing field rendering:\n{out}");
        assert!(out.contains("enum"), "missing enum shape:\n{out}");
        assert!(out.contains("NotFound"), "missing variant:\n{out}");
        assert!(
            out.contains("Conflict(string)"),
            "missing tuple variant:\n{out}"
        );

        // Empty IR uses placeholder messages
        let empty = render_table(&Ir::empty());
        assert!(empty.contains("no procedures registered"), "got:\n{empty}");
        assert!(empty.contains("no types defined"), "got:\n{empty}");

        // Alignment sanity check: every non-empty line of the procedures
        // section under the header has the same number of '|' separators
        // (5 separators -> 6 columns).
        let proc_block = out.split("\nTypes").next().expect("Procedures section");
        let pipe_counts: Vec<usize> = proc_block
            .lines()
            .skip(2) // "Procedures" + "----------"
            .filter(|l| !l.is_empty() && !l.starts_with('-'))
            .map(|l| l.matches('|').count())
            .collect();
        assert!(
            pipe_counts.iter().all(|&c| c == 5),
            "pipe counts not uniform: {pipe_counts:?}\nblock:\n{proc_block}"
        );
    }

    #[test]
    fn json_format_pretty_prints() {
        let ir = sample_ir();
        let out = render_json(&ir).expect("render json");

        // Pretty-printed JSON has newlines and 2-space indent.
        assert!(out.contains('\n'), "expected pretty JSON, got: {out}");
        assert!(
            out.contains("  \"ir_version\""),
            "expected indented ir_version, got: {out}"
        );
        assert!(out.contains("\"create_user\""), "missing proc name: {out}");
        assert!(out.contains("\"User\""), "missing type name: {out}");

        // Round-trips: pretty JSON parses back to the same IR.
        let back: Ir = serde_json::from_str(&out).expect("parse back");
        assert_eq!(back, ir);
    }

    #[test]
    fn mermaid_format_emits_flowchart_block() {
        let ir = sample_ir();
        let out = render_mermaid(&ir);

        assert!(
            out.starts_with("flowchart LR\n"),
            "missing flowchart header:\n{out}"
        );

        // Procedure node + edges per the spec example. The first edge for a
        // procedure carries the node-label syntax (`create_user[create_user /
        // mutation]`), subsequent edges reuse the bare node id.
        assert!(
            out.contains("create_user[create_user / mutation] -->|input| CreateUser"),
            "missing input edge:\n{out}"
        );
        assert!(
            out.contains("create_user -->|output| User"),
            "missing output edge:\n{out}"
        );
        assert!(
            out.contains("create_user -->|error| UserError"),
            "missing error edge:\n{out}"
        );
        assert!(
            out.contains("ping[ping / query] -->|input|"),
            "missing second proc node:\n{out}"
        );
        assert!(
            out.contains("ping -->|output| string"),
            "missing second proc output edge:\n{out}"
        );

        // Empty IR still produces a valid (if degenerate) flowchart block.
        let empty = render_mermaid(&Ir::empty());
        assert!(empty.starts_with("flowchart LR\n"), "got:\n{empty}");
        assert!(
            empty.contains("no procedures registered"),
            "missing empty placeholder:\n{empty}"
        );
    }
}
