# mgrade — where do you stand in a course, and what do you need?

`mgrade` keeps a course's grading rules and your scores in one plain text file, shows your standing, projects the rest of the term and tells you what average you need on the remaining work to finish with each letter.

```sh
cargo build --release -p mgrade
mgrade --new databases.grades        # a starter file to fill in
mgrade databases.grades              # the viewer
mgrade -p databases.grades           # print the same numbers and exit (also what you get when piped)
mgrade                               # opens the only *.grades here or in ~/.local/share/mgrade
```

`examples/` has files for four real syllabi: CSCI 4380 (a minimum exam average to pass), CSCI 4430 (the best two assignments and exams count most), CSCI 4962 (drop the lowest homework) and PHIL 4480 (an absence penalty).

## The file

```
# comments start with "# "; they are kept when mgrade saves
course CSCI 4380 Database Systems
scale A 93, A- 90, B+ 87, B 83, B- 80, C+ 77, C 73, C- 70, D+ 67, D 60
require exams >= 60 rounded      # exams average below 60 and the course grade is F

category Homework 26% n=8 drop=1
  HW1 92
  HW2 17/20
  HW3 ~85                        # a what-if
  HW4 -                          # not graded yet
category Midterm 1 14% group=exams
  Midterm 1 78
```

| Line | Meaning |
|---|---|
| `course NAME` | the title |
| `scale A 93, A- 90, …` | letter cutoffs: the lowest average that earns each letter. Below the last is F. If missing, a standard scale is used |
| `round yes` | round the final average to a whole number before looking up the letter (default: no rounding) |
| `require GROUP >= 60 [rounded]` | the weighted average of the categories tagged `group=GROUP` must reach this, or the course grade is F. It only fails you once every item in the group is graded; until then it warns |
| `absence-penalty free=3 step=1` and `absent N` | after `free` absences each extra one lowers the letter by `step` |
| `assume 85` | what ungraded work is assumed to score in the projection. Without it, your average so far |
| `category NAME WEIGHT% [options]` | a weighted category. Options: `n=10` the number of items (the ones you haven't named are empty slots), `drop=1` drop the lowest, `group=NAME`, `points` (average by total points, not the mean of the percentages), `ranked=20,20,10` (weights by rank: the best item counts 20, the next 20, the lowest 10, whatever order they came in) |
| `NAME SCORE` | an item under a category. SCORE is `85` or `85%` (out of 100), `17/20`, `~90` (a what-if), or `-` (not graded yet). Extra credit is fine: `105`, `21/20` |

A line is a setting if it starts in column 0 with a known word; indented lines are items. Weights that don't add up to 100 produce a warning.

## What it works out

* **So far** — the weighted average of the categories that have scores, renormalised over the weight that's in.
* **Projected** — every ungraded item scores the assumed value; what-if scores count as set.
* **Best / worst case** — everything ungraded at 100 / at 0.
* **Average needed** — for each letter, the average the remaining items must score (all the same) to land on that letter, by bisection, so drops, the exam rule and absence penalty are all included. "Out of reach" means even 100s won't do; a letter below the last row is already secured.
* **Drops** are saved for later: with 3 of 10 homeworks in and one to drop, nothing is dropped yet; in the projection the lowest of all 10 goes.

## The viewer

`j`/`k` move. `e` or `Enter` enters a score for the item under the cursor, and so does just typing a digit (`85`, `17/20`, `~90`, `-`); the cursor then moves to the next item, so a run of grades goes in quickly. `o` adds an item (`NAME [SCORE]`), `r` renames, `d` deletes, `x` clears a score, `w` turns a score into a what-if and back, `a` sets the assumed score, `b` the absence count, `u` undoes. `E` opens the file in `$VISUAL`/`$EDITOR` (default `mvi`) for rules, weights and categories; edits made outside while mgrade is open are picked up. Every change is written to the file at once. `Tab` shows the summary on a narrow window (it sits beside the list from 110 columns), `?` lists the keys, `q` quits.
