---
status: accepted
---

# Lock a Run's terminal intent on the first finalization request

The Rust Run facade fixes either finished or failed as soon as its first
terminal operation begins, closes metric admission, and lets matching
operations retry incomplete drain, lifecycle, or flush work. A conflicting
outcome fails explicitly instead of changing the Run's meaning across cloned
handles; the temporary Python compatibility adapter retains its shipped
timeout behavior until U4 adopts the Run facade.
