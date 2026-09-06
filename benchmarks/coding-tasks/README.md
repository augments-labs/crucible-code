# Coding-task campaign

This directory owns the small, repository-authored fixtures used by the manual
`task-campaign.yml` workflow. `suite.json` is versioned and each task names:

- a unique `name`;
- a fixture directory copied for each binary;
- the prompt sent as one redirected Crucible turn;
- an independent shell command whose exit status decides the task.

Validate additions without a provider:

```bash
scripts/task-campaign.py validate --suite benchmarks/coding-tasks/suite.json
```

Run a campaign only with a disposable provider credential in the environment:

```bash
scripts/task-campaign.py run \
  --binary target/release/crucible \
  --model provider/model \
  --suite benchmarks/coding-tasks/suite.json \
  --label candidate \
  --output candidate.json
```

The runner records the agent and verifier exits, wall time, normalized input and
output usage plus per-currency cost from the session journal, and bounded diagnostic tails. It does not
claim a pass from model prose. Reports may contain model output and therefore
belong in workflow artifacts, not in the repository.

Fixtures and prompts must be independently authored for Crucible, small enough
to review here, and free of secrets, network dependencies, and external test
corpora. A verifier must decide the requested repository state without trusting
the agent's answer.
