//! `bridge-controller` — the controller-loop primitives extracted from the
//! `a2a-bridge` binary (roadmap #9).
//!
//! This crate owns the *ports* (`TweakEffects`, `TurnRunner`, `CheckpointSink`,
//! `WarmRebuild`), the pure review→tweak loops, the git/verify/checkpoint
//! primitives, and the resolved `VerifyConfig`/`MergeConfig` input contracts.
//! The *composition* and the live *effects adapters* (`ProdEffects`,
//! `run_warm_loop`, `run_*_step`) remain in `bin/a2a-bridge` as the reference
//! adapter, which roadmap #10 later relocates into the Coordinator.
//!
//! Modules are moved in from the bin crate across six topologically-ordered,
//! behavior-preserving slices — see
//! `docs/superpowers/specs/2026-07-05-bridge-controller-extraction.md`.
//!
//! Scaffold slice: intentionally empty; module moves follow.

pub mod implement;
pub mod review;
pub mod turn;
pub mod verify;

pub use verify::VerifyConfig;
