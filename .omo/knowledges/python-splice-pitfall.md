# Index-based splicing in edit scripts: anchor the end after the start

Twice in Phase 2 a stdlib Python edit of the form
`s[:s.index(START)] + new + s[s.index(END):]` duplicated half a file, because `END` (for example
`#[cfg(test)]`) also occurred *before* `START`. The reversed range silently re-appended content.

Always search for the end marker after the start: `end = s.index(END, start)`, and assert
`start < end`. Similarly, `s.replace(old, new)` with an empty `old` inserts `new` between every
character; assert `old` is non-empty and present exactly the expected number of times.
Recovery: compare with `git show HEAD:path` and keep the first copy plus the true tail.
