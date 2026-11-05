# Rules of the Razor codebase

Read this before writing a line; reviewers check against it. It grows only on Trey's word: a proposed rule reaches him as one sentence. The spec (SPEC.md, section 7) says why each exists; this page only says what.

## Structure

1. A module exposes one small public surface and keeps the rest private. When a caller needs a second way in, deepen the module; do not widen it.
2. No abstraction before the second use. One caller, no trait; one implementation, no trait; a generic parameter earns its place by a second concrete type in the same commit.
3. Data is plain: structs and enums with public fields where the module owns the type, serialization derived where the type crosses a boundary. No builders for types with fewer than six fields; no getters over plain fields.
4. Modules talk only through their public surface. No reaching into a sibling's internals; no crate-visible escape hatch to dodge a design decision.
5. One definition per concept. A thing the codebase knows (a session id, a model name, an event) has one type in one place; a second definition is a bug.

## Errors

6. One error type per module: an enum with a variant per cause a caller can act on, and one catch-all variant carrying the underlying error and a sentence of context. Never `Result<_, String>`.
7. Every `unwrap`, `expect` and `panic!` outside the flows is a bug. Where an invariant truly cannot fail, the comment beside it names the invariant.
8. An error that reaches a person is one sentence: what failed, and what would work.

## Behaviour

9. Nothing blocks inside the loop or the terminal app's draw path: file and process I/O go through the async runtime or a thread of their own.
10. The record is read once per session and held as a cursor; a request is rendered from state, never from re-parsing the whole file.
11. Nothing unbounded in memory: every buffer, queue and in-memory log has a cap written beside it.
12. Every config key has a reader in the commit that adds it, and every key is live (re-read on change); otherwise it is not a key.
13. A behaviour switch is a config key; there are no compile-time features for behaviour.

## Names and text

14. Names are words the reader already has (`session`, `turn`, `record`, `child`). No coined names; the glossary in docs/concepts.md and the code's words in docs/contributing.md are the whole vocabulary, and a review refuses a new term.
15. A doc comment on a public item says what it guarantees, not what it does line by line. No comment restates the code.
16. Text a model reads (a tool description, a prompt section) is data in one place, versioned; it is never assembled from format strings scattered across modules, and Razor's own wording never mentions the model's thinking or reasoning, which Claude's filter refuses as reasoning extraction (Trey, 2026-10-29; a maker's pinned prompt is the maker's).

## Size

17. A file over 800 lines or a function over 80 is a review finding unless the reviewer writes why it is the right shape.
18. Each module's line budget is in the spec and the gate counts it; a lane that nears its budget cuts before it asks.
19. No code for a future need. If the spec does not name it and a flow does not exercise it, it is not written.

## Dependencies

20. A new crate needs one sentence in the module's design page: what it replaces and why it beats writing it. The spec already lists the large ones.
21. No "utils", "common" or "helpers" modules and no wrapper crates. A shared thing has a real name and one owner.

## Tests and tooling

22. No unit tests, no mocks, no fakes of our own code. The numbered flows are the tests, and so is the real-model evaluation wherever a checkout has it; a lane proves its module by running its flows.
23. Build tooling lives in `scripts/`; nothing in the product knows the gate exists.

## Process

24. A module's first commit is its public surface and a ten-line design note; the second is the implementation; then the rewrite by a fresh agent from the other family, and the smaller version ships.
25. A review finding is a deletion, a merge, a rename, or a question. Anything that adds goes to Trey as one sentence.
