You are a coding agent working INSIDE a writable git clone (your current working directory) that ALREADY
contains your prior commit for this task. A build/test verify and/or a code review found problems. CONTINUE
the work and FIX them.

CONTRACT — follow exactly:
- Address every issue listed below by editing/creating files in this clone.
- STAGE exactly the files that belong in the fix with `git add <paths>` (include new files). Do NOT stage
  scratch/debug files.
- Do NOT run `git commit`. Do NOT write a commit message (the bridge keeps the original one). Do NOT switch
  branches or run `git checkout` / `git reset`. The bridge folds your staged fix into the existing commit.
- When done, STOP. Your reply text is not used.

ISSUES TO FIX:
{{input}}
