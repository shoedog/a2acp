# 2B3b spec round-1 disposition

Review binding: `b199bfd0f44f88889a564dc4d1c352bbc7ec717b`, drafting base `5d2a82c457e626b2a108063e1de6b13b94e97f92`.
Raw independent receipt: `2026-09-29-adr0041-slice2b3b-spec-review-round1.md`.
Verdict REJECT, WRONG=4, SMELL=0, blockers=4. Revision 2 repairs the existing artifact; cap remains two.

1. Empty inventory/pack contradiction: owner explicitly authorized caller-selected SHA-1/SHA-256 for
   valid Empty object coverage, with typed provenance that does not claim sealed authentication. Captured
   still requires one pack and exact authenticated object-format agreement. Empty skips indexing and
   verifies empty inventory/fsck on both stores; malformed coverage/pack combinations refuse.
2. Worktree HOME/XDG pollution: final runner names all address the already retained `.git` child. The
   existing closed environment clears ambient state and disables global/system config. No new repository
   siblings are created; active allowlisting excludes `.gitconfig` and `git/config`. No runner change.
3. Remeasurement: revision 2 removes mandatory reuse of the path walker and requires a restore-specific
   bounded descriptor-relative, no-follow census with retained child pins, expected types/names and
   missing-file checks. Exporter behavior remains unchanged. One mechanism asserted in the review is
   incorrect: Rust `DirEntry::metadata` does not traverse symlinks, as documented by
   [the primary Rust API reference](https://doc.rust-lang.org/std/fs/struct.DirEntry.html#method.metadata).
   This is not a downgrade of the broader finding: the old walker accepts an expected-named symlink as a
   file and recursively reopens path names, so it still cannot satisfy the restore contract. The new
   regression must discriminate acceptance/path-reopen failures rather than claim metadata follows HEAD.
4. Frame symlink ordering: complete active path/type/class/conflict prevalidation now belongs to admission,
   before the first repository write. A valid refs symlink refuses with no repository and false active flag.

Independent round 2 must assess each correction and the complete revised task. No static approval yet.
