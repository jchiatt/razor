---
name: review
description: Review a change (a diff, a branch, a pull request or a document) for what it breaks, what it misses and what it does not need, with each finding's place and its smallest fix.
---

# Reviewing a change

1. Read what the change is for: the request, the issue, the commit message. A review judges the change against its purpose.
2. Read the whole change, then the code around it that it relies on. Run it, or the tests nearest it, when you can.
3. Look, in this order, for:
   - behaviour that is wrong: a case it breaks, an error it swallows, a race, a leak, input it trusts that it should not;
   - a case its purpose needs that it does not handle;
   - code that need not exist: a dead path, a layer with one caller, a second copy of something the codebase already has;
   - names and comments that mislead.
4. Give each finding its file and line, what goes wrong and when, and the smallest fix in one sentence. Rank findings by what they cost if left. Check a suspicion before reporting it as fact, and say how sure you are of the rest.
5. Prefer the fix that removes code to the one that adds it; a new check or mode needs a reason the simpler fix fails.
6. Say what you did not check. "Read all of it, ran the tests, found nothing that breaks" is a result.
