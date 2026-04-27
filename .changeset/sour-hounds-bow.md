---
"@biomejs/biome": patch
---

Improved `noMisleadingReturnType` to detect `object` return annotations that hide built-in global class instances such as `Date`, `Map`, `Set`, `WeakMap`, and `Error`.
