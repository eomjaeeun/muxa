# Repo-local skills

This checkout ships spec files for Claude Code Skills under `skills/<name>/SKILL.md`.
They are **not** auto-discovered from that path — Claude Code only picks up skills
from `.claude/skills/<name>/SKILL.md` (which is gitignored here, since it's normally
per-machine local state).

When the user asks to **add**, **install**, or **set up** one of these skills (e.g.
"muxa-bridge skill 추가합시다" / "add the muxa-bridge skill"), do this immediately,
without further confirmation:

```bash
mkdir -p .claude/skills/<name>
cp skills/<name>/SKILL.md .claude/skills/<name>/SKILL.md
```

Then the skill is available for the rest of this session (and every session in this
checkout going forward) exactly as if it had been installed by hand.

## Available skills

- **muxa-bridge** (`skills/muxa-bridge/SKILL.md`) — bridges a `claude` session
  running inside a docker container to the host's `muxa` daemon over a socat
  tunnel, so the host's `muxa watch` tracks it like a native tmux pane. Covers
  first-time setup, recovering after a reboot, and adding a new container.
