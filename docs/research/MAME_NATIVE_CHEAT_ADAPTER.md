# MAME native cheat adapter

Status: local/user-import only; no database download or bundled cheat data.

## Format and source of truth

MAME’s native format is an XML document rooted at `mamecheat version="1"`. A
`cheat` has a description, optional comment and parameters, and one or more
`script` elements. Scripts contain `action` debugger expressions and optional
output/argument expressions. Script states are `on`, `off`, `run`, or
`change(...)`. These are debugger expressions evaluated by MAME; EmuWiz never
executes or rewrites them into generic memory operations unless the expression
is the deliberately narrow numeric assignment form `address = value`.

The schema and expression contexts are documented in MAME’s source:
[src/frontend/mame/cheat.cpp](https://github.com/mamedev/mame/blob/master/src/frontend/mame/cheat.cpp).
MAME’s `-cheatpath` option accepts one or more search paths, and the asset
search documentation shows the per-machine shortname XML lookup convention:
[command-line options](https://docs.mamedev.org/commandline/commandline-all.html)
and [asset search paths](https://docs.mamedev.org/usingmame/assetsearch.html).

## Identity and readiness

The selected MAME machine shortname is authoritative. The adapter requires a
safe, exact shortname supplied by the caller; a title, filename, parent set, or
clone relationship is not promoted to an unattended target match. Parent/clone
automatic inheritance is intentionally not assumed. MAME version metadata is
recorded when the caller has it, but absence of a version does not invent a
compatibility claim.

Readiness is typed: `ReadyNative` means all actions are in the supported narrow
form; `ReadyWithOpaqueNativeOps` means the XML is preservable but contains
native expressions EmuWiz does not interpret; `WrongMachine`, `Malformed`,
`UnsupportedExpression`, and `Ambiguous` remain blocking/diagnostic states.

## Safety and persistence

Parsing is bounded by file size, nesting depth, cheat/parameter/script/action
counts and text length. DOCTYPE, ENTITY, SYSTEM and PUBLIC declarations are
refused, so external entities are not resolved. XML-defined expressions are
not executed. Rendering is deterministic and merge preserves unrelated cheats
and avoids duplicate entries.

The adapter stages `<machine>.xml` beneath a caller-owned staging directory and
builds the existing shared preview/transaction plan for the explicit local
MAME cheat root. Destination mutation therefore remains behind the normal
preview, confirmation, atomic publication, backup, history and rollback path.
The shared adapter is recorded as `Mame`; it is not relabelled as RetroArch or
as a generic mod.

MAME XML does not provide a portable persistent per-cheat enabled-state contract
for this adapter. Installing a definition reports `DefinitionInstalled` and
`RuntimeEnableRequired`; the user enables it through MAME’s runtime cheat UI.
EmuWiz does not silently change a global MAME setting. Exact rollback is the
shared transaction’s responsibility and refuses destructive removal after an
external destination change.

## GUI and legal boundary

The shared preview surface can describe the adapter as “MAME cheat”, show the
machine, native XML definition, understood versus opaque operations, and the
runtime-enable warning. No database browser or download control is added.
Inputs are local files or user-created definitions; provenance is retained as
`LocalImport`, `UserCreated`, `ExistingMameFile`, or `UnknownExternal`. MAME’s
external cheat archives are not treated as redistributable data.

## Limits

This is not a MAME expression interpreter, ROM repair tool, or cheat database.
Complex conditions, memory banking, scripting, outputs, parameters with runtime
semantics, and version-specific behavior remain native/opaque. They are
preserved for native MAME use but are not represented as generic compatibility
ranges.
