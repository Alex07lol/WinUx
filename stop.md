# STOP control

STOP=NO

Normal development and testing may proceed.

## When to set STOP=YES

Set `STOP=YES` at the top of this file and stop code modifications (and all
nonessential commands) when any of the following is true:

- the user requests a stop;
- a destructive or irreversible host action is proposed (partitioning, boot
  loader/EFI modification, firmware flashing, disk formatting, deleting user
  files, modifying an existing VM);
- privilege escalation or an unreviewed installer would be required;
- a critical license/provenance question is unresolved (for example, copying
  GPL kernel code or proprietary binaries into the project);
- contradictory architectural evidence could force a foundational redesign;
- success cannot be established and further changes would be speculative.

Do **not** set STOP just because a routine build or test failed. Diagnose it,
make a bounded evidence-driven fix, and record the result.

## What to do when stopping

1. Preserve the working tree; do not reset, clean or discard changes.
2. Update `progress.md` with the exact blocker, commands run, logs and the
   smallest next experiment.
3. Report why work stopped and what the user must decide.
4. Leave `STOP=YES` in place until the user resolves or authorizes resumption.

## Safe resume procedure

1. Confirm this file says `STOP=NO` (the user or a resolved blocker may change it).
2. Read `progress.md`.
3. Inspect `git status`, the newest `artifacts/qemu-serial.log` and relevant
   build/test output.
4. Identify the outstanding blocker and the single best next action.
5. Continue with the smallest verifiable step and update `progress.md`.

## Notes

This environment is case-insensitive; `STOP.md` resolves to this file. On a
case-sensitive filesystem, add a pointer file rather than a divergent duplicate.
