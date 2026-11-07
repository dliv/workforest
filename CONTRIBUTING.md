# Contributing

Bug reports, reproducible examples, and descriptions of real workflows are
especially welcome. Use an [issue form](https://github.com/dliv/workforest/issues/new/choose)
to describe what you tried, what happened, and what you expected. For feature
requests, explain the workflow and where it gets awkward; a proposed solution
is optional.

I generally implement changes myself to keep behavior and design consistent.
If you'd like to submit a PR, please discuss the approach in an issue first;
unsolicited PRs may be declined. A small reproducer or regression test can be
useful evidence even when the final implementation takes a different form.

Include the Workforest version, Git version, operating system, and the smallest
reproduction you can provide. Remove credentials, private remote URLs, and
sensitive paths from logs and examples. Verify any AI-assisted report before
submitting it; a concise reproduction is more useful than speculative analysis.

For agreed implementation work, [Development](docs/development.md) owns setup
and validation, including `just check`. [AGENTS.md](AGENTS.md) routes coding
agents to the repository's engineering instructions. Agents using the installed
tool should run `git forest agent-instructions`.
