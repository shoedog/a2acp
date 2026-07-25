# R4 — Reproducible dependency, image, and release policy implementation plan

- **Status:** NOT STARTED
- **Prerequisite:** R3a–R3f merged with pinned/floating artifacts available for every claimed provider
- **Program source:** [`../../bridge-reliability.md`](../../bridge-reliability.md)
- **Release baseline:** [`../specs/2026-07-03-m2-release-engineering.md`](../specs/2026-07-03-m2-release-engineering.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Completion shape:** R4a full resolution pins, R4b image evidence, R4c promotion gate, R4d rollback/cadence

R4 ensures that a release claim applies to the distributable binary and image actually shipped. A
top-level npm adapter version, mutable base tag, or `latest` download URL is not a reproducible pin.

## R4a — lock the reader dependency graph

- **Branch:** `agent/reliability-r4a-reader-locks`

Replace global floating installation in `deploy/containers/reader.Containerfile` with checked-in,
reviewable resolution inputs:

- a reader `package.json` and lockfile containing exact ACP adapters and their resolved nested SDK/CLI
  packages;
- `npm ci`/equivalent lock-enforcing install under a fixed prefix;
- explicit executable links from that prefix;
- exact Kiro versioned artifact URLs per architecture plus committed SHA-256 checksums;
- immutable base-image digest while retaining a human-readable tag comment;
- pinned package-manager/runtime versions required to reproduce the install.

Build fails on lock drift, checksum mismatch, wrong architecture, missing optional platform CLI, or a
resolved package version that differs from the checked-in manifest. No build step queries a `latest`
Kiro URL.

Keep adapter/CLI boundaries independent: Claude, Codex, Kiro, ACP Rust SDK, A2A SDK, base/toolchain, and
proxy changes are separate update groups unless a documented compatibility dependency requires them
together.

## R4b — embed and verify immutable image evidence

- **Branch:** `agent/reliability-r4b-image-manifest`

Generate a bounded, non-secret manifest in the image containing:

- source commit and build timestamp/source-date policy;
- base image digest and target architecture;
- Node/npm and package-lock identity;
- exact adapter, SDK, embedded CLI, and Kiro versions/checksums;
- settings/config schema version, not credential contents;
- final image id/digest supplied by the build/promotion system.

Expose the manifest through immutable labels and a fixed read-only file. Extend R2a/R2c provenance to
read only bounded allowlisted labels/files through the runtime inspect boundary; never infer container
packages from the host.

Tests cover missing/malformed/oversized/conflicting labels, host/image version mismatch, Docker/Podman id
formats, and secret-shaped label rejection. A missing manifest is `UNKNOWN`/`STALE`, never a guessed
success.

## R4c — release-candidate promotion gate

- **Branch:** `agent/reliability-r4c-promotion-gate`

Extend `.github/workflows/release.yml` without weakening its existing least-privilege/tag protections:

1. build the release binary with `--locked` on every supported target;
2. build the reader/proxy/toolchain image candidates from locked inputs;
3. record immutable artifact checksums and image digests;
4. run R3's pinned compatibility cases from the candidate release binary against candidate images;
5. require host/container results or explicit environment non-goals for every claimed path;
6. compare against the checked-in pinned baseline;
7. require deliberate approval for any compatibility/pin change;
8. update compatibility matrix and changelog in the same promotion PR;
9. publish only the already-tested artifact/image identities.

The post-merge, pre-tag `workflow_dispatch` dry run remains mandatory. Download at least one binary
artifact, verify its checksum, execute `--help`, `doctor`, and the applicable explicitly acknowledged
smoke. A tag is immutable; failures fix forward with a new patch rather than moving the tag.

Live tests requiring personal/subscription auth run only on the named R3 runner under its cost and
credential policy. A GitHub-hosted release job must not silently skip them while claiming the path.

## R4d — update cadence, rollback, and ownership

- **Branch:** `agent/reliability-r4d-promotion-policy`

Document and exercise:

- weekly candidate intake by compatibility boundary;
- normally monthly reviewed pin promotion;
- urgent patch promotion for a broken claimed path;
- last-known-good binary checksum and image digest;
- exact rollback commands for binary, image, config pin, and compatibility baseline;
- owner and expiry for a temporarily unsupported/quarantined path;
- changelog/matrix update requirements;
- evidence retention and comparison to the prior successful promotion.

Rollback must not require rebuilding the old image or re-resolving old packages. Keep at least the prior
working release artifacts and immutable image digest available for the documented retention window.

## Required tests and gates

- two clean builds from the same inputs resolve identical declared packages and manifest content;
- lock drift and architecture/checksum mismatch fail closed;
- no `latest` download remains in production reader inputs;
- embedded manifest matches live package probes and immutable image identity;
- candidate smokes run from the staged release binary/image, not the developer tree;
- pinned failure blocks promotion; floating failure is advisory but visible;
- workflow permissions stay read-only except the existing scoped release job;
- no credentials appear in layers, labels, logs, build cache exports, or artifacts;
- compatibility and changelog changes are required when a pin/support row changes;
- rollback exercise restores and verifies the prior binary/image without a rebuild;
- Docker and Podman behavior is either tested or recorded as an explicit per-release non-goal.

Run the full Rust suite, container build/inspection tests, R3 pinned lane, release dry-run, and artifact
download/execute check. Report all live lanes not exercised.

## Completion

R4 is complete when production resolution is reproducible, candidate artifacts are tested before
publication, the compatibility matrix is a release gate, and rollback has been exercised. At that point
re-evaluate the global P0 and the M4 resume gates; do not automatically start R2e.
