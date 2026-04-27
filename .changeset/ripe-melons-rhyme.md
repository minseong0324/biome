---
"@biomejs/biome": patch
---

Improved `noUnnecessaryConditions` to detect conditions that are always truthy because they check built-in global class instances such as `Date`, `Map`, `Set`, `WeakMap`, and `Error`.
