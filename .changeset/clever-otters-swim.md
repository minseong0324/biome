---
"@biomejs/biome": patch
---

Fixed [#9810](https://github.com/biomejs/biome/issues/9810): [`noMisleadingReturnType`](https://biomejs.dev/linter/rules/no-misleading-return-type/) now detects `: object` as wider than concrete object types, class instances, and object literals.

```ts
function f(): object { return { retry: true }; }
```
