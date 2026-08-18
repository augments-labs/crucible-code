# Tools

Nine tools, and the model chooses which one to call. Three of them only look —
`read`, `grep` and `glob` — and are never asked about. Three of them change
something — `edit`, `write` and `bash` — and ask until a rule or a mode answers
for you. Two of them leave your machine: `web_search` and `web_fetch` are asked
about in every mode but `fullAccess`, and appear only where the session has
something to answer them. The ninth changes nothing outside crucible:
`todo_write` puts down the plan, and you read it above the prompt.

- [What all seven hold themselves to](tools.md)
- [Reading and changing a file](files.md) — `read`, `edit`, `write`
- [Searching the tree](searching.md) — `grep`, `glob`
- [Running a command](commands.md) — `bash`
- [Reaching the web](web.md) — `web_search`, `web_fetch`
- [Writing down the plan](planning.md) — `todo_write`
