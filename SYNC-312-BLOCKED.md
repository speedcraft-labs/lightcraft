# #312: LightCraft sync blocked by ambiguous persisted process version

Attempt: fork main 09eb4c56accd9dc0e405510d099fee10d6a68f55 + upstream main f6b57b0b59f45f3cfae1fb8d26a834e336be5a11 (34 ahead / 169 behind before merge).

The fork's Process::V5 is the variance-calibrated recursive Gaussian from #176. Upstream's Process::V5 is camera highlight roll-off; upstream also adds V6 colour noise reduction and V7 grain. ProcessVersion serializes as a transparent u32, including in catalogs and XMP sidecars; there is no process provenance in that field. Choosing either V5 interpretation silently changes renders of the other lineage. Renumbering the new Gaussian to V8 alone does not migrate already-written fork V5 values.

Needed policy: must existing fork V5 edits retain their appearance, or may they adopt upstream V5 semantics? Preserving them needs an explicit lineage/migration design, not only a conflict resolution or startup benchmark. No library/catalog/sidecar has been modified, and no LightCraft sync PR or benchmark claim is being made.

Conflicts: .github/workflows/ci.yml, .github/workflows/release.yml, AGENTS.md, crates/catalog/src/tests_log_thread.rs, crates/develop/src/settings.rs, crates/gpu/src/render.rs, crates/gpu/tests/it/equivalence.rs, crates/pipeline/src/light_v3.rs, crates/pipeline/src/local.rs, crates/pipeline/src/tests_light.rs, docs/process-versions.md.

Reproduce in an isolated worktree from the fork SHA above, then `git merge --no-ff f6b57b0b59f45f3cfae1fb8d26a834e336be5a11`. The attempted merge's log, conflict patch and failure issue are kept at wormhole:/mnt/bulk/backups/speedcraft-labs/tickets/312-upstream-sync/20261109T220959Z. Follow-up conflict-index metadata is kept with the final #312 backup. See https://github.com/speedcraft-labs/hyper/issues/315.

DeckCraft succeeded separately: https://github.com/speedcraft-labs/deckcraft/pull/5 and https://github.com/speedcraft-labs/hyper/pull/314 are merged. LightCraft remains at its original fork main.
