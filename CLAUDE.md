# king-tongue-2

## Agent skills

### Issue tracker

Issues live as GitHub issues on `BunnyEncoder20/king-tongue-2`; use `gh`. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context: `GLOSSARY.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Standards and setup

Review rules, test discipline, and the deliberately open decisions: `CODING_STANDARDS.md`.

Once per clone, to run fmt and workspace clippy before each commit:

```sh
git config core.hooksPath scripts/hooks
```

Start a ticket in its own worktree, based on the integration tip rather than on
whatever ref happens to be checked out:

```sh
scripts/start-ticket.sh <ticket-number> <slug>
```
