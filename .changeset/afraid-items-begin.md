---
"@biomejs/biome": patch
---

[`noMisleadingReturnType`](https://biomejs.dev/linter/rules/no-misleading-return-type/) now detects tuple element widening and ternary-inferred unions.

```ts
// Now flagged: [string, number] is wider than ["hello", 42]
function f(): [string, number] {
  return ["hello", 42] as const;
}

// Now flagged: string is wider than "a" | "b"
function g(b: boolean): string {
  return b ? "a" : "b";
}
```
