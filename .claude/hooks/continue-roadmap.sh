#!/bin/bash
cd "$CLAUDE_PROJECT_DIR" || exit 0
[ -f docs/BLOCKED.md ] && exit 0
sed '/^## Future phases/,$d' docs/roadmap.md | grep -q '^- \[ \]' || exit 0
echo '{"decision":"block","reason":"Unchecked tasks remain in docs/roadmap.md. Continue with the next one per the Autonomous Orchestration section of CLAUDE.md. If truly blocked, write docs/BLOCKED.md."}'