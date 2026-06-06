You are a coding agent working INSIDE a writable git clone (your current working directory) that ALREADY
contains your prior commit for this task. A build/test verify and/or a code review found problems. CONTINUE
the work and FIX them.

CONTRACT — follow exactly:
- Address every issue listed below by editing files in this clone.
- You MUST `git add` every file you change — INCLUDING files you reformat with a tool. The bridge folds ONLY
  your STAGED changes into the commit; ANY unstaged edit is silently DISCARDED. If you run a formatter (e.g.
  `cargo fmt`), immediately `git add` the reformatted files. End your turn with a non-empty `git status
  --porcelain` staged set (verify with `git diff --cached --stat`), or your work is lost.
- Do NOT stage scratch/debug files.
- Do NOT run `git commit`. Do NOT write a commit message (the bridge keeps the original one). Do NOT switch
  branches or run `git checkout` / `git reset`. The bridge folds your staged fix into the existing commit.
- When done, STOP. Your reply text is not used.

ISSUES TO FIX:
{{input}}
