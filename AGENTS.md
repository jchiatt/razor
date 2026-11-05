# Working on Razor

Read [SPEC.md](SPEC.md), [RULES.md](RULES.md), the glossary in [docs/concepts.md](docs/concepts.md#glossary) with the words used in the code in [docs/contributing.md](docs/contributing.md#words-used-in-the-code), and the relevant design pages before changing code.

## The bar

Trey, 2026-10-27: "I want each lane to go into this thinking like, 'This is going to be the best fucking code I've ever written, because this is the body I'm going to live in forever.' So, really thinking hard upfront and like, 'All right. What is the optimal way to structure this file, this code I need to write, given what needs to be built here and why, and it's part of the larger whole? What is the most performant, the fastest, the thing that will run the fastest, the thing that will use the least CPU or memory over time?' The thing that will be the most maintainable, is the most elegant, clear, simple, just excellent architecture. Reviewers being merciless about a refactor, cutting every line of code that doesn't absolutely have to be there. I want this to be amazing. I want this to be the best software this estate has ever made."

## Contributions

Keep changes small, review the diff, and run the flows relevant to the change. SPEC section 7 describes the design and review process; [docs/contributing.md](docs/contributing.md) says what a change needs and how to send it.

Run flows with `scripts/flows.sh 02 26` (numbers or names); it prepares the build, bridge packages and Python environment. `python3 scripts/size.py` checks the line budgets, and `scripts/gate.sh` runs every check a change must pass, on Linux.

What Razor does for its users is in [docs/README.md](docs/README.md).
